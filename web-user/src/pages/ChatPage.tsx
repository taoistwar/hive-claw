import { useCallback, useEffect, useRef, useState } from 'react';
import { Button, Input, Typography, App } from 'antd';
import { SendOutlined, ReloadOutlined, UserOutlined } from '@ant-design/icons';
import {
  sendMessage,
  getMessages,
  type ChatMessage,
} from '../services/chat';
const { Title, Text } = Typography;

/** 获取当前时间字符串 (YYYY-MM-DD HH:MM:SS) */
function nowStr(): string {
  const d = new Date();
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  const hh = String(d.getHours()).padStart(2, '0');
  const mm = String(d.getMinutes()).padStart(2, '0');
  const ss = String(d.getSeconds()).padStart(2, '0');
  return `${y}-${m}-${day} ${hh}:${mm}:${ss}`;
}

export default function ChatPage() {
  const { message: appMessage } = App.useApp();
  const [userId, setUserId] = useState<number>(0);
  const [history, setHistory] = useState<ChatMessage[]>([]);
  const [draft, setDraft] = useState('');
  const [pending, setPending] = useState(false);
  const [loadingHistory, setLoadingHistory] = useState(false);
  const historyEndRef = useRef<HTMLDivElement>(null);

  const scrollToBottom = () => {
    historyEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  };

  // 加载历史消息
  const loadHistory = useCallback(async () => {
    if (userId <= 0) return;
    setLoadingHistory(true);
    try {
      const msgs = await getMessages({ user_id: userId, date: nowStr() });
      // API 返回按时间倒序，前端渲染需要正序
      setHistory(msgs.reverse());
    } catch (e) {
      void appMessage.error(`加载历史失败：${(e as Error).message}`);
    } finally {
      setLoadingHistory(false);
    }
  }, [userId]);

  // userId 变化时重置并加载历史
  const prevUserId = useRef<number>(0);
  useEffect(() => {
    if (userId <= 0 || userId === prevUserId.current) return;
    prevUserId.current = userId;
    setHistory([]);
    void loadHistory();
  }, [userId, loadHistory]);

  // 新消息到达时滚动到底部
  useEffect(() => {
    scrollToBottom();
  }, [history]);

  // 发送消息
  const onSend = async () => {
    if (userId <= 0 || !draft.trim() || pending) return;
    const text = draft.trim();
    setDraft('');
    setPending(true);

    const userMsg: ChatMessage = {
      id: -Date.now(),
      session_id: 0,
      user_id: userId,
      role: 'user',
      content: text,
      elapsed_ms: null,
      created_at: new Date().toISOString(),
    };
    setHistory((h) => [...h, userMsg]);

    try {
      const resp = await sendMessage({
        user_id: userId,
        message: text,
        channel: 'web',
        platform: 'web',
        app_version: '1.0.0',
      });

      const assistantMsg: ChatMessage = {
        id: -(Date.now() + 1),
        session_id: 0,
        user_id: userId,
        role: 'assistant',
        content: resp.reply,
        elapsed_ms: resp.elapsed_ms || null,
        extensions: resp.extensions || null,
        created_at: new Date().toISOString(),
      };
      setHistory((h) => [...h, assistantMsg]);
    } catch (e) {
      void appMessage.error(`发送失败：${(e as Error).message}`);
      setHistory((h) => h.filter((m) => m.id !== userMsg.id));
    } finally {
      setPending(false);
    }
  };

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        height: 'calc(100vh - 160px)',
        maxWidth: '900px',
        margin: '0 auto',
        background: 'var(--bg-card)',
        border: '1px solid var(--border-subtle)',
        borderRadius: 'var(--radius-md)',
        boxShadow: 'var(--shadow-sm)',
        overflow: 'hidden',
      }}
    >
      {/* 顶栏：user_id 输入 + 刷新 */}
      <div
        style={{
          padding: '12px 24px',
          borderBottom: '1px solid var(--border-subtle)',
          background: 'var(--bg-secondary)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: '16px',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: '12px', flex: 1 }}>
          <Title level={5} style={{ margin: 0, color: 'var(--text-primary)', whiteSpace: 'nowrap' }}>
            AI 助手聊天
          </Title>
          <Input
            prefix={<UserOutlined style={{ color: 'var(--text-muted)' }} />}
            placeholder="输入 User ID"
            type="number"
            style={{
              maxWidth: '180px',
              background: 'var(--bg-input)',
              border: '1px solid var(--border-subtle)',
              borderRadius: 'var(--radius-sm)',
              color: 'var(--text-primary)',
            }}
            onPressEnter={(e) => {
              const val = parseInt((e.target as HTMLInputElement).value, 10);
              if (!isNaN(val) && val > 0) {
                setUserId(val);
              }
            }}
            onBlur={(e) => {
              const val = parseInt(e.target.value, 10);
              if (!isNaN(val) && val > 0) {
                setUserId(val);
              }
            }}
          />
        </div>
        <Button
          size="small"
          icon={<ReloadOutlined />}
          onClick={() => void loadHistory()}
          loading={loadingHistory}
          disabled={userId <= 0}
          style={{ borderRadius: 'var(--radius-sm)' }}
        >
          刷新
        </Button>
      </div>

      {/* 消息区域 */}
      <div
        style={{
          flex: 1,
          overflowY: 'auto',
          padding: '24px',
          background: `linear-gradient(180deg, var(--bg-card) 0%, var(--bg-secondary) 100%)`,
        }}
      >
        {userId <= 0 ? (
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              justifyContent: 'center',
              height: '100%',
              gap: '16px',
            }}
          >
            <div
              style={{
                width: '64px',
                height: '64px',
                borderRadius: '50%',
                background: 'var(--accent-glow)',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                fontSize: '28px',
              }}
            >
              💬
            </div>
            <Text style={{ color: 'var(--text-secondary)' }}>
              请在上方输入 User ID 开始聊天
            </Text>
          </div>
        ) : history.length === 0 && !loadingHistory ? (
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              justifyContent: 'center',
              height: '100%',
              gap: '16px',
            }}
          >
            <div
              style={{
                width: '64px',
                height: '64px',
                borderRadius: '50%',
                background: 'var(--accent-glow)',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                fontSize: '28px',
                animation: 'float 3s ease-in-out infinite',
              }}
            >
              💬
            </div>
            <Title level={4} style={{ margin: 0, color: 'var(--text-primary)' }}>
              开始对话
            </Title>
            <Text style={{ color: 'var(--text-secondary)' }}>
              在下方输入消息开始与 AI 助手对话
            </Text>
          </div>
        ) : (
          <>
            {history.map((m, index) => (
              <div
                key={m.id}
                style={{
                  display: 'flex',
                  justifyContent: m.role === 'user' ? 'flex-end' : 'flex-start',
                  marginBottom: '20px',
                  animation: `fadeIn 300ms ease-out`,
                  animationDelay: `${index * 30}ms`,
                  animationFillMode: 'both',
                }}
              >
                <div
                  style={{
                    width: '32px',
                    height: '32px',
                    borderRadius: '50%',
                    background:
                      m.role === 'user'
                        ? 'var(--gradient-primary)'
                        : 'linear-gradient(135deg, #48bb78 0%, #38a169 100%)',
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    marginRight: m.role === 'user' ? '0' : '12px',
                    marginLeft: m.role === 'user' ? '12px' : '0',
                    order: m.role === 'user' ? '1' : '0',
                    flexShrink: 0,
                    fontSize: '14px',
                  }}
                >
                  {m.role === 'user' ? '👤' : '🤖'}
                </div>
                <div
                  style={{
                    maxWidth: '70%',
                    padding: '12px 16px',
                    borderRadius:
                      m.role === 'user' ? '16px 16px 4px 16px' : '16px 16px 16px 4px',
                    background:
                      m.role === 'user' ? 'var(--gradient-primary)' : 'var(--bg-input)',
                    border: m.role === 'user' ? 'none' : '1px solid var(--border-subtle)',
                    boxShadow:
                      m.role === 'user'
                        ? '0 2px 12px rgba(102, 126, 234, 0.2)'
                        : 'var(--shadow-sm)',
                  }}
                >
                  <pre
                    style={{
                      margin: 0,
                      whiteSpace: 'pre-wrap',
                      fontFamily: 'inherit',
                      fontSize: '14px',
                      lineHeight: '1.6',
                      color: m.role === 'user' ? 'white' : 'var(--text-primary)',
                      background: 'transparent',
                    }}
                  >
                    {m.content}
                  </pre>
                  {m.role === 'assistant' && m.extensions && m.extensions.length > 0 ? (
                    <pre
                      style={{
                        margin: '8px 0 0 0',
                        padding: '8px 12px',
                        background: 'rgba(0,0,0,0.06)',
                        borderRadius: '8px',
                        whiteSpace: 'pre-wrap',
                        fontFamily: 'ui-monospace, SFMono-Regular, monospace',
                        fontSize: '12px',
                        lineHeight: '1.5',
                        color: 'var(--text-primary)',
                      }}
                    >
                      {JSON.stringify(m.extensions, null, 2)}
                    </pre>
                  ) : null}
                  {m.elapsed_ms && m.role === 'assistant' ? (
                    <Text
                      style={{
                        display: 'block',
                        color: 'var(--text-muted)',
                        fontSize: '11px',
                        marginTop: '4px',
                        textAlign: 'right',
                      }}
                    >
                      {(m.elapsed_ms / 1000).toFixed(1)}s
                    </Text>
                  ) : null}
                </div>
              </div>
            ))}
            {pending && (
              <div style={{ display: 'flex', justifyContent: 'flex-start', marginBottom: '20px' }}>
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
                    padding: '16px 24px',
                    borderRadius: '16px 16px 16px 4px',
                    background: 'var(--bg-input)',
                    border: '1px solid var(--border-subtle)',
                    display: 'flex',
                    alignItems: 'center',
                    gap: '8px',
                  }}
                >
                  <span className="dot-pulse" />
                  <Text style={{ color: 'var(--text-muted)', fontSize: '13px' }}>
                    AI 正在思考...
                  </Text>
                </div>
              </div>
            )}
            <div ref={historyEndRef} />
          </>
        )}
      </div>

      {/* 输入区域 */}
      {userId > 0 && (
        <div
          style={{
            padding: '16px 24px',
            borderTop: '1px solid var(--border-subtle)',
            background: 'var(--bg-secondary)',
            display: 'flex',
            gap: '12px',
            alignItems: 'flex-end',
          }}
        >
          <Input.TextArea
            rows={2}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onPressEnter={(e) => {
              if (!e.shiftKey) {
                e.preventDefault();
                void onSend();
              }
            }}
            placeholder="输入消息（Enter 发送，Shift+Enter 换行）"
            disabled={pending}
            style={{
              background: 'var(--bg-input)',
              border: '1px solid var(--border-subtle)',
              borderRadius: 'var(--radius-sm)',
              color: 'var(--text-primary)',
              resize: 'none',
              transition: 'all var(--transition-fast)',
            }}
            autoSize={{ minRows: 2, maxRows: 6 }}
          />
          <Button
            type="primary"
            icon={<SendOutlined />}
            onClick={() => void onSend()}
            disabled={pending || !draft.trim()}
            loading={pending}
            style={{
              height: 'auto',
              padding: '12px 20px',
              background: 'var(--gradient-primary)',
              border: 'none',
              borderRadius: 'var(--radius-sm)',
              boxShadow: '0 2px 12px rgba(102, 126, 234, 0.3)',
              transition: 'all var(--transition-fast)',
            }}
          />
        </div>
      )}
    </div>
  );
}
