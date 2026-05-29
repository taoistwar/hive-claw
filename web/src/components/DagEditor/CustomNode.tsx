import { Handle, Position, type NodeProps } from 'reactflow';

interface CustomNodeData {
  node_key: string;
  function_id?: number | null;
  function_name?: string;
  execution_result?: unknown;
}

function emitViewResult(nodeKey: string, e: React.MouseEvent) {
  e.stopPropagation();
  window.dispatchEvent(new CustomEvent('node-view-result', { detail: { nodeKey } }));
}

export function CustomNode({ data, selected }: NodeProps<CustomNodeData>) {
  const hasResult = data.execution_result !== undefined;
  
  return (
    <div
      style={{
        padding: '14px 18px',
        border: selected 
          ? '2px solid #1890ff' 
          : hasResult 
            ? '2px solid #52c41a' 
            : '1px solid #91caff',
        borderRadius: 12,
        background: hasResult ? 'linear-gradient(135deg, #f6ffed 0%, #fff 100%)' : 'linear-gradient(135deg, #e6f7ff 0%, #fff 100%)',
        minWidth: 160,
        boxShadow: selected ? '0 2px 12px rgba(24, 144, 255, 0.25)' : '0 1px 4px rgba(24, 144, 255, 0.1)',
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
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
          <div
            style={{
              width: 28,
              height: 28,
              borderRadius: '50%',
              background: 'linear-gradient(135deg, #1890ff, #69b1ff)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              color: '#fff',
              fontSize: 14,
            }}
          >
            ⚙
          </div>
          <span style={{ fontSize: 14, fontWeight: 600, color: '#003eb3', wordBreak: 'break-all' }}>
            {data.function_name || '未命名函数'}
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
                flexShrink: 0,
              }}
            >
              查看结果
            </span>
          )}
        </div>
        <div style={{ fontSize: 11, color: '#69b1ff', background: '#e6f7ff', borderRadius: 4, padding: '2px 6px', alignSelf: 'flex-start' }}>
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