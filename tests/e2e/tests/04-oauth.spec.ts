import { test, expect } from '@playwright/test';
import { GatewayApiHelper } from '../fixtures/api-helpers';
import { OAUTH_PROVIDERS, TEST_TENANTS } from '../fixtures/test-data';

test.describe('OAuth Authentication', () => {
  let api: GatewayApiHelper;

  test.beforeEach(() => {
    api = new GatewayApiHelper(process.env.GATEWAY_URL);
  });

  test('should initiate Google OAuth flow', async ({ page }) => {
    // Navigate to a test page that would trigger OAuth (or call API directly)
    const response = await page.request.post(`${process.env.GATEWAY_URL}/auth/oauth/initiate`, {
      data: {
        provider: OAUTH_PROVIDERS.GOOGLE,
        platform_code: TEST_TENANTS[0].platform_code,
        redirect_uri: 'http://localhost:4200/auth/callback',
      },
    });

    expect(response.ok()).toBeTruthy();
    const data = await response.json();

    expect(data.auth_url).toBeDefined();
    expect(data.auth_url).toContain('accounts.google.com');
    expect(data.state).toBeDefined();
  });

  test.skip('should complete Google OAuth callback flow', async ({ page }) => {
    // This test requires actual Google OAuth interaction
    // Skipping as it needs real credentials and browser automation
    // In practice, you would:
    // 1. Initiate OAuth
    // 2. Navigate to auth_url
    // 3. Login with test Google account
    // 4. Handle redirect with authorization code
    // 5. Complete callback to get tokens

    test.info().annotations.push({
      type: 'manual-test',
      description: 'Requires Google test account credentials',
    });
  });

  test.skip('should link Google account to existing identity', async () => {
    // This would test:
    // 1. Register with email/password
    // 2. Login
    // 3. Initiate OAuth to link Google account
    // 4. Complete OAuth flow
    // 5. Verify same identity_id is used

    test.info().annotations.push({
      type: 'manual-test',
      description: 'Requires OAuth integration testing',
    });
  });

  test.skip('should login with Google OAuth without password', async () => {
    // This would test:
    // 1. First time: OAuth creates identity
    // 2. Subsequent login: OAuth recognizes existing identity by email
    // 3. No password required

    test.info().annotations.push({
      type: 'manual-test',
      description: 'Requires OAuth integration testing',
    });
  });

  test('should reject OAuth initiate with invalid provider', async ({ page }) => {
    const response = await page.request.post(`${process.env.GATEWAY_URL}/auth/oauth/initiate`, {
      data: {
        provider: 'invalid-provider',
        platform_code: TEST_TENANTS[0].platform_code,
        redirect_uri: 'http://localhost:4200/auth/callback',
      },
    });

    expect(response.ok()).toBeFalsy();
  });

  test.skip('should handle OAuth state mismatch', async () => {
    // Test CSRF protection by providing wrong state parameter
    test.info().annotations.push({
      type: 'security-test',
      description: 'Tests OAuth state parameter validation',
    });
  });

  test.skip('should list OAuth connections for user', async () => {
    // This would test the GET /auth/oauth/connections endpoint
    // Requires a user with linked OAuth accounts

    test.info().annotations.push({
      type: 'todo',
      description: 'Implement after OAuth connection management is tested',
    });
  });

  test.skip('should delete OAuth connection', async () => {
    // This would test the DELETE /auth/oauth/connections/:provider endpoint
    // Requires a user with linked OAuth accounts

    test.info().annotations.push({
      type: 'todo',
      description: 'Implement after OAuth connection management is tested',
    });
  });
});

test.describe('OAuth - Mock/Stub Tests', () => {
  test('should verify OAuth endpoints exist', async ({ page }) => {
    const api = new GatewayApiHelper(process.env.GATEWAY_URL);

    // Just verify the endpoints respond (even if they fail due to config)
    const initiateResponse = await page.request.post(
      `${process.env.GATEWAY_URL}/auth/oauth/initiate`,
      {
        data: {
          provider: 'google',
          platform_code: 'progalaxy',
          redirect_uri: 'http://test.com/callback',
        },
        failOnStatusCode: false,
      }
    );

    // Should at least get a response (might be error if OAuth not configured)
    expect([200, 400, 500]).toContain(initiateResponse.status());
  });
});
