import { Handle, Position, type NodeProps } from 'reactflow';

interface AnswerNodeData {
  node_key: string;
  node_config?: {
    system_prompt?: string;
    model_preset?: string;
    history_window?: number;
    variables?: Array<{ name: string }>;
  } | null;
  execution_result?: unknown;
}

function emitViewResult(nodeKey: string, e: React.MouseEvent) {
  e.stopPropagation();
  window.dispatchEvent(new CustomEvent('node-view-result', { detail: { nodeKey } }));
}

export function AnswerNode({ data, selected }: NodeProps<AnswerNodeData>) {
  const varCount = data.node_config?.variables?.length ?? 0;
  const modelLabel = data.node_config?.model_preset || '默认模型';
  const promptPreview = data.node_config?.system_prompt
    ?.substring(0, 40) || '未配置提示词';
  const hasResult = data.execution_result !== undefined;

  return (
    <div
      style={{
        padding: '14px 18px',
        border: selected 
          ? '2px solid #13c2c2' 
          : hasResult 
            ? '2px solid #52c41a' 
            : '1px solid #87e8de',
        borderRadius: 12,
        background: hasResult 
          ? `linear-gradient(135deg, #f6ffed 0%, #fff 100%)` 
          : `linear-gradient(135deg, #e6fffb 0%, #fff 100%)`,
        minWidth: 160,
        boxShadow: selected ? '0 2px 12px rgba(19, 194, 194, 0.25)' : '0 1px 4px rgba(19, 194, 194, 0.1)',
        cursor: 'pointer',
        position: 'relative',
      }}
    >
      <Handle type="target" position={Position.Top} style={{ background: '#13c2c2', width: 12, height: 12 }} />
      <div style={{
        position: 'absolute',
        top: -4,
        right: '38%',
        transform: 'translateX(50%)',
        fontSize: 8,
        color: '#13c2c2',
        background: '#e6fffb',
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
              background: 'linear-gradient(135deg, #13c2c2, #36cfc9)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              color: '#fff',
              fontSize: 14,
            }}
          >
            💬
          </div>
          <span style={{ fontSize: 14, fontWeight: 600, color: '#006d75' }}>
            生成回答
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
        <div style={{ fontSize: 11, color: '#8c8c8c', maxWidth: 180, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {promptPreview}
        </div>
        <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
          <span style={{ fontSize: 11, color: '#08979c', background: '#e6fffb', borderRadius: 4, padding: '2px 6px' }}>
            {modelLabel}
          </span>
          <span style={{ fontSize: 11, color: '#08979c', background: '#e6fffb', borderRadius: 4, padding: '2px 6px' }}>
            {varCount} 个变量
          </span>
        </div>
        {hasResult && (
          <div style={{
            fontSize: 11,
            color: '#389e0d',
            background: '#f6ffed',
            borderRadius: 4,
            padding: '2px 6px',
            display: 'flex',
            alignItems: 'center',
            gap: 4,
            marginTop: 4,
          }}>
            ✓ 已运行
          </div>
        )}
      </div>
      <Handle type="source" position={Position.Bottom} style={{ background: '#13c2c2', width: 12, height: 12 }} />
      <div style={{
        position: 'absolute',
        bottom: -4,
        left: '62%',
        transform: 'translateX(-50%)',
        fontSize: 10,
        color: '#13c2c2',
        background: '#e6fffb',
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