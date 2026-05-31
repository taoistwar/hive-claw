import { Alert, Typography } from 'antd';
import type { SseEvent } from '../services/chat';

const { Text } = Typography;

export interface ChatStreamProps {
  events: SseEvent[];
  tokenBuffer: string;
  pending: boolean;
}

export function ChatStream({ events, tokenBuffer, pending }: ChatStreamProps) {
  return (
    <div role="log" aria-live="polite" aria-label="对话流">
      {events.map((e, i) => {
        switch (e.type) {
          case 'error':
            return (
              <Alert
                key={i}
                type="error"
                showIcon
                message={e.message}
                style={{ margin: '8px 0' }}
              />
            );
          default:
            return null;
        }
      })}
      {tokenBuffer && (
        <div
          style={{
            display: 'flex',
            justifyContent: 'flex-start',
            marginBottom: '20px',
          }}
        >
          <div
            style={{
              width: '32px',
              height: '32px',
              borderRadius: '50%',
              background: 'linear-gradient(135deg, #48bb78 0%, #38a169 100%)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              marginRight: '12px',
              flexShrink: 0,
              fontSize: '14px',
            }}
          >
            🤖
          </div>
          <div
            style={{
              maxWidth: '70%',
              padding: '12px 16px',
              borderRadius: '16px 16px 16px 4px',
              background: 'var(--bg-input)',
              border: '1px solid var(--border-subtle)',
              boxShadow: 'var(--shadow-sm)',
            }}
          >
            <pre
              style={{
                margin: 0,
                whiteSpace: 'pre-wrap',
                fontFamily: 'inherit',
                fontSize: '14px',
                lineHeight: '1.6',
                color: 'var(--text-primary)',
                background: 'transparent',
              }}
            >
              {tokenBuffer}{pending && '▌'}
            </pre>
          </div>
        </div>
      )}
    </div>
  );
}
