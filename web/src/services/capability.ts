// 004 US4 — Capability service (T106)
import apiClient from './api';

export interface CapabilityItem {
  name: string;
  description: string;
  is_dangerous: boolean;
}

export interface CapabilityDetail extends CapabilityItem {
  allowed_plugins?: string[];
}

export async function listCapabilities(): Promise<CapabilityItem[]> {
  const resp = await apiClient.get<CapabilityItem[]>('/capabilities');
  return resp.data;
}

export async function getCapabilityDetail(name: string): Promise<CapabilityDetail> {
  const resp = await apiClient.get<CapabilityDetail>(`/capabilities/${name}`);
  return resp.data;
}

// Pool stats — T162
export interface PoolMetrics {
  in_use: number;
  idle: number;
  created_total: number;
  cache_misses: number;
  wait_count: number;
  reset_failures: number;
}

export interface PerPluginMetrics {
  plugin_id: number;
  identifier: string;
  version: string;
  in_use: number;
  idle: number;
  cache_misses: number;
}

export interface PoolStats {
  global: PoolMetrics;
  per_plugin: PerPluginMetrics[];
}

export async function getPoolStats(): Promise<PoolStats> {
  const resp = await apiClient.get<PoolStats>('/runtime/pool/stats');
  return resp.data;
}
