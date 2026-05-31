import { Handle, Position, type NodeProps } from 'reactflow';

interface EndNodeData {
  node_key: string;
  output_schema?: Record<string, unknown> | null;
  execution_result?: unknown;
}

function emitViewResult(nodeKey: string, e: React.MouseEvent) {
  e.stopPropagation();
  window.dispatchEvent(new CustomEvent('node-view-result', { detail: { nodeKey } }));
}

export function EndNode({ data, selected }: NodeProps<EndNodeData>) {
  const varCount = data.output_schema?.properties
    ? Object.keys(data.output_schema.properties).length
    : 0;
  const hasResult = data.execution_result !== undefined;

  return (
    <div
      style={{
        padding: '14px 18px',
        border: selected
          ? '2px solid #fa541c'
          : hasResult
            ? '2px solid #52c41a'
            : '1px solid #ffbb96',
        borderRadius: 12,
        background: hasResult ? 'linear-gradient(135deg, #f6ffed 0%, #fff 100%)' : 'linear-gradient(135deg, #fff2e8 0%, #fff 100%)',
        minWidth: 160,
        boxShadow: selected ? '0 2px 12px rgba(250, 84, 28, 0.25)' : '0 1px 4px rgba(250, 84, 28, 0.1)',
        cursor: 'pointer',
        position: 'relative',
      }}
    >
      <Handle type="target" position={Position.Top} style={{ background: '#fa541c', width: 12, height: 12 }} />
      <div style={{
        position: 'absolute',
        top: -4,
        right: '38%',
        transform: 'translateX(50%)',
        fontSize: 8,
        color: '#fa541c',
        background: '#fff2e8',
        padding: '0 4px',
        borderRadius: 3,
        pointerEvents: 'none',
        whiteSpace: 'nowrap',
      }}>
        输入
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
          <div
            style={{
              width: 28,
              height: 28,
              borderRadius: '50%',
              background: 'linear-gradient(135deg, #fa541c, #ffa940)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              color: '#fff',
              fontSize: 14,
            }}
          >
            ■
          </div>
          <span style={{ fontSize: 14, fontWeight: 600, color: '#873800' }}>
            结束
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
        <div style={{ fontSize: 11, color: '#d4380d', background: '#fff1e6', borderRadius: 4, padding: '2px 6px', alignSelf: 'flex-start' }}>
          {varCount} 个输出变量
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
    </div>
  );
}
