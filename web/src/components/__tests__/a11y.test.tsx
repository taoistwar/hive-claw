/// <reference types="vitest/globals" />
import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/react';
import { axe, toHaveNoViolations } from 'vitest-axe';
import { MemoryRouter } from 'react-router-dom';
import 'vitest-axe/extend-expect';
import LoginForm from '../LoginForm';
import AdminTable from '../AdminTable';
import PermissionGuard from '../PermissionGuard';
import type { Admin } from '../../services/admin';

// T094 / SC-008：登录页与管理员管理页通过 axe-core 自动检测时
// 0 个 critical / serious 违规项。

expect.extend({ toHaveNoViolations: toHaveNoViolations as never });

vi.mock('../../hooks/useAuth', () => ({
  useAuth: () => ({
    user: { id: 1, phone: '13800138000', nickname: 'A', role: 3, status: 1 },
    login: vi.fn(),
    logout: vi.fn(),
    isAuthenticated: true,
  }),
}));

const sampleAdmin: Admin = {
  id: 1,
  phone: '13800138000',
  nickname: 'Sample',
  role: 3,
  status: 1,
  created_at: '2026-05-25T10:00:00Z',
  updated_at: '2026-05-25T10:00:00Z',
  last_login_at: '2026-05-26T08:00:00Z',
};

async function expectNoCriticalA11yViolations(container: HTMLElement) {
  const results = await axe(container, {
    rules: {
      // antd's static <a> with javascript-style href in some demo pages
      // triggers irrelevant findings in jsdom; keep the rule but only block
      // on critical/serious below.
    },
  });
  const blocking = (results.violations ?? []).filter(
    (v) => v.impact === 'critical' || v.impact === 'serious',
  );
  // Surface non-blocking violations as console info so we don't lose them.
  if (results.violations.length > 0) {
    // eslint-disable-next-line no-console
    console.info('axe violations:', results.violations.map((v) => `${v.id} (${v.impact})`));
  }
  expect(blocking).toEqual([]);
}

describe('a11y — SC-008 gate', () => {
  it('LoginForm has no critical/serious axe violations', async () => {
    const { container } = render(
      <MemoryRouter>
        <LoginForm />
      </MemoryRouter>,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('AdminTable has no critical/serious axe violations', async () => {
    const { container } = render(
      <AdminTable
        admins={[sampleAdmin]}
        loading={false}
        pagination={{ current: 1, pageSize: 10, total: 1 }}
        onPaginationChange={() => undefined}
        onEdit={() => undefined}
        onDelete={() => undefined}
        onToggleStatus={() => undefined}
      />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('PermissionGuard "无权限" page has no critical/serious axe violations', async () => {
    const { container } = render(
      <PermissionGuard requiredRole={99}>
        <div>nope</div>
      </PermissionGuard>,
    );
    await expectNoCriticalA11yViolations(container);
  });
});
