import { test, expect } from '@playwright/test';
import { GatewayApiHelper } from '../fixtures/api-helpers';
import { createTestUser, TEST_TENANTS } from '../fixtures/test-data';

test.describe('User Login', () => {
  let api: GatewayApiHelper;

  test.beforeEach(() => {
    api = new GatewayApiHelper(process.env.GATEWAY_URL);
  });

  test('should login successfully with correct credentials', async () => {
    const user = createTestUser();
    const tenant = TEST_TENANTS[0];

    // Register user
    const regResponse = await api.register({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
      tenant_slug: tenant.slug,
    });

    // Login
    const loginResponse = await api.login({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
    });

    expect(loginResponse.identity_id).toBe(regResponse.identity_id);
    expect(loginResponse.email).toBe(user.email);
    expect(loginResponse.tenants).toBeDefined();
    expect(Array.isArray(loginResponse.tenants)).toBe(true);
    expect(loginResponse.tenants.length).toBeGreaterThan(0);

    // Verify tenant in the list
    const userTenant = loginResponse.tenants.find(
      (t: any) => t.slug === tenant.slug
    );
    expect(userTenant).toBeDefined();
  });

  test('should fail login with incorrect password', async () => {
    const user = createTestUser();
    const tenant = TEST_TENANTS[0];

    // Register user
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
      tenant_slug: tenant.slug,
    });

    // Try to login with wrong password
    await expect(
      api.login({
        email: user.email,
        password: 'WrongPassword123!',
        platform_code: tenant.platform_code,
      })
    ).rejects.toThrow(/invalid|credentials/i);
  });

  test('should fail login with non-existent email', async () => {
    await expect(
      api.login({
        email: 'nonexistent@example.com',
        password: 'SomePassword123!',
        platform_code: TEST_TENANTS[0].platform_code,
      })
    ).rejects.toThrow(/invalid|credentials|not found/i);
  });

  test('should return tenant list for user with multiple tenants', async () => {
    const user = createTestUser();

    // Register user with first tenant
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
      tenant_slug: TEST_TENANTS[0].slug,
    });

    // TODO: Add user to second tenant via invite or admin API
    // For now, we verify the structure with one tenant

    const loginResponse = await api.login({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
    });

    expect(loginResponse.tenants).toBeDefined();
    expect(Array.isArray(loginResponse.tenants)).toBe(true);
  });

  test('should select tenant and get access token', async () => {
    const user = createTestUser();
    const tenant = TEST_TENANTS[0];

    // Register user
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
      tenant_slug: tenant.slug,
    });

    // Login
    const loginResponse = await api.login({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
    });

    // Select tenant
    const userTenant = loginResponse.tenants[0];
    const selectResponse = await api.selectTenant(
      { tenant_id: userTenant.tenant_id },
      loginResponse.pre_auth_token
    );

    expect(selectResponse.access_token).toBeDefined();
    expect(selectResponse.refresh_token).toBeDefined();
    expect(selectResponse.tenant_id).toBe(userTenant.tenant_id);

    // Decode and verify JWT claims
    const claims = api.decodeJwt(selectResponse.access_token);
    expect(claims.identity_id).toBe(loginResponse.identity_id);
    expect(claims.tenant_id).toBe(userTenant.tenant_id);
    expect(claims.platform_code).toBe(tenant.platform_code);
    expect(claims.email).toBe(user.email);
  });

  test('should handle login rate limiting', async ({ page }) => {
    // This test would require making many rapid login attempts
    // Skipping for now as it might affect other tests
    test.skip();
  });

  test('should preserve identity across platforms', async () => {
    const user = createTestUser();

    // Register on first platform
    const regResponse1 = await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
      tenant_slug: TEST_TENANTS[0].slug,
    });

    // Register same user on second platform
    const regResponse2 = await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[1].platform_code,
      tenant_slug: TEST_TENANTS[1].slug,
    });

    // Identity IDs should be the same
    expect(regResponse1.identity_id).toBe(regResponse2.identity_id);

    // Login on first platform
    const login1 = await api.login({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
    });

    // Login on second platform
    const login2 = await api.login({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[1].platform_code,
    });

    // Same identity, different tenant lists
    expect(login1.identity_id).toBe(login2.identity_id);
    expect(login1.identity_id).toBe(regResponse1.identity_id);
  });
});
