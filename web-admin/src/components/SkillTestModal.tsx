import { useState } from 'react';
import { Modal, Input, Button, Spin, Typography, Collapse, Tag, message } from 'antd';
import { CaretRightOutlined, LoadingOutlined } from '@ant-design/icons';
import type { SkillTestResult, SkillTestCall } from '../services/skill';
import { testSkill } from '../services/skill';

const { TextArea } = Input;
const { Panel } = Collapse;
const { Text, Paragraph } = Typography;

interface Props {
  visible: boolean;
  skillId: number | null;
  skillName: string;
  onCancel: () => void;
}

const SkillTestModal: React.FC<Props> = ({ visible, skillId, skillName, onCancel }) => {
  const [inputMessage, setInputMessage] = useState('');
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<SkillTestResult | null>(null);

  const handleTest = async () => {
    if (!skillId) return;
    if (!inputMessage.trim()) {
      message.warning('请输入测试消息');
      return;
    }
    setLoading(true);
    setResult(null);
    try {
      const res = await testSkill(skillId, { message: inputMessage.trim() });
      setResult(res);
    } catch (e: unknown) {
      const err = e as { response?: { data?: { message?: string } } };
      message.error(err.response?.data?.message || '测试失败');
    } finally {
      setLoading(false);
    }
  };

  const handleReset = () => {
    setInputMessage('');
    setResult(null);
  };

  const handleClose = () => {
    handleReset();
    onCancel();
  };

  const renderArgs = (args: unknown) => {
    const str = typeof args === 'string' ? args : JSON.stringify(args, null, 2);
    return <pre style={{ margin: 0, fontSize: 12, maxHeight: 200, overflow: 'auto' }}>{str}</pre>;
  };

  const renderContent = (content: unknown) => {
    const str = typeof content === 'string' ? content : JSON.stringify(content, null, 2);
    return <pre style={{ margin: 0, fontSize: 12, maxHeight: 200, overflow: 'auto' }}>{str}</pre>;
  };

  const renderToolCalls = (calls: SkillTestCall[]) => {
    if (!calls.length) return null;
    return (
      <Collapse style={{ marginTop: 12 }} accordion>
        {calls.map((call, idx) => (
          <Panel
            key={idx}
            header={
              <span>
                <Text strong>{call.tool_name}</Text>
                <Tag color={call.result.success ? 'green' : 'red'} style={{ marginLeft: 8 }}>
                  {call.result.success ? '成功' : '失败'}
                </Tag>
              </span>
            }
          >
            <div style={{ marginBottom: 8 }}>
              <Text strong>参数：</Text>
              {renderArgs(call.arguments)}
            </div>
            <div>
              <Text strong>结果：</Text>
              {renderContent(call.result.content)}
            </div>
            {call.result.error && (
              <div style={{ marginTop: 8 }}>
                <Text type="danger">错误：{call.result.error}</Text>
              </div>
            )}
          </Panel>
        ))}
      </Collapse>
    );
  };

  return (
    <Modal
      title={`测试 Skill: ${skillName}`}
      open={visible}
      onCancel={handleClose}
      footer={null}
      width={700}
      destroyOnHidden
    >
      <div style={{ marginBottom: 16 }}>
        <Text strong>输入测试消息：</Text>
        <TextArea
          rows={3}
          value={inputMessage}
          onChange={(e) => setInputMessage(e.target.value)}
          placeholder="输入消息，观察 Skill 指令如何引导 LLM 调用工具..."
          style={{ marginTop: 8 }}
          onPressEnter={(e) => {
            if (e.ctrlKey) handleTest();
          }}
        />
        <div style={{ marginTop: 8, display: 'flex', gap: 8 }}>
          <Button
            type="primary"
            icon={loading ? <LoadingOutlined /> : <CaretRightOutlined />}
            onClick={handleTest}
            loading={loading}
          >
            {loading ? '测试中...' : '测试'}
          </Button>
          <Button onClick={handleReset}>重置</Button>
          <Text type="secondary" style={{ fontSize: 12, lineHeight: '32px' }}>
            Ctrl+Enter 快捷发送
          </Text>
        </div>
      </div>

      {loading && (
        <div style={{ textAlign: 'center', padding: 32 }}>
          <Spin size="large" />
          <div style={{ marginTop: 12 }}>正在调用 LLM 执行测试...</div>
        </div>
      )}

      {!loading && result && (
        <div>
          <Paragraph>
            <Text strong>LLM 回复：</Text>
          </Paragraph>
          <div
            style={{
              padding: 12,
              background: '#f5f5f5',
              borderRadius: 4,
              maxHeight: 150,
              overflow: 'auto',
            }}
          >
            {result.assistant_content || <Text type="secondary">（无内容回复）</Text>}
          </div>

          <div style={{ marginTop: 12 }}>
            <Text strong>工具调用：</Text>
            {result.has_tool_calls ? (
              <Tag color="blue" style={{ marginLeft: 8 }}>
                {result.tool_calls.length} 次调用
              </Tag>
            ) : (
              <Tag color="orange" style={{ marginLeft: 8 }}>
                未调用工具
              </Tag>
            )}
          </div>

          {renderToolCalls(result.tool_calls)}
        </div>
      )}
    </Modal>
  );
};

export default SkillTestModal;
