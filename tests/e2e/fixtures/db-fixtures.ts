/**
 * Database fixtures for E2E tests
 * These helpers create test data directly in the database for test setup
 */

import { TestTenant, TestMembership, ROLES } from './test-data';

/**
 * Database fixture helper
 * Note: This uses direct SQL queries for setup. In production, you might use a proper DB client.
 */
export class DbFixtures {
  private baseUrl: string;

  constructor(baseUrl: string = 'http://localhost:9000') {
    this.baseUrl = baseUrl;
  }

  /**
   * Create a tenant via admin API
   * Note: Assumes you have admin access or using internal endpoints
   */
  async createTenant(tenant: TestTenant, adminToken?: string): Promise<string> {
    // This would need to use the admin API to create tenants
    // For now, we'll assume tenants are pre-created via migrations or scripts
    throw new Error('createTenant not implemented - tenants should be pre-created');
  }

  /**
   * Create a membership directly
   * In practice, memberships are created during registration or via invite flows
   */
  async createMembership(membership: TestMembership, adminToken: string): Promise<void> {
    throw new Error('createMembership not implemented - use registration or invite flows');
  }

  /**
   * Clean up test users by email pattern
   * This helps maintain a clean test database
   */
  async cleanupTestUsers(emailPattern: string = '%test-%@example.com'): Promise<void> {
    // This would require admin/internal API access
    console.warn('cleanupTestUsers not implemented - manual cleanup may be required');
  }

  /**
   * Get tenant by platform and slug (for test verification)
   */
  async getTenantBySlug(platformCode: string, slug: string): Promise<any> {
    // This would query the database or admin API
    throw new Error('getTenantBySlug not implemented - use test-setup scripts');
  }
}

/**
 * Global setup function for Playwright tests
 * This runs once before all tests to ensure test tenants exist
 */
export async function globalSetup() {
  console.log('Running global test setup...');

  // Verify gateway is accessible
  const gatewayUrl = process.env.GATEWAY_URL || 'http://localhost:9000';

  try {
    const response = await fetch(`${gatewayUrl}/health`);
    if (!response.ok) {
      throw new Error(`Gateway health check failed: ${response.status}`);
    }
    console.log('✓ Gateway is accessible');
  } catch (error) {
    console.error('✗ Gateway is not accessible:', error);
    throw error;
  }

  // Note: Tenant creation should be done via setup scripts, not in tests
  console.log('⚠ Ensure test tenants are created manually:');
  console.log(`  - ${process.env.APP1_PLATFORM_CODE || 'myapp'}/${process.env.APP1_TENANT_SLUG || 'test-tenant'}`);
  console.log(`  - ${process.env.APP2_PLATFORM_CODE || 'otherapp'}/${process.env.APP2_TENANT_SLUG || 'test-company'}`);

  console.log('Global setup complete');
}

/**
 * Global teardown function for Playwright tests
 */
export async function globalTeardown() {
  console.log('Running global test teardown...');
  // Cleanup logic here if needed
  console.log('Global teardown complete');
}
