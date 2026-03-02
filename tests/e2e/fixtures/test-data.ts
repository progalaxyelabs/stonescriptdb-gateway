/**
 * Test data generators and fixtures
 */

export interface TestUser {
  email: string;
  password: string;
  identity_id?: string;
  access_token?: string;
  refresh_token?: string;
}

export interface TestTenant {
  id?: string;
  platform_code: string;
  slug: string;
  name: string;
}

export interface TestMembership {
  identity_id: string;
  tenant_id: string;
  role: 'admin' | 'staff' | 'member';
}

/**
 * Generate a unique email for testing
 */
export function generateTestEmail(prefix: string = 'test'): string {
  const timestamp = Date.now();
  const random = Math.floor(Math.random() * 10000);
  return `${prefix}-${timestamp}-${random}@example.com`;
}

/**
 * Generate a strong password
 */
export function generatePassword(): string {
  return `TestPass${Math.floor(Math.random() * 100000)}!`;
}

// Read platform configs from environment (set in .env, see .env.example)
const app1PlatformCode = process.env.APP1_PLATFORM_CODE || 'myapp';
const app1TenantSlug = process.env.APP1_TENANT_SLUG || 'test-tenant';
const app2PlatformCode = process.env.APP2_PLATFORM_CODE || 'otherapp';
const app2TenantSlug = process.env.APP2_TENANT_SLUG || 'test-company';

/**
 * Default test tenants — configured via APP1_* and APP2_* env vars in .env
 */
export const TEST_TENANTS: TestTenant[] = [
  {
    platform_code: app1PlatformCode,
    slug: app1TenantSlug,
    name: 'Test Tenant',
  },
  {
    platform_code: app2PlatformCode,
    slug: app2TenantSlug,
    name: 'Test Company',
  },
];

/**
 * Create a test user with random credentials
 */
export function createTestUser(overrides?: Partial<TestUser>): TestUser {
  return {
    email: generateTestEmail(),
    password: generatePassword(),
    ...overrides,
  };
}

/**
 * Create multiple test users
 */
export function createTestUsers(count: number): TestUser[] {
  return Array.from({ length: count }, () => createTestUser());
}

/**
 * Role hierarchy for testing RBAC
 */
export const ROLES = {
  ADMIN: 'admin',
  STAFF: 'staff',
  MEMBER: 'member',
} as const;

/**
 * Test scenarios for multi-tenant flows
 */
export const MULTI_TENANT_SCENARIOS = {
  SINGLE_PLATFORM_SINGLE_TENANT: {
    description: 'User belongs to one tenant on one platform',
    platforms: [app1PlatformCode],
    tenantsPerPlatform: 1,
  },
  SINGLE_PLATFORM_MULTI_TENANT: {
    description: 'User belongs to multiple tenants on same platform',
    platforms: [app1PlatformCode],
    tenantsPerPlatform: 3,
  },
  MULTI_PLATFORM_SINGLE_TENANT: {
    description: 'User belongs to one tenant on each platform',
    platforms: [app1PlatformCode, app2PlatformCode],
    tenantsPerPlatform: 1,
  },
  MULTI_PLATFORM_MULTI_TENANT: {
    description: 'User belongs to multiple tenants on multiple platforms',
    platforms: [app1PlatformCode, app2PlatformCode],
    tenantsPerPlatform: 2,
  },
};

/**
 * OAuth providers for testing
 */
export const OAUTH_PROVIDERS = {
  GOOGLE: 'google',
} as const;

/**
 * Common test constants
 */
export const TEST_CONSTANTS = {
  DEFAULT_PASSWORD: 'TestPass123!',
  WEAK_PASSWORD: '123',
  INVALID_EMAIL: 'not-an-email',
  TOKEN_EXPIRY_SHORT: 60, // seconds
  TOKEN_EXPIRY_LONG: 3600, // seconds
};
