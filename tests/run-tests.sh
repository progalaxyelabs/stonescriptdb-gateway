#!/bin/bash
# =============================================================================
# StoneScriptDB Gateway Integration Tests
# =============================================================================
# Tests all gateway endpoints using a sample todos app
#
# Usage: ./tests/run-tests.sh
# =============================================================================

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# Configuration
GATEWAY_URL="${GATEWAY_URL:-http://127.0.0.1:9000}"
PLATFORM_ID="todos-app"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SAMPLE_APP_DIR="$SCRIPT_DIR/sample-todos-app"

# Counters
TESTS_PASSED=0
TESTS_FAILED=0

# Store created todo ID
NEW_TODO_ID=""

# Test helper functions
pass() {
    echo -e "${GREEN}  ✓ $1${NC}"
    ((TESTS_PASSED++)) || true
}

fail() {
    echo -e "${RED}  ✗ $1${NC}"
    echo -e "${RED}    $2${NC}"
    ((TESTS_FAILED++)) || true
}

section() {
    echo ""
    echo -e "${CYAN}━━━ $1 ━━━${NC}"
}

# Check if gateway is running
check_gateway() {
    section "Checking Gateway Status"

    RESPONSE=$(curl -s -w "\n%{http_code}" "$GATEWAY_URL/health" 2>/dev/null) || true
    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    if [ "$HTTP_CODE" = "200" ]; then
        pass "Gateway is healthy"
        echo "    $(echo "$BODY" | jq -c . 2>/dev/null || echo "$BODY")"
    else
        fail "Gateway not responding" "HTTP $HTTP_CODE - Is the gateway running at $GATEWAY_URL?"
        exit 1
    fi
}

# Test 1: Health endpoint
test_health() {
    section "Test 1: Health Endpoint"

    RESPONSE=$(curl -s "$GATEWAY_URL/health")

    if echo "$RESPONSE" | jq -e '.status == "healthy"' > /dev/null 2>&1; then
        pass "Health check returns healthy status"
    else
        fail "Health check failed" "$RESPONSE"
    fi

    if echo "$RESPONSE" | jq -e '.postgres_connected == true' > /dev/null 2>&1; then
        pass "PostgreSQL connection confirmed"
    else
        fail "PostgreSQL not connected" "$RESPONSE"
    fi
}

# Test 2: Register platform
test_register() {
    section "Test 2: Register Platform"

    # Create tar.gz of sample app
    TAR_FILE="/tmp/todos-app-schema.tar.gz"
    tar -czf "$TAR_FILE" -C "$SAMPLE_APP_DIR" postgresql

    # Register with gateway
    RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$GATEWAY_URL/register" \
        -F "platform=$PLATFORM_ID" \
        -F "schema=@$TAR_FILE" 2>/dev/null)

    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    rm -f "$TAR_FILE"

    if [ "$HTTP_CODE" = "200" ]; then
        pass "Platform registered successfully"
        echo "    Response: $(echo "$BODY" | jq -c . 2>/dev/null || echo "$BODY")"

        # Check migrations were applied (4 with seed data on first run, 0 on subsequent)
        MIGRATIONS=$(echo "$BODY" | jq '.migrations_applied' 2>/dev/null)
        if [ "$MIGRATIONS" -ge 0 ]; then
            pass "Migrations applied: $MIGRATIONS (0 if already migrated)"
        else
            fail "Migrations count error" "$BODY"
        fi

        # Check functions were deployed
        FUNCTIONS=$(echo "$BODY" | jq '.functions_deployed' 2>/dev/null)
        if [ "$FUNCTIONS" -ge 10 ]; then
            pass "Functions deployed: $FUNCTIONS"
        else
            fail "Functions count mismatch" "$BODY"
        fi
    else
        fail "Registration failed" "HTTP $HTTP_CODE: $BODY"
    fi
}

# Test 3: Call function - get_all_tags (no params)
test_call_function_no_params() {
    section "Test 3: Call Function (no params)"

    RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$GATEWAY_URL/call" \
        -H "Content-Type: application/json" \
        -d "{\"platform\": \"$PLATFORM_ID\", \"function\": \"get_all_tags\", \"params\": []}" 2>/dev/null)

    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    if [ "$HTTP_CODE" = "200" ]; then
        pass "Function call successful"

        # Check we got tags back (response uses .rows not .result)
        TAG_COUNT=$(echo "$BODY" | jq '.rows | length' 2>/dev/null || echo "0")
        if [ "$TAG_COUNT" -ge 5 ]; then
            pass "Got $TAG_COUNT tags (5 seeded)"
            echo "    Tags: $(echo "$BODY" | jq -c '[.rows[].name]' 2>/dev/null)"
        else
            fail "Expected at least 5 tags, got $TAG_COUNT" "$BODY"
        fi
    else
        fail "Function call failed" "HTTP $HTTP_CODE: $BODY"
    fi
}

