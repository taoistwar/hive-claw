import { Handle, Position, type NodeProps } from 'reactflow';

interface CustomNodeData {
  node_key: string;
  function_id: number;
  function_name?: string;
}

export function CustomNode({ data, selected }: NodeProps<CustomNodeData>) {
  return (
    <div
      style={{
        padding: '12px 16px',
        border: selected ? '2px solid #1890ff' : '1px solid #91caff',
        borderRadius: 8,
        background: '#fff',
        minWidth: 120,
        boxShadow: selected ? '0 2px 8px rgba(24, 144, 255, 0.3)' : 'none',
      }}
    >
      <Handle type="target" position={Position.Top} style={{ background: '#91caff' }} />
      <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        <div style={{ fontSize: 14, fontWeight: 600, color: '#333', wordBreak: 'break-all' }}>
          {data.function_name || '未命名函数'}
        </div>
        <div style={{ fontSize: 12, color: '#999' }}>
          {data.node_key}
        </div>
      </div>
      <Handle type="source" position={Position.Bottom} style={{ background: '#91caff' }} />
    </div>
  );
}