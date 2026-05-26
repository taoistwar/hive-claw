// usePlugins — 分页 + filter 状态 (T078)

import { useCallback, useEffect, useState } from 'react';
import { listPlugins, type Plugin, type PluginListParams } from '../services/plugin';

export interface UsePluginsResult {
  items: Plugin[];
  total: number;
  loading: boolean;
  error: string | null;
  params: PluginListParams;
  setParams: (next: PluginListParams) => void;
  refresh: () => Promise<void>;
}

export function usePlugins(initial: PluginListParams = { offset: 0, limit: 20 }): UsePluginsResult {
  const [params, setParams] = useState<PluginListParams>(initial);
  const [items, setItems] = useState<Plugin[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await listPlugins(params);
      setItems(list.items);
      setTotal(list.total);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [params]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return { items, total, loading, error, params, setParams, refresh };
}
