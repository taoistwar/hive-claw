import { Modal, Descriptions, Typography } from 'antd';
import type { ToolItem } from '../services/tool';

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

const ToolDetail: React.FC<ToolDetailProps> = ({ visible, tool, onCancel }) => {
  if (!tool) return null;

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
        {tool.function_id && (
          <Descriptions.Item label="Function ID">{tool.function_id}</Descriptions.Item>
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
