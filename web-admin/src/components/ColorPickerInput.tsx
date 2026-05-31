import { ColorPicker, Input, Space } from 'antd';
import { useState } from 'react';
import type { Color } from 'antd/es/color-picker';

export interface ColorPickerInputProps {
  value?: string | null;
  onChange?: (value: string | null) => void;
  allowClear?: boolean;
}

export function ColorPickerInput({ value, onChange, allowClear = true }: ColorPickerInputProps) {
  const [inputValue, setInputValue] = useState(value ?? '');

  const onPickerChange = (color: Color) => {
    const hex = color.toHexString();
    setInputValue(hex);
    onChange?.(hex);
  };

  const onInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const v = e.target.value;
    setInputValue(v);
    if (/^#[0-9a-fA-F]{6}$/.test(v)) {
      onChange?.(v);
    } else if (v === '') {
      onChange?.(null);
    }
  };

  const onClear = () => {
    setInputValue('');
    onChange?.(null);
  };

  return (
    <Space.Compact style={{ width: '100%' }}>
      <ColorPicker value={value ?? undefined} onChange={onPickerChange} />
      <Input
        value={inputValue}
        onChange={onInputChange}
        placeholder="#1677ff"
        style={{ width: '100%' }}
        suffix={
          allowClear && inputValue ? (
            <a onClick={onClear} style={{ cursor: 'pointer', fontSize: 12 }}>
              ✕
            </a>
          ) : null
        }
      />
    </Space.Compact>
  );
}
