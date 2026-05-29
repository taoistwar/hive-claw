import { Handle, Position, type NodeProps } from 'reactflow';

interface CustomNodeData {
  node_key: string;
  function_id?: number | null;
  function_name?: string;
  execution_result?: unknown;
}

export function CustomNode({ data, selected }: NodeProps<CustomNodeData>) {
  const hasResult = data.execution_result !== undefined;
  
  return (
    <div
      style={{
        padding: '12px 16px',
        border: selected 
          ? '2px solid #1890ff' 
          : hasResult 
            ? '2px solid #52c41a' 
            : '1px solid #91caff',
        borderRadius: 8,
        background: hasResult ? '#f6ffed' : '#fff',
        minWidth: 120,
        boxShadow: selected ? '0 2px 8px rgba(24, 144, 255, 0.3)' : 'none',
        position: 'relative',
      }}
    >
      <Handle type="target" position={Position.Top} style={{ background: '#91caff', width: 12, height: 12 }} />
      <div style={{
        position: 'absolute',
        top: -4,
        right: '38%',
        transform: 'translateX(50%)',
        fontSize: 8,
        color: '#1890ff',
        background: '#e6f7ff',
        padding: '0 4px',
        borderRadius: 3,
        pointerEvents: 'none',
        whiteSpace: 'nowrap',
      }}>
        输入
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        <div style={{ fontSize: 14, fontWeight: 600, color: '#333', wordBreak: 'break-all' }}>
          {data.function_name || '未命名函数'}
        </div>
        <div style={{ fontSize: 12, color: '#999' }}>
          {data.node_key}
        </div>
        {hasResult && (
          <div style={{
            fontSize: 11,
            color: '#389e0d',
            background: '#f6ffed',
            borderRadius: 4,
            padding: '2px 6px',
            marginTop: 4,
            display: 'flex',
            alignItems: 'center',
            gap: 4,
          }}>
            ✓ 已运行
          </div>
        )}
      </div>
      <Handle type="source" position={Position.Bottom} style={{ background: '#91caff', width: 12, height: 12 }} />
      <div style={{
        position: 'absolute',
        bottom: -4,
        left: '62%',
        transform: 'translateX(-50%)',
        fontSize: 10,
        color: '#1890ff',
        background: '#e6f7ff',
        padding: '0 4px',
        borderRadius: 3,
        pointerEvents: 'none',
        whiteSpace: 'nowrap',
      }}>
        输出
      </div>
    </div>
  );
}