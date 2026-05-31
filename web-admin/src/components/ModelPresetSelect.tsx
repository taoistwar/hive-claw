// ModelPresetSelect — 下拉 + GET /agents/model-presets (T123)

import { useEffect, useState } from 'react';
import { Select, Tag, message } from 'antd';
import { listModelPresets, type ModelPreset } from '../services/agent';

export interface ModelPresetSelectProps {
  value?: string | null;
  onChange: (v: string | null) => void;
  /** 是否允许 null = 用全局默认 preset；默认 true */
  allowEmpty?: boolean;
}

export function ModelPresetSelect({
  value,
  onChange,
  allowEmpty = true,
}: ModelPresetSelectProps) {
  const [presets, setPresets] = useState<ModelPreset[]>([]);

  useEffect(() => {
    listModelPresets()
      .then(setPresets)
      .catch((e) => message.error(`Preset 加载失败：${(e as Error).message}`));
  }, []);

  const unknown = value && !presets.some((p) => p.name === value);

  return (
    <Select
      value={value ?? undefined}
      onChange={(v) => onChange(v ?? null)}
      placeholder={allowEmpty ? '使用全局默认 preset' : '请选择 preset'}
      allowClear={allowEmpty}
      style={{ width: '100%' }}
      aria-label="model preset 下拉"
      status={unknown ? 'warning' : undefined}
      options={presets.map((p) => ({
        value: p.name,
        label: (
          <span>
            {p.name}
            {p.is_default ? <Tag color="green" style={{ marginLeft: 8 }}>default</Tag> : null}
            <span style={{ color: '#888', marginLeft: 8 }}>{p.description}</span>
          </span>
        ),
      }))}
      notFoundContent={
        unknown ? <span style={{ color: '#faad14' }}>当前 preset 不在列表中（已离线）</span> : null
      }
    />
  );
}
