/// <reference types="vitest/globals" />

import { render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { RuntimePoolCard } from '../RuntimePoolCard'
import { getPoolStats } from '../../services/capability'

vi.mock('../../services/capability', () => ({
  getPoolStats: vi.fn(),
}))

const mockedGetPoolStats = vi.mocked(getPoolStats)

describe('RuntimePoolCard audit metrics', () => {
  beforeEach(() => {
    mockedGetPoolStats.mockReset()
  })

  it('展示 runtime 审计写入和 tracing-only 指标', async () => {
    mockedGetPoolStats.mockResolvedValue({
      global: {
        in_use: 1,
        idle: 2,
        created_total: 3,
        cache_misses: 4,
        wait_count: 5,
        reset_failures: 0,
      },
      per_plugin: [],
      audit: {
        enqueued: 21,
        persisted: 20,
        tracing_only: 7,
        dropped_queue_full: 0,
        dropped_writer_closed: 0,
        dropped_no_writer: 0,
        persist_failures: 0,
      },
    })

    render(<RuntimePoolCard />)

    expect(await screen.findByText('Runtime 审计持久化')).toBeInTheDocument()
    expect(screen.getByText('audit.enqueued')).toBeInTheDocument()
    expect(screen.getByText('audit.persisted')).toBeInTheDocument()
    expect(screen.getByText('audit.tracing_only')).toBeInTheDocument()
    expect(screen.getByText('audit.dropped_queue_full')).toBeInTheDocument()
    expect(screen.getByText('audit.dropped_writer_closed')).toBeInTheDocument()
    expect(screen.getByText('audit.dropped_no_writer')).toBeInTheDocument()
    expect(screen.getByText('audit.persist_failures')).toBeInTheDocument()
    expect(screen.queryByText(/Runtime 审计持久化异常/)).not.toBeInTheDocument()
  })

  it('审计记录丢弃或写库失败时显示可诊断告警', async () => {
    mockedGetPoolStats.mockResolvedValue({
      global: {
        in_use: 0,
        idle: 0,
        created_total: 0,
        cache_misses: 0,
        wait_count: 0,
        reset_failures: 0,
      },
      per_plugin: [],
      audit: {
        enqueued: 10,
        persisted: 5,
        tracing_only: 2,
        dropped_queue_full: 3,
        dropped_writer_closed: 2,
        dropped_no_writer: 1,
        persist_failures: 4,
      },
    })

    render(<RuntimePoolCard />)

    expect(
      await screen.findByText('Runtime 审计持久化异常：dropped = 6，persist_failures = 4')
    ).toBeInTheDocument()
    expect(
      screen.getByText(
        'queue_full = 3，writer_closed = 2，no_writer = 1；请检查审计队列与数据库写入器'
      )
    ).toBeInTheDocument()
  })
})
