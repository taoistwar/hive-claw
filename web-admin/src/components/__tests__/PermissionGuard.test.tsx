/// <reference types="vitest/globals" />
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import PermissionGuard from '../PermissionGuard';

// T026l — Phase 2.5 RED.
// Asserts the "no permission = not rendered / redirect message" convention
// added in the 2026-05-26 spec revision (spec §US3 AS-1..4, FR-005).

const userRef: { current: { role: number } | null } = { current: null };

vi.mock('../../hooks/useAuth', () => ({
  useAuth: () => ({ user: userRef.current }),
}));

describe('PermissionGuard', () => {
  it('renders nothing/403 when user is unauthenticated', () => {
    userRef.current = null;
    render(
      <PermissionGuard requiredRole={1}>
        <div>secret</div>
      </PermissionGuard>,
    );
    expect(screen.queryByText('secret')).not.toBeInTheDocument();
    expect(screen.getByText(/无权限/)).toBeInTheDocument();
  });

  it('renders nothing when user role is below requirement (spec §US3 AS-1)', () => {
    userRef.current = { role: 1 }; // Normal admin
    render(
      <PermissionGuard requiredRole={3}>
        <div>delete-admin-button</div>
      </PermissionGuard>,
    );
    expect(screen.queryByText('delete-admin-button')).not.toBeInTheDocument();
    expect(screen.getByText(/您的权限不足/)).toBeInTheDocument();
  });

  it('renders children when user role meets or exceeds requirement (spec §US3 AS-3)', () => {
    userRef.current = { role: 3 }; // Super admin
    render(
      <PermissionGuard requiredRole={3}>
        <div>delete-admin-button</div>
      </PermissionGuard>,
    );
    expect(screen.getByText('delete-admin-button')).toBeInTheDocument();
  });

  it('treats role boundary exactly at requiredRole as permitted', () => {
    userRef.current = { role: 2 }; // System admin meeting requiredRole=2
    render(
      <PermissionGuard requiredRole={2}>
        <div>edit-admin-button</div>
      </PermissionGuard>,
    );
    expect(screen.getByText('edit-admin-button')).toBeInTheDocument();
  });
});
