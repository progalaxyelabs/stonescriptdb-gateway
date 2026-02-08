/**
 * API Helper functions for interacting with the Gateway API
 */

export interface RegisterPayload {
  email: string;
  password: string;
  platform_code?: string;
  tenant_slug?: string;
}

export interface LoginPayload {
  email: string;
  password: string;
  platform_code: string;
}

export interface SelectTenantPayload {
  tenant_id: string;
}

export interface SwitchTenantPayload {
  tenant_id: string;
}

export interface RefreshPayload {
  refresh_token: string;
}

export interface PasswordResetRequestPayload {
  email: string;
  platform_code: string;
}

export interface PasswordResetConfirmPayload {
  token: string;
  new_password: string;
}

export class GatewayApiHelper {
  private baseUrl: string;

  constructor(baseUrl: string = 'http://192.168.122.173:9000') {
    this.baseUrl = baseUrl;
  }

  async register(payload: RegisterPayload) {
    const response = await fetch(`${this.baseUrl}/auth/register`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });

    const data = await response.json();

    if (!response.ok) {
      throw new Error(`Registration failed: ${JSON.stringify(data)}`);
    }

    return data;
  }

  async login(payload: LoginPayload) {
    const response = await fetch(`${this.baseUrl}/auth/login`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });

    const data = await response.json();

    if (!response.ok) {
      throw new Error(`Login failed: ${JSON.stringify(data)}`);
    }

    return data;
  }

  async selectTenant(payload: SelectTenantPayload, accessToken: string) {
    const response = await fetch(`${this.baseUrl}/auth/select-tenant`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Authorization': `Bearer ${accessToken}`,
      },
      body: JSON.stringify(payload),
    });

    const data = await response.json();

    if (!response.ok) {
      throw new Error(`Select tenant failed: ${JSON.stringify(data)}`);
    }

    return data;
  }

  async switchTenant(payload: SwitchTenantPayload, accessToken: string) {
    const response = await fetch(`${this.baseUrl}/auth/switch-tenant`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Authorization': `Bearer ${accessToken}`,
      },
      body: JSON.stringify(payload),
    });

    const data = await response.json();

    if (!response.ok) {
      throw new Error(`Switch tenant failed: ${JSON.stringify(data)}`);
    }

    return data;
  }

  async refresh(payload: RefreshPayload) {
    const response = await fetch(`${this.baseUrl}/auth/refresh`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });

    const data = await response.json();

    if (!response.ok) {
      throw new Error(`Token refresh failed: ${JSON.stringify(data)}`);
    }

    return data;
  }

  async logout(accessToken: string, refreshToken: string) {
    const response = await fetch(`${this.baseUrl}/auth/logout`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Authorization': `Bearer ${accessToken}`,
      },
      body: JSON.stringify({ refresh_token: refreshToken }),
    });

    if (!response.ok) {
      const data = await response.json();
      throw new Error(`Logout failed: ${JSON.stringify(data)}`);
    }

    return true;
  }

  async passwordResetRequest(payload: PasswordResetRequestPayload) {
    const response = await fetch(`${this.baseUrl}/auth/password-reset-request`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });

    const data = await response.json();

    if (!response.ok) {
      throw new Error(`Password reset request failed: ${JSON.stringify(data)}`);
    }

    return data;
  }

  async passwordResetConfirm(payload: PasswordResetConfirmPayload) {
    const response = await fetch(`${this.baseUrl}/auth/password-reset-confirm`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });

    const data = await response.json();

    if (!response.ok) {
      throw new Error(`Password reset confirm failed: ${JSON.stringify(data)}`);
    }

    return data;
  }

  async getJwks() {
    const response = await fetch(`${this.baseUrl}/auth/jwks`, {
      method: 'GET',
    });

    return await response.json();
  }

  /**
   * Decode JWT payload without verification (for testing)
   */
  decodeJwt(token: string) {
    const parts = token.split('.');
    if (parts.length !== 3) {
      throw new Error('Invalid JWT format');
    }

    const payload = Buffer.from(parts[1], 'base64').toString('utf-8');
    return JSON.parse(payload);
  }
}
