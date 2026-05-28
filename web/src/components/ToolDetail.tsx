import { Modal, Descriptions, Typography, Spin, Tag } from 'antd';
import { useState, useEffect } from 'react';
import type { ToolItem } from '../services/tool';
import { listFunctions } from '../services/function';

interface ToolDetailProps {
  visible: boolean;
  tool: ToolItem | null;
  onCancel: () => void;
}

const { Text } = Typography;

const KIND_MAP: Record<number, string> = {
  1: 'function-wrap',
  2: 'workflow-wrap',
};

const SOURCE_TAG: Record<string, { label: string; color: string }> = {
  builtin: { label: 'builtin', color: 'purple' },
  workspace: { label: 'workspace', color: 'blue' },
};

const ALWAYS_TAG = { label: 'always', color: 'red' };

const ToolDetail: React.FC<ToolDetailProps> = ({ visible, tool, onCancel }) => {
  const [functionNames, setFunctionNames] = useState<Map<number, string>>(new Map());
  const [loadingFunctions, setLoadingFunctions] = useState(false);

  useEffect(() => {
    if (!visible || !tool?.function_id) return;
    setLoadingFunctions(true);
    listFunctions({ limit: 500 })
      .then((res) => {
        const map = new Map<number, string>();
        res.items.forEach((f) => map.set(f.id, f.name || f.identifier));
        setFunctionNames(map);
      })
      .catch(() => {})
      .finally(() => setLoadingFunctions(false));
  }, [visible, tool?.function_id]);

  if (!tool) return null;

  const functionLabel = tool.function_id
    ? (() => {
        const name = functionNames.get(tool.function_id);
        return name ? `${name}(${tool.function_id})` : `${tool.function_id}`;
      })()
    : null;

  return (
    <Modal
      title="工具详情"
      open={visible}
      onCancel={onCancel}
      footer={null}
      width={800}
    >
      <Descriptions bordered column={1} size="small">
        <Descriptions.Item label="ID">{tool.id}</Descriptions.Item>
        <Descriptions.Item label="标识符">{tool.identifier}</Descriptions.Item>
        <Descriptions.Item label="名称">{tool.name}</Descriptions.Item>
        <Descriptions.Item label="描述">{tool.description}</Descriptions.Item>
        <Descriptions.Item label="类型">{KIND_MAP[tool.kind] || tool.kind}</Descriptions.Item>
        <Descriptions.Item label="来源">
          {(() => {
            const tag = SOURCE_TAG[tool.source];
            return tag ? <Tag color={tag.color}>{tag.label}</Tag> : tool.source;
          })()}
        </Descriptions.Item>
        <Descriptions.Item label="Always">
          {tool.is_always ? <Tag color={ALWAYS_TAG.color}>{ALWAYS_TAG.label}</Tag> : '-'}
        </Descriptions.Item>
        <Descriptions.Item label="Required Capabilities">
          {tool.required_capabilities && tool.required_capabilities.length > 0
            ? tool.required_capabilities.map((c) => <Tag key={c}>{c}</Tag>)
            : '-'}
        </Descriptions.Item>
        <Descriptions.Item label="标签">
          {tool.tags && tool.tags.length > 0
            ? tool.tags.map((t) => <Tag key={t.id}>{t.name}</Tag>)
            : '-'}
        </Descriptions.Item>
        {tool.function_id && (
          <Descriptions.Item label="Function">
            {loadingFunctions ? <Spin size="small" /> : functionLabel}
          </Descriptions.Item>
        )}
        {tool.workflow_id && (
          <Descriptions.Item label="Workflow ID">{tool.workflow_id}</Descriptions.Item>
        )}
        <Descriptions.Item label="输入Schema">
          <Text
            style={{
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-all',
              maxHeight: 200,
              overflow: 'auto',
              display: 'block',
              fontFamily: 'monospace',
              fontSize: 12,
            }}
          >
            {typeof tool.input_schema === 'string'
              ? tool.input_schema
              : JSON.stringify(tool.input_schema, null, 2)}
          </Text>
        </Descriptions.Item>
        <Descriptions.Item label="输出Schema">
          <Text
            style={{
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-all',
              maxHeight: 200,
              overflow: 'auto',
              display: 'block',
              fontFamily: 'monospace',
              fontSize: 12,
            }}
          >
            {typeof tool.output_schema === 'string'
              ? tool.output_schema
              : JSON.stringify(tool.output_schema, null, 2)}
          </Text>
        </Descriptions.Item>
        <Descriptions.Item label="创建时间">
          {new Date(tool.created_at).toLocaleString()}
        </Descriptions.Item>
        <Descriptions.Item label="修改时间">
          {new Date(tool.updated_at).toLocaleString()}
        </Descriptions.Item>
      </Descriptions>
    </Modal>
  );
};

export default ToolDetail;
