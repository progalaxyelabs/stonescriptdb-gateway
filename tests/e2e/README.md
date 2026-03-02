# StoneScriptDB Gateway - E2E Tests

End-to-end tests for the StoneScriptDB Gateway identity system using Playwright.

## Test Coverage

### Authentication & Authorization
- ✅ User registration (email/password)
- ✅ User login flow
- ✅ OAuth authentication (Google)
- ✅ Token refresh
- ✅ Password reset
- ✅ Logout

### Multi-Tenancy
- ✅ Tenant selection
- ✅ Tenant switching
- ✅ Multi-tenant user flows
- ✅ Cross-platform identity

### User Management
- ✅ User invitations
- ✅ Invitation acceptance
- ✅ Role-based access control (RBAC)
- ✅ Membership management

### Cross-Platform Features
- ✅ Same identity across platforms
- ✅ Platform-specific tenant memberships
- ✅ Platform-specific JWTs

## Prerequisites

1. **Gateway Service Running**
   ```bash
   # Gateway should be accessible at http://localhost:9000
   curl http://localhost:9000/health
   ```

2. **Test Tenants Created** (matching your `APP1_*` and `APP2_*` env vars)

3. **Node.js and npm**
   ```bash
   node --version  # v18+ recommended
   npm --version
   ```

## Setup

1. **Install Dependencies**
   ```bash
   cd tests/e2e
   npm install
   npx playwright install  # Install browser binaries
   ```

2. **Configure Environment**
   ```bash
   cp .env.example .env
   # Edit .env with your gateway URL and test credentials
   ```

3. **Verify Gateway Connection**
   ```bash
   curl $GATEWAY_URL/health
   ```

## Running Tests

### Run All Tests
```bash
npm test
```

### Run Specific Test Suite
```bash
npx playwright test tests/01-registration.spec.ts
npx playwright test tests/02-login.spec.ts
```

### Run in Headed Mode (See Browser)
```bash
npm run test:headed
```

### Run with UI Mode (Interactive)
```bash
npm run test:ui
```

### Debug Mode
```bash
npm run test:debug
```

### Run Tests in CI
```bash
# Non-interactive, generates reports
CI=true npm test
```

## Test Structure

```
tests/e2e/
├── fixtures/
│   ├── api-helpers.ts       # API client helpers
│   ├── test-data.ts         # Test data generators
│   └── db-fixtures.ts       # Database fixture utilities
├── tests/
│   ├── 01-registration.spec.ts      # Registration tests
│   ├── 02-login.spec.ts             # Login tests
│   ├── 03-tenant-switching.spec.ts  # Multi-tenant tests
│   ├── 04-oauth.spec.ts             # OAuth tests
│   ├── 05-token-refresh.spec.ts     # Token refresh tests
│   ├── 06-password-reset.spec.ts    # Password reset tests
│   ├── 07-invitations.spec.ts       # Invitation tests
│   ├── 08-rbac.spec.ts              # RBAC tests
│   └── 09-cross-platform.spec.ts    # Cross-platform tests
├── playwright.config.ts     # Playwright configuration
├── package.json
└── README.md
```

## Test Data Management

- Tests use randomly generated emails (`test-{timestamp}-{random}@example.com`)
- Each test creates its own users to avoid conflicts
- Test users are not automatically cleaned up (manual cleanup may be needed)

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `GATEWAY_URL` | Gateway base URL | `http://localhost:9000` |
| `APP1_PLATFORM_CODE` | First test platform code | `myapp` |
| `APP1_TENANT_SLUG` | First test tenant slug | `test-tenant` |
| `APP2_PLATFORM_CODE` | Second test platform code | `otherapp` |
| `APP2_TENANT_SLUG` | Second test tenant slug | `test-company` |

## Reports

After running tests, view the HTML report:
```bash
npm run test:report
```

Reports are generated in `playwright-report/` directory.

## CI Integration

### GitHub Actions Example
```yaml
- name: Install dependencies
  run: |
    cd tests/e2e
    npm ci
    npx playwright install --with-deps

- name: Run E2E tests
  run: |
    cd tests/e2e
    npm test
  env:
    GATEWAY_URL: http://localhost:9000

- name: Upload test reports
  if: always()
  uses: actions/upload-artifact@v3
  with:
    name: playwright-report
    path: tests/e2e/playwright-report/
```

### GitLab CI Example
```yaml
e2e-tests:
  stage: test
  image: mcr.microsoft.com/playwright:v1.48.0-jammy
  script:
    - cd tests/e2e
    - npm ci
    - npm test
  artifacts:
    when: always
    paths:
      - tests/e2e/playwright-report/
    expire_in: 30 days
```

## Troubleshooting

### Gateway Not Accessible
```bash
# Check if gateway is running
sudo systemctl status stonescriptdb-gateway

# Check network connectivity
ping localhost
curl http://localhost:9000/health
```

### Test Tenants Don't Exist
Create test tenants manually or via admin API before running tests.

### Browser Installation Issues
```bash
# Reinstall browsers
npx playwright install --force
npx playwright install-deps
```

### Rate Limiting
If tests fail due to rate limiting, reduce parallel workers in `playwright.config.ts`:
```typescript
workers: 1,  // Run tests sequentially
```

## Writing New Tests

1. Create a new spec file in `tests/`
2. Import fixtures from `../fixtures/`
3. Use `GatewayApiHelper` for API calls
4. Use `createTestUser()` for unique test users
5. Follow existing test patterns

Example:
```typescript
import { test, expect } from '@playwright/test';
import { GatewayApiHelper } from '../fixtures/api-helpers';
import { createTestUser, TEST_TENANTS } from '../fixtures/test-data';

test.describe('My Feature', () => {
  let api: GatewayApiHelper;

  test.beforeEach(() => {
    api = new GatewayApiHelper(process.env.GATEWAY_URL);
  });

  test('should do something', async () => {
    const user = createTestUser();
    // ... test code
  });
});
```

## Manual Testing Scenarios

Some tests are marked as `test.skip()` because they require:
- OAuth credentials (Google test account)
- Email service integration (password reset token extraction)
- Time-based testing (token expiry)
- Database access (invitation token extraction)

These should be tested manually or with additional infrastructure.

## Contributing

- Keep tests independent (no shared state)
- Use descriptive test names
- Add comments for complex flows
- Update this README when adding new test suites
