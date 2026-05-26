/// <reference types="vitest/globals" />
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import AdminTable from '../AdminTable';
import type { Admin } from '../../services/admin';

// T026m — Phase 2.5 RED.
// Pagination + role-based button visibility (spec §US3 AS-1..3, FR-005, FR-010, FR-018).

const userRef: { current: { role: number } | null } = { current: null };

vi.mock('../../hooks/useAuth', () => ({
  useAuth: () => ({ user: userRef.current }),
}));

const sampleAdmin: Admin = {
  id: 1,
  phone: '18810154696',
  nickname: 'Super',
  role: 3,
  status: 1,
  created_at: '2026-05-25T10:00:00Z',
  updated_at: '2026-05-25T10:00:00Z',
  last_login_at: '2026-05-26T08:00:00Z',
};

const otherAdmin: Admin = {
  ...sampleAdmin,
  id: 2,
  phone: '13800138001',
  nickname: 'NormalGuy',
  role: 1,
};

const baseProps = {
  admins: [sampleAdmin, otherAdmin],
  loading: false,
  pagination: { current: 1, pageSize: 10, total: 2 },
  onPaginationChange: vi.fn(),
  onEdit: vi.fn(),
  onDelete: vi.fn(),
  onToggleStatus: vi.fn(),
  searchParams: {},
  onSearchParamsChange: vi.fn(),
  onSearch: vi.fn(),
  onReset: vi.fn(),
};

describe('AdminTable', () => {
  it('renders the columns mandated by FR-010 (ID, phone, nickname, status, created, last login)', () => {
    userRef.current = { role: 3 };
    render(<AdminTable {...baseProps} />);
    // 文本可能同时出现在表头和 filter Form.Item label 中，因此用
    // getAllByText 而非 getByText（后者 ≥2 命中即抛错）。
    const expectPresent = (re: RegExp) =>
      expect(screen.getAllByText(re).length).toBeGreaterThan(0);
    expectPresent(/ID|编号/);
    expectPresent(/手机号/);
    expectPresent(/昵称/);
    expectPresent(/状态/);
    expectPresent(/注册时间|创建时间/);
    expectPresent(/最后登录/);
  });

  it('hides action buttons entirely for Normal admins (spec §US3 AS-1, hidden-not-disabled convention)', () => {
    userRef.current = { role: 1 };
    render(<AdminTable {...baseProps} />);
    expect(screen.queryByRole('button', { name: /编辑/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /删除/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /禁用|启用/ })).not.toBeInTheDocument();
  });

  it('hides only the delete button for System admins (spec §US3 AS-2, post-2026-05-26 convention)', () => {
    userRef.current = { role: 2 };
    render(<AdminTable {...baseProps} />);
    expect(screen.queryAllByRole('button', { name: /编辑/ }).length).toBeGreaterThan(0);
    expect(screen.queryAllByRole('button', { name: /禁用|启用/ }).length).toBeGreaterThan(0);
    expect(screen.queryAllByRole('button', { name: /删除/ })).toHaveLength(0);
  });

  it('shows all action buttons for Super admins (spec §US3 AS-3)', () => {
    userRef.current = { role: 3 };
    render(<AdminTable {...baseProps} />);
    expect(screen.queryAllByRole('button', { name: /编辑/ }).length).toBeGreaterThan(0);
    expect(screen.queryAllByRole('button', { name: /删除/ }).length).toBeGreaterThan(0);
  });

  it('reflects the page size + total provided by props (spec §Edge-cases pagination)', () => {
    userRef.current = { role: 3 };
    render(<AdminTable {...baseProps} pagination={{ current: 1, pageSize: 10, total: 123 }} />);
    // antd renders total as e.g. "共 123 条" via showTotal — assert via regex.
    expect(screen.getByText(/123/)).toBeInTheDocument();
  });
});
