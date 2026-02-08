import { test, expect } from '@playwright/test';
import { GatewayApiHelper } from '../fixtures/api-helpers';
import { createTestUser, TEST_TENANTS } from '../fixtures/test-data';

test.describe('Cross-Platform Identity', () => {
  let api: GatewayApiHelper;

  test.beforeEach(() => {
    api = new GatewayApiHelper(process.env.GATEWAY_URL);
  });

  test('should use same identity_id across platforms', async () => {
    const user = createTestUser();

    // Register on progalaxy platform
    const reg1 = await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code, // progalaxy
      tenant_slug: TEST_TENANTS[0].slug,
    });

    // Register same user on btechrecruiter platform
    const reg2 = await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[1].platform_code, // btechrecruiter
      tenant_slug: TEST_TENANTS[1].slug,
    });

    // Should have the same identity_id
    expect(reg1.identity_id).toBe(reg2.identity_id);
  });

  test('should maintain separate tenant memberships per platform', async () => {
    const user = createTestUser();

    // Register on both platforms
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
      tenant_slug: TEST_TENANTS[0].slug,
    });

    await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[1].platform_code,
      tenant_slug: TEST_TENANTS[1].slug,
    });

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

    // Should have different tenant lists
    expect(login1.tenants.length).toBeGreaterThan(0);
    expect(login2.tenants.length).toBeGreaterThan(0);

    // Tenant IDs should be different
    const tenant1Id = login1.tenants[0].tenant_id;
    const tenant2Id = login2.tenants[0].tenant_id;
    expect(tenant1Id).not.toBe(tenant2Id);

    // Platform codes should be different
    expect(login1.tenants[0].platform_code).toBe(TEST_TENANTS[0].platform_code);
    expect(login2.tenants[0].platform_code).toBe(TEST_TENANTS[1].platform_code);
  });

  test('should generate platform-specific JWTs', async () => {
    const user = createTestUser();

    // Register on both platforms
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
      tenant_slug: TEST_TENANTS[0].slug,
    });

    await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[1].platform_code,
      tenant_slug: TEST_TENANTS[1].slug,
    });

    // Login and select tenant on platform 1
    const login1 = await api.login({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
    });

    const select1 = await api.selectTenant(
      { tenant_id: login1.tenants[0].tenant_id },
      login1.pre_auth_token
    );

    // Login and select tenant on platform 2
    const login2 = await api.login({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[1].platform_code,
    });

    const select2 = await api.selectTenant(
      { tenant_id: login2.tenants[0].tenant_id },
      login2.pre_auth_token
    );

    // Decode both tokens
    const claims1 = api.decodeJwt(select1.access_token);
    const claims2 = api.decodeJwt(select2.access_token);

    // Same identity, different platform context
    expect(claims1.identity_id).toBe(claims2.identity_id);
    expect(claims1.platform_code).toBe(TEST_TENANTS[0].platform_code);
    expect(claims2.platform_code).toBe(TEST_TENANTS[1].platform_code);
    expect(claims1.tenant_id).not.toBe(claims2.tenant_id);
  });

  test('should use same password across platforms', async () => {
    const user = createTestUser();

    // Register on first platform
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
      tenant_slug: TEST_TENANTS[0].slug,
    });

    // Register on second platform with same credentials
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[1].platform_code,
      tenant_slug: TEST_TENANTS[1].slug,
    });

    // Should be able to login to both platforms with same password
    const login1 = await api.login({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
    });

    const login2 = await api.login({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[1].platform_code,
    });

    expect(login1.identity_id).toBe(login2.identity_id);
  });

  test('should handle user registered on one platform logging into another', async () => {
    const user = createTestUser();

    // Register ONLY on progalaxy
    const regResponse = await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
      tenant_slug: TEST_TENANTS[0].slug,
    });

    // Try to login on btechrecruiter (user exists but has no tenants there)
    const login2 = await api.login({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[1].platform_code,
    });

    // Should succeed with same identity_id but empty tenant list
    expect(login2.identity_id).toBe(regResponse.identity_id);
    expect(login2.tenants).toEqual([]);
  });

  test('should support password change affecting all platforms', async () => {
    const user = createTestUser();
    const newPassword = 'NewPassword123!';

    // Register on both platforms
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
      tenant_slug: TEST_TENANTS[0].slug,
    });

    await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[1].platform_code,
      tenant_slug: TEST_TENANTS[1].slug,
    });

    // TODO: Change password on one platform
    // Then verify old password fails on both platforms
    // And new password works on both platforms

    test.skip();
  });

  test('should support OAuth linking across platforms', async () => {
    // This would test:
    // 1. User links Google account on progalaxy
    // 2. Can use Google OAuth on btechrecruiter
    // 3. Same identity_id

    test.skip();
    test.info().annotations.push({
      type: 'oauth-cross-platform-test',
      description: 'Tests OAuth identity across platforms',
    });
  });

  test('should handle user with multiple tenants across platforms', async () => {
    const user = createTestUser();

    // Register on progalaxy - tenant 1
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
      tenant_slug: TEST_TENANTS[0].slug,
    });

    // Register on btechrecruiter - tenant 2
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[1].platform_code,
      tenant_slug: TEST_TENANTS[1].slug,
    });

    // Login on progalaxy - should show progalaxy tenants
    const login1 = await api.login({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[0].platform_code,
    });

    expect(login1.tenants.length).toBe(1);
    expect(login1.tenants[0].platform_code).toBe(TEST_TENANTS[0].platform_code);

    // Login on btechrecruiter - should show btechrecruiter tenants
    const login2 = await api.login({
      email: user.email,
      password: user.password,
      platform_code: TEST_TENANTS[1].platform_code,
    });

    expect(login2.tenants.length).toBe(1);
    expect(login2.tenants[0].platform_code).toBe(TEST_TENANTS[1].platform_code);

    // Same identity across platforms
    expect(login1.identity_id).toBe(login2.identity_id);
  });
});
