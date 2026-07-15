/// <reference types="vitest/globals" />
import { render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import Dashboard from '../Dashboard'

vi.mock('../../services/dashboard', () => ({
  getDashboardStats: vi.fn().mockResolvedValue({
    totalAdmins: 1,
    activeAdmins: 1,
    todayLogins: 1,
    disabledAdmins: 0,
  }),
  getRecentLogins: vi.fn().mockResolvedValue([
    {
      id: 1,
      admin_id: 1,
      admin_nickname: 'Super',
      login_at: '2026-07-15T02:15:01Z',
      ip_address: '127.0.0.1',
      success: true,
    },
  ]),
}))

describe('Dashboard', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('最近登录记录使用浏览器本地时区格式化带 Z 的 API 时间', async () => {
    let parsedIso = ''
    vi.spyOn(Date.prototype, 'toLocaleString').mockImplementation(function (this: Date) {
      parsedIso = this.toISOString()
      return 'browser-local-time'
    })

    render(<Dashboard />)

    expect(await screen.findByText('browser-local-time')).toBeInTheDocument()
    expect(parsedIso).toBe('2026-07-15T02:15:01.000Z')
  })
})
