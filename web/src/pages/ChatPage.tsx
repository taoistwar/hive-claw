// ChatPage — 会话列表 + 当前对话 (T131)

import { useCallback, useEffect, useRef, useState } from 'react';
import { Button, Input, List, Space, Typography, message } from 'antd';
import { ChatStream } from '../components/ChatStream';
import {
  createSession,
  deleteSession,
  getMessages,
  listSessions,
  sendMessageStream,
  type ChatMessage,
  type ChatSession,
  type SseEvent,
} from '../services/chat';

const { Title, Text } = Typography;

export default function ChatPage() {
  const [sessions, setSessions] = useState<ChatSession[]>([]);
  const [active, setActive] = useState<ChatSession | null>(null);
  const [history, setHistory] = useState<ChatMessage[]>([]);
  const [events, setEvents] = useState<SseEvent[]>([]);
  const [tokenBuf, setTokenBuf] = useState('');
  const [pending, setPending] = useState(false);
  const [draft, setDraft] = useState('');
  const abortRef = useRef<AbortController | null>(null);

  const refreshSessions = useCallback(async () => {
    try {
      const list = await listSessions();
      setSessions(list.items);
    } catch (e) {
      void message.error(`加载失败：${(e as Error).message}`);
    }
  }, []);

  useEffect(() => {
    void refreshSessions();
  }, [refreshSessions]);

  const onSelectSession = async (s: ChatSession) => {
    setActive(s);
    setEvents([]);
    setTokenBuf('');
    try {
      const msgs = await getMessages(s.id);
      setHistory(msgs);
    } catch (e) {
      void message.error(`历史加载失败：${(e as Error).message}`);
    }
  };

  const onNewSession = async () => {
    try {
      const s = await createSession(`会话 ${new Date().toLocaleString('zh-CN')}`);
      await refreshSessions();
      await onSelectSession(s);
    } catch (e) {
      void message.error(`创建失败：${(e as Error).message}`);
    }
  };

  const onDeleteSession = async (s: ChatSession) => {
    try {
      await deleteSession(s.id);
      void message.success('已删除');
      if (active?.id === s.id) {
        setActive(null);
        setHistory([]);
        setEvents([]);
        setTokenBuf('');
      }
      await refreshSessions();
    } catch (e) {
      void message.error(`删除失败：${(e as Error).message}`);
    }
  };

  const onSend = async () => {
    if (!active || !draft.trim()) return;
    const text = draft;
    setDraft('');
    setEvents([]);
    setTokenBuf('');
    setPending(true);
    abortRef.current = new AbortController();

    // 把 user 消息加到 history（乐观更新）
    setHistory((h) => [
      ...h,
      {
        id: -Date.now(),
        session_id: active.id,
        seq: h.length + 1,
        role: 'user',
        content: text,
        tool_calls: null,
        routed_to_agent_id: null,
        elapsed_ms: null,
        created_at: new Date().toISOString(),
      },
    ]);

    try {
      await sendMessageStream(
        active.id,
        text,
        (e) => {
          setEvents((evs) => [...evs, e]);
          if (e.type === 'token') {
            setTokenBuf((s) => s + e.text);
          }
          if (e.type === 'done' || e.type === 'error') {
            setPending(false);
          }
        },
        abortRef.current.signal,
      );
    } catch (e) {
      void message.error(`流错误：${(e as Error).message}`);
      setPending(false);
    } finally {
      // 流结束后用最新 history（含 server 写入的 assistant）替换乐观更新
      try {
        const msgs = await getMessages(active.id);
        setHistory(msgs);
      } catch {
        // ignore
      }
    }
  };

  return (
    <div style={{ display: 'flex', gap: 16, height: 'calc(100vh - 200px)' }}>
      <div style={{ width: 260, borderRight: '1px solid #eee', paddingRight: 12 }}>
        <Space style={{ marginBottom: 12, width: '100%', justifyContent: 'space-between' }}>
          <Title level={5} style={{ margin: 0 }}>
            会话
          </Title>
          <Button type="primary" size="small" onClick={onNewSession}>
            新建
          </Button>
        </Space>
        <List
          dataSource={sessions}
          renderItem={(s) => (
            <List.Item
              onClick={() => void onSelectSession(s)}
              style={{
                cursor: 'pointer',
                padding: 8,
                background: active?.id === s.id ? '#f0f5ff' : 'transparent',
                borderRadius: 4,
              }}
              actions={[
                <Button
                  key="del"
                  type="link"
                  danger
                  size="small"
                  onClick={(e) => {
                    e.stopPropagation();
                    void onDeleteSession(s);
                  }}
                >
                  删除
                </Button>,
              ]}
            >
              <List.Item.Meta
                title={s.title ?? `Session #${s.id}`}
                description={s.updated_at?.slice(0, 19)}
              />
            </List.Item>
          )}
        />
      </div>
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column' }}>
        {active ? (
          <>
            <div style={{ flex: 1, overflowY: 'auto', padding: 8 }}>
              {history.map((m) => (
                <div key={m.id} style={{ marginBottom: 12 }}>
                  <Text strong>{m.role === 'user' ? '👤 我' : '🤖 助手'}</Text>
                  <pre style={{ margin: '4px 0 0', whiteSpace: 'pre-wrap' }}>{m.content}</pre>
                </div>
              ))}
              <ChatStream events={events} tokenBuffer={tokenBuf} pending={pending} />
            </div>
            <div style={{ display: 'flex', gap: 8, marginTop: 12 }}>
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
              />
              <Button type="primary" onClick={() => void onSend()} disabled={pending || !draft.trim()}>
                发送
              </Button>
            </div>
          </>
        ) : (
          <Text type="secondary">从左侧选一个会话或新建</Text>
        )}
      </div>
    </div>
  );
}
