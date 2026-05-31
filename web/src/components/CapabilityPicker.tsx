// CapabilityPicker — 含 dangerous 标记；非 Super 不可勾选 dangerous (T107)

import { useEffect, useState } from 'react';
import { Checkbox, Space, Tag, Tooltip, message } from 'antd';
import { listCapabilities, type CapabilityItem } from '../services/capability';

export interface CapabilityPickerProps {
  value: string[];
  onChange: (next: string[]) => void;
  /** 当前管理员 role；只有 role===3 (Super) 才能勾选 dangerous capability */
  currentRole: number;
}

export function CapabilityPicker({ value, onChange, currentRole }: CapabilityPickerProps) {
  const [caps, setCaps] = useState<CapabilityItem[]>([]);
  const [loading, setLoading] = useState(false);
  const isSuper = currentRole === 3;

  useEffect(() => {
    setLoading(true);
    listCapabilities()
      .then(([caps]) => setCaps(caps))
      .catch((e) => message.error(`Capability 加载失败：${(e as Error).message}`))
      .finally(() => setLoading(false));
  }, []);

  const toggle = (name: string, on: boolean) => {
    const next = on ? [...value, name] : value.filter((v) => v !== name);
    onChange(next);
  };

  return (
    <div role="group" aria-label="选择 Capability">
      {loading ? <p>加载中…</p> : null}
      <Space direction="vertical" style={{ width: '100%' }}>
        {caps.map((c) => {
          const checked = value.includes(c.name);
          const disabled = c.is_dangerous && !isSuper;
          const cb = (
            <Checkbox
              checked={checked}
              disabled={disabled}
              onChange={(e) => toggle(c.name, e.target.checked)}
              aria-label={c.name}
            >
              <span style={{ marginRight: 8 }}>{c.name}</span>
              {c.is_dangerous ? <Tag color="red">dangerous</Tag> : null}
              <span style={{ color: '#888', marginLeft: 8 }}>{c.description}</span>
            </Checkbox>
          );
          return disabled ? (
            <Tooltip key={c.name} title="dangerous capability 只能由 Super 角色授予">
              <span>{cb}</span>
            </Tooltip>
          ) : (
            <span key={c.name}>{cb}</span>
          );
        })}
      </Space>
    </div>
  );
}
