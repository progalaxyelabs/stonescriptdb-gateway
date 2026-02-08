import { test, expect } from '@playwright/test';
import { GatewayApiHelper } from '../fixtures/api-helpers';
import { createTestUser, TEST_TENANTS } from '../fixtures/test-data';

test.describe('Token Refresh', () => {
  let api: GatewayApiHelper;

  test.beforeEach(() => {
    api = new GatewayApiHelper(process.env.GATEWAY_URL);
  });

  test('should refresh access token with valid refresh token', async () => {
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

    // Refresh the token
    const refreshResponse = await api.refresh({
      refresh_token: selectResponse.refresh_token,
    });

    expect(refreshResponse.access_token).toBeDefined();
    expect(refreshResponse.refresh_token).toBeDefined();
    expect(refreshResponse.access_token).not.toBe(selectResponse.access_token);

    // Verify new token has same claims
    const oldClaims = api.decodeJwt(selectResponse.access_token);
    const newClaims = api.decodeJwt(refreshResponse.access_token);

    expect(newClaims.identity_id).toBe(oldClaims.identity_id);
    expect(newClaims.tenant_id).toBe(oldClaims.tenant_id);
    expect(newClaims.platform_code).toBe(oldClaims.platform_code);
    expect(newClaims.email).toBe(oldClaims.email);
    expect(newClaims.role).toBe(oldClaims.role);
  });

  test('should reject refresh with invalid token', async () => {
    await expect(
      api.refresh({
        refresh_token: 'invalid-token',
      })
    ).rejects.toThrow();
  });

  test('should reject refresh with expired token', async () => {
    // This would require waiting for token expiry or using a pre-expired token
    // Skipping for now as it requires time-based testing
    test.skip();
  });

  test('should reject refresh with revoked token', async () => {
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

    // Logout (revokes refresh token)
    await api.logout(selectResponse.access_token, selectResponse.refresh_token);

    // Try to use revoked refresh token
    await expect(
      api.refresh({
        refresh_token: selectResponse.refresh_token,
      })
    ).rejects.toThrow();
  });

  test('should handle multiple refresh token chains', async () => {
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

    // Refresh multiple times
    let currentRefreshToken = selectResponse.refresh_token;
    const refreshedTokens: string[] = [];

    for (let i = 0; i < 3; i++) {
      const refreshResponse = await api.refresh({
        refresh_token: currentRefreshToken,
      });

      refreshedTokens.push(refreshResponse.access_token);
      currentRefreshToken = refreshResponse.refresh_token;

      // Each refresh should give new tokens
      expect(refreshResponse.access_token).toBeDefined();
      expect(refreshResponse.refresh_token).toBeDefined();
    }

    // All access tokens should be unique
    const uniqueTokens = new Set(refreshedTokens);
    expect(uniqueTokens.size).toBe(3);
  });

  test.skip('should automatically refresh when access token expires', async () => {
    // This would test the client-side automatic refresh behavior
    // Requires waiting for token expiry (typically 15 minutes)
    // Can be tested with short-lived tokens in test environment

    test.info().annotations.push({
      type: 'integration-test',
      description: 'Requires client-side auto-refresh logic',
    });
  });
});
