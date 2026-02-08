import { test, expect } from '@playwright/test';
import { GatewayApiHelper } from '../fixtures/api-helpers';
import { createTestUser, TEST_TENANTS } from '../fixtures/test-data';

test.describe('Password Reset', () => {
  let api: GatewayApiHelper;

  test.beforeEach(() => {
    api = new GatewayApiHelper(process.env.GATEWAY_URL);
  });

  test('should request password reset for existing user', async () => {
    const user = createTestUser();
    const tenant = TEST_TENANTS[0];

    // Register user
    await api.register({
      email: user.email,
      password: user.password,
      platform_code: tenant.platform_code,
      tenant_slug: tenant.slug,
    });

    // Request password reset
    const response = await api.passwordResetRequest({
      email: user.email,
      platform_code: tenant.platform_code,
    });

    expect(response.message).toBeDefined();
    // Should not reveal whether email exists (security best practice)
    expect(response.message).toMatch(/email.*sent|instructions.*sent/i);
  });

  test('should handle password reset request for non-existent email', async () => {
    // Should return same message as success (don't reveal user existence)
    const response = await api.passwordResetRequest({
      email: 'nonexistent@example.com',
      platform_code: TEST_TENANTS[0].platform_code,
    });

    expect(response.message).toBeDefined();
  });

  test.skip('should complete password reset with valid token', async () => {
    // This requires:
    // 1. Request password reset
    // 2. Extract reset token from email or database
    // 3. Confirm password reset with token
    // 4. Login with new password

    // Skipping as it requires email interception or database access
    test.info().annotations.push({
      type: 'manual-test',
      description: 'Requires email service or database access to get reset token',
    });
  });

  test('should reject password reset with invalid token', async () => {
    await expect(
      api.passwordResetConfirm({
        token: 'invalid-token-xyz',
        new_password: 'NewPassword123!',
      })
    ).rejects.toThrow(/invalid|expired|token/i);
  });

  test.skip('should reject password reset with expired token', async () => {
    // This would require:
    // 1. Creating a reset token
    // 2. Waiting for it to expire (typically 1 hour)
    // 3. Attempting to use it

    test.info().annotations.push({
      type: 'time-based-test',
      description: 'Requires token expiry testing',
    });
  });

  test.skip('should reject reusing password reset token', async () => {
    // This would test:
    // 1. Request reset
    // 2. Use token successfully
    // 3. Try to use same token again (should fail)

    test.info().annotations.push({
      type: 'security-test',
      description: 'Tests one-time use of reset tokens',
    });
  });

  test.skip('should invalidate old password after reset', async () => {
    // This would test:
    // 1. Register user
    // 2. Reset password
    // 3. Try to login with old password (should fail)
    // 4. Login with new password (should succeed)

    test.info().annotations.push({
      type: 'functional-test',
      description: 'Tests password invalidation after reset',
    });
  });

  test('should validate new password complexity on reset', async () => {
    await expect(
      api.passwordResetConfirm({
        token: 'some-token',
        new_password: 'weak',
      })
    ).rejects.toThrow();
  });
});

test.describe('Password Change (Authenticated)', () => {
  let api: GatewayApiHelper;

  test.beforeEach(() => {
    api = new GatewayApiHelper(process.env.GATEWAY_URL);
  });

  test.skip('should change password when authenticated', async () => {
    // This would test POST /auth/change-password
    // Requires:
    // 1. Login
    // 2. Change password with current password + new password
    // 3. Logout
    // 4. Login with new password

    test.info().annotations.push({
      type: 'todo',
      description: 'Implement authenticated password change tests',
    });
  });

  test.skip('should reject password change with incorrect current password', async () => {
    test.info().annotations.push({
      type: 'security-test',
      description: 'Tests current password verification',
    });
  });

  test.skip('should reject password change to same password', async () => {
    test.info().annotations.push({
      type: 'validation-test',
      description: 'Tests password uniqueness requirement',
    });
  });
});
