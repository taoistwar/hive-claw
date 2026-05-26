// ChatStream — 6 SSE 事件渲染 (T130)

import { Alert, Card, Tag, Typography } from 'antd';
import type { SseEvent } from '../services/chat';

const { Text } = Typography;

export interface ChatStreamProps {
  events: SseEvent[];
  /** 流式累计 token text（来自 events 中的 token 累加） */
  tokenBuffer: string;
  pending: boolean;
}

export function ChatStream({ events, tokenBuffer, pending }: ChatStreamProps) {
  return (
    <div role="log" aria-live="polite" aria-label="对话流">
      {events.map((e, i) => {
        switch (e.type) {
          case 'tool_call':
            return (
              <Card
                key={i}
                size="small"
                style={{ marginBottom: 8, borderLeft: '3px solid #1677ff' }}
              >
                <Tag color="blue">tool_call</Tag>
                <Text strong>{e.name}</Text>
                <pre style={{ margin: '4px 0 0', fontSize: 12 }}>
                  {JSON.stringify(e.args, null, 2)}
                </pre>
              </Card>
            );
          case 'tool_result':
            return (
              <Card
                key={i}
                size="small"
                style={{ marginBottom: 8, borderLeft: '3px solid #52c41a' }}
              >
                <Tag color="green">tool_result</Tag>
                <pre style={{ margin: '4px 0 0', fontSize: 12 }}>
                  {JSON.stringify(e.result, null, 2)}
                </pre>
              </Card>
            );
          case 'routed':
            return (
              <Alert
                key={i}
                type="info"
                showIcon
                message={`已切换到 Agent「${e.agent_identifier}」`}
                style={{ margin: '8px 0' }}
              />
            );
          case 'fallback_used':
            return (
              <Alert
                key={i}
                type="warning"
                showIcon
                message={`已切换备用模型 ${e.from} → ${e.to} (${e.reason})`}
                style={{ margin: '8px 0' }}
              />
            );
          case 'error':
            return (
              <Alert
                key={i}
                type="error"
                showIcon
                message={`错误 ${e.code}: ${e.message}`}
                style={{ margin: '8px 0' }}
              />
            );
          case 'done':
            return (
              <Text key={i} type="secondary" style={{ fontSize: 12 }}>
                完成（{e.elapsed_ms} ms{e.final_agent_id ? ` / agent=${e.final_agent_id}` : ''}）
              </Text>
            );
          default:
            return null;
        }
      })}
      {tokenBuffer && (
        <div style={{ padding: 12, background: '#f5f5f5', borderRadius: 6, whiteSpace: 'pre-wrap' }}>
          {tokenBuffer}
          {pending ? <span style={{ color: '#888' }}>▍</span> : null}
        </div>
      )}
      {pending && !tokenBuffer && (
        <Text type="secondary" style={{ fontStyle: 'italic' }}>
          正在思考…
        </Text>
      )}
    </div>
  );
}