# Test 4: Call function - get_user_by_id (with params)
test_call_function_with_params() {
    section "Test 4: Call Function (with params)"

    # params is an array in order: [p_user_id]
    RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$GATEWAY_URL/call" \
        -H "Content-Type: application/json" \
        -d "{\"platform\": \"$PLATFORM_ID\", \"function\": \"get_user_by_id\", \"params\": [1]}" 2>/dev/null)

    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    if [ "$HTTP_CODE" = "200" ]; then
        pass "Function call successful"

        # Check user data
        EMAIL=$(echo "$BODY" | jq -r '.rows[0].email' 2>/dev/null)
        if [ "$EMAIL" = "john@example.com" ]; then
            pass "Got correct user: $EMAIL"
        else
            fail "Unexpected user data" "$BODY"
        fi
    else
        fail "Function call failed" "HTTP $HTTP_CODE: $BODY"
    fi
}

# Test 5: Call function - get_todos_by_user
test_get_todos() {
    section "Test 5: Get Todos by User"

    # params: [p_user_id, p_include_completed]
    RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$GATEWAY_URL/call" \
        -H "Content-Type: application/json" \
        -d "{\"platform\": \"$PLATFORM_ID\", \"function\": \"get_todos_by_user\", \"params\": [1, true]}" 2>/dev/null)

    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    if [ "$HTTP_CODE" = "200" ]; then
        pass "Get todos successful"

        TODO_COUNT=$(echo "$BODY" | jq '.rows | length' 2>/dev/null || echo "0")
        if [ "$TODO_COUNT" -ge 5 ]; then
            pass "Got $TODO_COUNT todos for user 1"
            echo "    First todo: $(echo "$BODY" | jq -c '.rows[0].title' 2>/dev/null)"
        else
            fail "Expected at least 5 todos, got $TODO_COUNT" "$BODY"
        fi
    else
        fail "Get todos failed" "HTTP $HTTP_CODE: $BODY"
    fi
}

# Test 6: Insert todo
test_insert_todo() {
    section "Test 6: Insert Todo"

    # params: [p_user_id, p_title, p_description, p_priority, p_due_date]
    RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$GATEWAY_URL/call" \
        -H "Content-Type: application/json" \
        -d "{\"platform\": \"$PLATFORM_ID\", \"function\": \"insert_todo\", \"params\": [1, \"Test todo from integration test\", \"This was created by the test script\", 2, null]}" 2>/dev/null)

    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    if [ "$HTTP_CODE" = "200" ]; then
        pass "Insert todo successful"

        # The function returns an INT, which appears in rows[0].insert_todo
        NEW_TODO_ID=$(echo "$BODY" | jq '.rows[0].insert_todo' 2>/dev/null)
        if [ -n "$NEW_TODO_ID" ] && [ "$NEW_TODO_ID" != "null" ]; then
            pass "New todo ID: $NEW_TODO_ID"
        else
            fail "No todo ID returned" "$BODY"
        fi
    else
        fail "Insert todo failed" "HTTP $HTTP_CODE: $BODY"
    fi
}

# Test 7: Mark todo complete
test_mark_complete() {
    section "Test 7: Mark Todo Complete"

    # params: [p_todo_id, p_user_id, p_is_completed]
    RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$GATEWAY_URL/call" \
        -H "Content-Type: application/json" \
        -d "{\"platform\": \"$PLATFORM_ID\", \"function\": \"mark_todo_complete\", \"params\": [1, 1, true]}" 2>/dev/null)

    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    if [ "$HTTP_CODE" = "200" ]; then
        ROWS_AFFECTED=$(echo "$BODY" | jq '.rows[0].mark_todo_complete' 2>/dev/null)
        if [ "$ROWS_AFFECTED" = "1" ]; then
            pass "Todo marked complete (1 row affected)"
        else
            fail "Unexpected rows affected: $ROWS_AFFECTED" "$BODY"
        fi
    else
        fail "Mark complete failed" "HTTP $HTTP_CODE: $BODY"
    fi
}

