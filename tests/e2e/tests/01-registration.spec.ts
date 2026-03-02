import { test, expect } from '@playwright/test';
import { GatewayApiHelper } from '../fixtures/api-helpers';
import { createTestUser, TEST_TENANTS, TEST_CONSTANTS } from '../fixtures/test-data';

test.describe('User Registration', () => {
  let api: GatewayApiHelper;

  test.beforeEach(() => {
    api = new GatewayApiHelper(process.env.GATEWAY_URL);
  });

  test('should register a new user successfully', async () => {
    const user = createTestUser();

    const response = await api.register({
      email: user.email,
      password: user.password,
    });

    expect(response.identity_id).toBeDefined();
    expect(response.identity_id).toMatch(/^[0-9a-f-]{36}$/); // UUID format
    expect(response.email).toBe(user.email);
    expect(response.email_verified).toBe(false);
    expect(response.verification_sent).toBe(false);
  });

  test('should register user with platform and tenant', async () => {
    const user = createTestUser();
    const tenant = TEST_TENANTS[0];

    const response = await api.register({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
      tenant_slug: tenant.slug,
    });

    expect(response.identity_id).toBeDefined();
    expect(response.email).toBe(user.email);
  });

  test('should reject duplicate email registration', async () => {
    const user = createTestUser();

    // Register once
    await api.register({
      email: user.email,
      password: user.password,
    });

    // Try to register again with same email
    await expect(
      api.register({
        email: user.email,
        password: 'DifferentPass123!',
      })
    ).rejects.toThrow(/already registered/i);
  });

  test('should reject invalid email format', async () => {
    await expect(
      api.register({
        email: TEST_CONSTANTS.INVALID_EMAIL,
        password: TEST_CONSTANTS.DEFAULT_PASSWORD,
      })
    ).rejects.toThrow(/invalid email/i);
  });

  test('should reject weak password', async () => {
    const user = createTestUser();

    await expect(
      api.register({
        email: user.email,
        password: TEST_CONSTANTS.WEAK_PASSWORD,
      })
    ).rejects.toThrow(/password/i);
  });

  test('should reject registration with non-existent tenant', async () => {
    const user = createTestUser();

    await expect(
      api.register({
        email: user.email,
        password: user.password,
        platform_code: TEST_TENANTS[0].platform_code,
        tenant_slug: 'non-existent-tenant-xyz',
      })
    ).rejects.toThrow(/does not exist/i);
  });

  test('should handle concurrent registrations', async () => {
    const users = [createTestUser(), createTestUser(), createTestUser()];

    const registrations = users.map(user =>
      api.register({
        email: user.email,
        password: user.password,
      })
    );

    const results = await Promise.all(registrations);

    expect(results).toHaveLength(3);
    results.forEach(result => {
      expect(result.identity_id).toBeDefined();
    });

    // Verify all identity_ids are unique
    const identityIds = results.map(r => r.identity_id);
    const uniqueIds = new Set(identityIds);
    expect(uniqueIds.size).toBe(3);
  });
});
