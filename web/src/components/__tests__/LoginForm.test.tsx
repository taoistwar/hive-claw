/// <reference types="vitest/globals" />
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import LoginForm from '../LoginForm';

// T026k — Phase 2.5 RED.
// These tests assert the contract documented in spec.md §US1 acceptance scenarios.

const loginMock = vi.fn();

vi.mock('../../hooks/useAuth', () => ({
  useAuth: () => ({
    user: null,
    login: loginMock,
    logout: vi.fn(),
    isAuthenticated: false,
  }),
}));

const renderForm = () =>
  render(
    <MemoryRouter>
      <LoginForm />
    </MemoryRouter>,
  );

beforeEach(() => {
  loginMock.mockReset();
});

describe('LoginForm', () => {
  it('rejects empty fields with required-field messages (spec §US1 AS-4)', async () => {
    renderForm();
    const submit = screen.getByRole('button', { name: /登\s*录|login/i });
    await userEvent.click(submit);
    expect(await screen.findByText(/请输入手机号/)).toBeInTheDocument();
    expect(await screen.findByText(/请输入密码/)).toBeInTheDocument();
    expect(loginMock).not.toHaveBeenCalled();
  });

  it('rejects malformed phone numbers (spec §FR-001, data-model phone regex)', async () => {
    renderForm();
    const phone = screen.getByPlaceholderText(/手机号/);
    const password = screen.getByPlaceholderText(/密码/);
    await userEvent.type(phone, '12345');
    await userEvent.type(password, 'whatever');
    await userEvent.click(screen.getByRole('button', { name: /登\s*录|login/i }));
    expect(await screen.findByText(/请输入有效的11位手机号/)).toBeInTheDocument();
    expect(loginMock).not.toHaveBeenCalled();
  });

  it('calls login() with the entered credentials on submit (spec §US1 AS-1)', async () => {
    loginMock.mockResolvedValueOnce(undefined);
    renderForm();
    await userEvent.type(screen.getByPlaceholderText(/手机号/), '18810154696');
    await userEvent.type(screen.getByPlaceholderText(/密码/), 'admin123');
    await userEvent.click(screen.getByRole('button', { name: /登\s*录|login/i }));
    expect(loginMock).toHaveBeenCalledWith('18810154696', 'admin123');
  });

  it('surfaces the backend error message when login() rejects (spec §US1 AS-2)', async () => {
    loginMock.mockRejectedValueOnce({ response: { data: { message: '密码错误' } } });
    renderForm();
    await userEvent.type(screen.getByPlaceholderText(/手机号/), '18810154696');
    await userEvent.type(screen.getByPlaceholderText(/密码/), 'bad');
    await userEvent.click(screen.getByRole('button', { name: /登\s*录|login/i }));
    expect(await screen.findByText('密码错误')).toBeInTheDocument();
  });
});
