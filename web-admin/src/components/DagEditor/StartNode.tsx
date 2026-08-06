import { Handle, Position, type NodeProps } from 'reactflow';

interface StartNodeData {
  node_key: string;
  input_schema?: Record<string, unknown> | null;
  start_description?: string | null;
  execution_result?: unknown;
}

function emitViewResult(nodeKey: string, e: React.MouseEvent) {
  e.stopPropagation();
  window.dispatchEvent(new CustomEvent('node-view-result', { detail: { nodeKey } }));
}

export function StartNode({ data, selected }: NodeProps<StartNodeData>) {
  const varCount = data.input_schema?.properties
    ? Object.keys(data.input_schema.properties).length
    : 0;
  const hasResult = data.execution_result !== undefined;

  return (
    <div
      style={{
        padding: '14px 18px',
        border: selected
          ? '2px solid #722ed1'
          : hasResult
            ? '2px solid #52c41a'
            : '1px solid #d3adf7',
        borderRadius: 12,
        background: hasResult ? 'linear-gradient(135deg, #f6ffed 0%, #fff 100%)' : 'linear-gradient(135deg, #f9f0ff 0%, #fff 100%)',
        minWidth: 160,
        boxShadow: selected ? '0 2px 12px rgba(114, 46, 209, 0.25)' : '0 1px 4px rgba(114, 46, 209, 0.1)',
        cursor: 'pointer',
        position: 'relative',
      }}
    >
      <Handle type="source" position={Position.Bottom} style={{ background: '#722ed1', width: 12, height: 12 }} />
      <div style={{
        position: 'absolute',
        bottom: -4,
        left: '62%',
        transform: 'translateX(-50%)',
        fontSize: 8,
        color: '#722ed1',
        background: '#f9f0ff',
        padding: '0 4px',
        borderRadius: 3,
        pointerEvents: 'none',
        whiteSpace: 'nowrap',
      }}>
        输出
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
          <div
            style={{
              width: 28,
              height: 28,
              borderRadius: '50%',
              background: 'linear-gradient(135deg, #722ed1, #b37feb)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              color: '#fff',
              fontSize: 14,
            }}
          >
            ▶
          </div>
          <span style={{ fontSize: 14, fontWeight: 600, color: '#391085' }}>
            开始
          </span>
          {hasResult && (
            <span
              onClick={(e) => emitViewResult(data.node_key, e)}
              style={{
                fontSize: 11,
                color: '#1890ff',
                cursor: 'pointer',
                marginLeft: 'auto',
                textDecoration: 'underline',
              }}
            >
              查看结果
            </span>
          )}
        </div>
        <div style={{ fontSize: 11, color: '#9254de', background: '#f0e6ff', borderRadius: 4, padding: '2px 6px', alignSelf: 'flex-start' }}>
          {varCount} 个输入变量
        </div>
        {data.start_description && (
          <div
            title={data.start_description}
            style={{
              maxWidth: 220,
              overflow: 'hidden',
              color: '#8c8c8c',
              fontSize: 11,
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            {data.start_description}
          </div>
        )}
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
    </div>
  );
}
