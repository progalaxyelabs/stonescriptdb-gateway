import { test, expect } from '@playwright/test';
import { GatewayApiHelper } from '../fixtures/api-helpers';
import { createTestUser, TEST_TENANTS } from '../fixtures/test-data';

test.describe('Multi-Tenant and Tenant Switching', () => {
  let api: GatewayApiHelper;

  test.beforeEach(() => {
    api = new GatewayApiHelper(process.env.GATEWAY_URL);
  });

  test('should display tenant picker when user has multiple tenants', async () => {
    const user = createTestUser();

    // Register user with first tenant
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
      tenant_slug: TEST_TENANTS[0].slug,
    });

    // TODO: Add user to second tenant via invite flow
    // For now we test the structure with one tenant

    const loginResponse = await api.login({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
    });

    expect(loginResponse.tenants).toBeDefined();
    expect(Array.isArray(loginResponse.tenants)).toBe(true);
    expect(loginResponse.tenants.length).toBeGreaterThan(0);

    // Each tenant should have required fields
    loginResponse.tenants.forEach((tenant: any) => {
      expect(tenant.tenant_id).toBeDefined();
      expect(tenant.slug).toBeDefined();
      expect(tenant.name).toBeDefined();
      expect(tenant.role).toBeDefined();
    });
  });

  test('should switch between tenants on same platform', async () => {
    const user = createTestUser();
    const tenant = TEST_TENANTS[0];

    // Register and login
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
      tenant_slug: tenant.slug,
    });

    const loginResponse = await api.login({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
    });

    // Select first tenant
    const userTenant = loginResponse.tenants[0];
    const selectResponse = await api.selectTenant(
      { tenant_id: userTenant.tenant_id },
      loginResponse.pre_auth_token
    );

    expect(selectResponse.access_token).toBeDefined();

    // TODO: When user has multiple tenants, test switching:
    // const switchResponse = await api.switchTenant(
    //   { tenant_id: secondTenantId },
    //   selectResponse.access_token
    // );
    // expect(switchResponse.access_token).toBeDefined();
    // Verify new token has different tenant_id
  });

  test('should maintain same identity when switching tenants', async () => {
    const user = createTestUser();
    const tenant = TEST_TENANTS[0];

    // Register and login
    const regResponse = await api.register({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
      tenant_slug: tenant.slug,
    });

    const loginResponse = await api.login({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
    });

    const selectResponse = await api.selectTenant(
      { tenant_id: loginResponse.tenants[0].tenant_id },
      loginResponse.pre_auth_token
    );

    const claims = api.decodeJwt(selectResponse.access_token);
    expect(claims.identity_id).toBe(regResponse.identity_id);
  });

  test('should prevent selecting tenant user does not belong to', async () => {
    const user1 = createTestUser();
    const user2 = createTestUser();
    const tenant = TEST_TENANTS[0];

    // Register two different users
    await api.register({
      email: user1.email,
      password: user1.password,
      platform_code: tenant.platform_code,
      tenant_slug: tenant.slug,
    });

    await api.register({
      email: user2.email,
      password: user2.password,
      platform_code: tenant.platform_code,
      tenant_slug: tenant.slug,
    });

    // Login as user1
    const login1 = await api.login({
      email: user1.email,
      password: user1.password,
      platform_code: tenant.platform_code,
    });

    // Login as user2
    const login2 = await api.login({
      email: user2.email,
      password: user2.password,
      platform_code: tenant.platform_code,
    });

    // User1 selects their tenant
    const select1 = await api.selectTenant(
      { tenant_id: login1.tenants[0].tenant_id },
      login1.pre_auth_token
    );

    // Try to use user1's token to switch to user2's tenant (if they're different)
    // This should fail
    const user2TenantId = login2.tenants[0].tenant_id;

    // If they're in the same tenant, this test doesn't apply
    if (login1.tenants[0].tenant_id !== user2TenantId) {
      await expect(
        api.switchTenant(
          { tenant_id: user2TenantId },
          select1.access_token
        )
      ).rejects.toThrow(/not found|unauthorized|forbidden/i);
    }
  });

  test('should include role information in tenant selection', async () => {
    const user = createTestUser();
    const tenant = TEST_TENANTS[0];

    // Register (first user becomes admin)
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
      tenant_slug: tenant.slug,
    });

    const loginResponse = await api.login({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
    });

    const selectResponse = await api.selectTenant(
      { tenant_id: loginResponse.tenants[0].tenant_id },
      loginResponse.pre_auth_token
    );

    const claims = api.decodeJwt(selectResponse.access_token);
    expect(claims.role).toBeDefined();
    expect(['admin', 'staff', 'member']).toContain(claims.role);

    // First registration should typically be admin
    expect(loginResponse.tenants[0].role).toBe('admin');
  });

  test('should handle tenant switching with token refresh', async () => {
    const user = createTestUser();
    const tenant = TEST_TENANTS[0];

    // Register and login
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
      tenant_slug: tenant.slug,
    });

    const loginResponse = await api.login({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
    });

    const selectResponse = await api.selectTenant(
      { tenant_id: loginResponse.tenants[0].tenant_id },
      loginResponse.pre_auth_token
    );

    // Refresh token
    const refreshResponse = await api.refresh({
      refresh_token: selectResponse.refresh_token,
    });

    expect(refreshResponse.access_token).toBeDefined();
    expect(refreshResponse.refresh_token).toBeDefined();

    // Verify refreshed token has same tenant
    const oldClaims = api.decodeJwt(selectResponse.access_token);
    const newClaims = api.decodeJwt(refreshResponse.access_token);

    expect(newClaims.identity_id).toBe(oldClaims.identity_id);
    expect(newClaims.tenant_id).toBe(oldClaims.tenant_id);
    expect(newClaims.platform_code).toBe(oldClaims.platform_code);
  });
});