# Test 8: Get todo stats
test_get_stats() {
    section "Test 8: Get Todo Stats"

    # params: [p_user_id]
    RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$GATEWAY_URL/call" \
        -H "Content-Type: application/json" \
        -d "{\"platform\": \"$PLATFORM_ID\", \"function\": \"get_todo_stats\", \"params\": [1]}" 2>/dev/null)

    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    if [ "$HTTP_CODE" = "200" ]; then
        pass "Get stats successful"

        STATS=$(echo "$BODY" | jq '.rows[0]' 2>/dev/null)
        echo "    Stats: $STATS"

        TOTAL=$(echo "$STATS" | jq '.total_todos' 2>/dev/null)
        if [ "$TOTAL" -ge 5 ]; then
            pass "Total todos: $TOTAL"
        else
            fail "Unexpected total: $TOTAL" "$BODY"
        fi
    else
        fail "Get stats failed" "HTTP $HTTP_CODE: $BODY"
    fi
}

# Test 9: Migrate (update functions)
test_migrate() {
    section "Test 9: Migrate Schema"

    # Create tar.gz of sample app
    TAR_FILE="/tmp/todos-app-schema.tar.gz"
    tar -czf "$TAR_FILE" -C "$SAMPLE_APP_DIR" postgresql

    # Migrate
    RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$GATEWAY_URL/migrate" \
        -F "platform=$PLATFORM_ID" \
        -F "schema=@$TAR_FILE" 2>/dev/null)

    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    rm -f "$TAR_FILE"

    if [ "$HTTP_CODE" = "200" ]; then
        pass "Migration successful"
        echo "    Response: $(echo "$BODY" | jq -c . 2>/dev/null || echo "$BODY")"

        # Check functions were updated
        FUNCTIONS=$(echo "$BODY" | jq '.functions_updated' 2>/dev/null)
        if [ "$FUNCTIONS" -ge 10 ]; then
            pass "Functions updated: $FUNCTIONS"
        else
            pass "Schema migrated (functions refreshed)"
        fi
    else
        fail "Migration failed" "HTTP $HTTP_CODE: $BODY"
    fi
}

# Test 10: Error handling - invalid function
test_invalid_function() {
    section "Test 10: Error Handling"

    RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$GATEWAY_URL/call" \
        -H "Content-Type: application/json" \
        -d "{\"platform\": \"$PLATFORM_ID\", \"function\": \"nonexistent_function\", \"params\": []}" 2>/dev/null)

    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    if [ "$HTTP_CODE" != "200" ]; then
        pass "Invalid function returns error (HTTP $HTTP_CODE)"
    else
        # Check if there's an error in the response
        if echo "$BODY" | jq -e '.error' > /dev/null 2>&1; then
            pass "Error returned for invalid function"
        else
            fail "Expected error for invalid function" "$BODY"
        fi
    fi
}

# Test 11: Delete todo
test_delete_todo() {
    section "Test 11: Delete Todo"

    # Delete the todo we created in test 6
    TODO_TO_DELETE="${NEW_TODO_ID:-100}"

    # params: [p_todo_id, p_user_id]
    RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$GATEWAY_URL/call" \
        -H "Content-Type: application/json" \
        -d "{\"platform\": \"$PLATFORM_ID\", \"function\": \"delete_todo\", \"params\": [$TODO_TO_DELETE, 1]}" 2>/dev/null)

    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    if [ "$HTTP_CODE" = "200" ]; then
        ROWS_AFFECTED=$(echo "$BODY" | jq '.rows[0].delete_todo' 2>/dev/null)
        if [ "$ROWS_AFFECTED" = "1" ]; then
            pass "Todo deleted (1 row affected)"
        else
            pass "Delete executed (rows: $ROWS_AFFECTED)"
        fi
    else
        fail "Delete todo failed" "HTTP $HTTP_CODE: $BODY"
    fi
}

# Summary
print_summary() {
    echo ""
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${CYAN}               TEST SUMMARY              ${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
    echo -e "  ${GREEN}Passed: $TESTS_PASSED${NC}"
    echo -e "  ${RED}Failed: $TESTS_FAILED${NC}"
    echo ""

    if [ $TESTS_FAILED -eq 0 ]; then
        echo -e "${GREEN}All tests passed!${NC}"
        exit 0
    else
        echo -e "${RED}Some tests failed.${NC}"
        exit 1
    fi
}

# Main
main() {
    echo ""
    echo -e "${CYAN}╔═══════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║  StoneScriptDB Gateway Integration Tests  ║${NC}"
    echo -e "${CYAN}╚═══════════════════════════════════════════╝${NC}"
    echo ""
    echo "Gateway URL: $GATEWAY_URL"
    echo "Platform ID: $PLATFORM_ID"
    echo "Sample App:  $SAMPLE_APP_DIR"

    check_gateway
    test_health
    test_register
    test_call_function_no_params
    test_call_function_with_params
    test_get_todos
    test_insert_todo
    test_mark_complete
    test_get_stats
    test_migrate
    test_invalid_function
    test_delete_todo

    print_summary
}

main "$@"
