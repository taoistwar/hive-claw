import { beforeEach, describe, expect, it, vi } from 'vitest'
import dayjs from 'dayjs'
import apiClient from '../api'
import {
  getRuntimeAuditLog,
  getRuntimeAuditLogs,
  toRuntimeAuditFilterTimestamp,
} from '../runtimeAuditLog'

vi.mock('../api', () => ({
  default: {
    get: vi.fn(),
  },
}))

describe('runtime audit read service', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('uses only the bounded list GET contract and the unwrapped response', async () => {
    const payload = { items: [], total: 0, offset: 20, limit: 100 }
    vi.mocked(apiClient.get).mockResolvedValueOnce({ data: payload })

    await expect(getRuntimeAuditLogs(20, 100, { outcome: 'error', agent_id: 7 })).resolves.toEqual(
      payload
    )
    expect(apiClient.get).toHaveBeenCalledWith('/runtime-audit-logs', {
      params: { offset: 20, limit: 100, outcome: 'error', agent_id: 7 },
    })
  })

  it('uses the read-only detail GET contract', async () => {
    const payload = {
      id: 42,
      request_id: null,
      session_id: null,
      agent_id: null,
      plugin_id: null,
      function_id: null,
      capability: null,
      event_type: 'plugin_invoke',
      outcome: 'success',
      elapsed_ms: null,
      error_message: null,
      payload_summary: null,
      occurred_at: '2026-07-23T00:00:00Z',
    }
    vi.mocked(apiClient.get).mockResolvedValueOnce({ data: payload })

    await expect(getRuntimeAuditLog(42)).resolves.toEqual(payload)
    expect(apiClient.get).toHaveBeenCalledWith('/runtime-audit-logs/42')
  })

  it('serializes a UTC+8 range instant as RFC3339 UTC without an eight-hour drift', () => {
    const localTime = dayjs('2026-07-24T08:30:00+08:00')

    expect(toRuntimeAuditFilterTimestamp(localTime)).toBe('2026-07-24T00:30:00.000Z')
  })
})
