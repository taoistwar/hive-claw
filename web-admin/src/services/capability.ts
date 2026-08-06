// 004 US4 — Capability service (T106)
import apiClient from './api'

export interface CapabilityItem {
  name: string
  description: string
  is_dangerous: boolean
  category_id: number | null
  created_at: string
}

export interface CapabilityDetail extends CapabilityItem {
  allowed_plugins?: string[]
}

export interface ListCapabilitiesParams {
  name?: string
  description?: string
  is_dangerous?: boolean
  category_id?: number
  offset?: number
  limit?: number
}

export async function listCapabilities(
  params?: ListCapabilitiesParams
): Promise<[CapabilityItem[], number]> {
  const resp = await apiClient.get<[CapabilityItem[], number]>('/capabilities', { params })
  return resp.data
}

export async function getCapabilityDetail(name: string): Promise<CapabilityDetail> {
  const resp = await apiClient.get<CapabilityDetail>(`/capabilities/${name}`)
  return resp.data
}

export async function createCapability(meta: {
  name: string
  description: string
  is_dangerous: boolean
  category_id?: number
}): Promise<CapabilityItem> {
  const resp = await apiClient.post<CapabilityItem>('/capabilities', meta)
  return resp.data
}

export async function updateCapability(
  name: string,
  meta: {
    description?: string
    is_dangerous?: boolean
    category_id?: number | null
  }
): Promise<CapabilityItem> {
  const resp = await apiClient.put<CapabilityItem>(`/capabilities/${name}`, meta)
  return resp.data
}

export async function deleteCapability(name: string): Promise<void> {
  await apiClient.delete(`/capabilities/${name}`)
}

// Pool stats — T162
export interface PoolMetrics {
  in_use: number
  idle: number
  created_total: number
  cache_misses: number
  wait_count: number
  reset_failures: number
}

export interface PerPluginMetrics {
  plugin_id: number
  identifier: string
  version: string
  in_use: number
  idle: number
  cache_misses: number
}

export interface AuditMetricsSnapshot {
  enqueued: number
  persisted: number
  tracing_only: number
  dropped_queue_full: number
  dropped_writer_closed: number
  dropped_no_writer: number
  persist_failures: number
}

export interface PoolStats {
  global: PoolMetrics
  per_plugin: PerPluginMetrics[]
  audit: AuditMetricsSnapshot
}

export async function getPoolStats(): Promise<PoolStats> {
  const resp = await apiClient.get<PoolStats>('/runtime/pool/stats')
  return resp.data
}
