/// <reference types="vitest/globals" />
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { App as AntdApp } from 'antd';
import { MemoryRouter } from 'react-router-dom';
import LoginForm from '../LoginForm';

// T026k — Phase 2.5 component contract.
// These tests assert the contract documented in spec.md §US1 acceptance scenarios.

const loginAdminMock = vi.fn();

vi.mock('../../hooks/useAuth', () => ({
  useAuth: () => ({
    admin: null,
    user: null,
    loginAdmin: loginAdminMock,
    logout: vi.fn(),
    isAuthenticated: false,
    isAdmin: false,
    isUser: false,
  }),
}));

const renderForm = () =>
  render(
    <AntdApp>
      <MemoryRouter>
        <LoginForm />
      </MemoryRouter>
    </AntdApp>,
  );

beforeEach(() => {
  loginAdminMock.mockReset();
});

describe('LoginForm', () => {
  it('rejects empty fields with required-field messages (spec §US1 AS-4)', async () => {
    renderForm();
    const submit = screen.getByRole('button', { name: /登\s*录|login/i });
    await userEvent.click(submit);
    expect(await screen.findByText(/请输入手机号/)).toBeInTheDocument();
    expect(await screen.findByText(/请输入密码/)).toBeInTheDocument();
    expect(loginAdminMock).not.toHaveBeenCalled();
  });

  it('rejects malformed phone numbers (spec §FR-001, data-model phone regex)', async () => {
    renderForm();
    const phone = screen.getByPlaceholderText(/手机号/);
    const password = screen.getByPlaceholderText(/密码/);
    await userEvent.type(phone, '12345');
    await userEvent.type(password, 'whatever');
    await userEvent.click(screen.getByRole('button', { name: /登\s*录|login/i }));
    expect(await screen.findByText(/请输入有效的11位手机号/)).toBeInTheDocument();
    expect(loginAdminMock).not.toHaveBeenCalled();
  });

  it('calls loginAdmin() with the entered credentials on submit (spec §US1 AS-1)', async () => {
    loginAdminMock.mockResolvedValueOnce(undefined);
    renderForm();
    await userEvent.type(screen.getByPlaceholderText(/手机号/), '18810154696');
    await userEvent.type(screen.getByPlaceholderText(/密码/), 'admin123');
    await userEvent.click(screen.getByRole('button', { name: /登\s*录|login/i }));
    await waitFor(() =>
      expect(loginAdminMock).toHaveBeenCalledWith('18810154696', 'admin123'),
    );
  });

  it('surfaces the backend error message when loginAdmin() rejects (spec §US1 AS-2)', async () => {
    loginAdminMock.mockRejectedValueOnce({ response: { data: { message: '密码错误' } } });
    renderForm();
    await userEvent.type(screen.getByPlaceholderText(/手机号/), '18810154696');
    await userEvent.type(screen.getByPlaceholderText(/密码/), 'bad');
    await userEvent.click(screen.getByRole('button', { name: /登\s*录|login/i }));
    expect(await screen.findByText('密码错误')).toBeInTheDocument();
  });
});
