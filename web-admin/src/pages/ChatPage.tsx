// ChatPage — 会话列表 + 当前对话 (T131)

import { useCallback, useEffect, useRef, useState } from 'react';
import { Button, Input, List, Space, Typography, Pagination, message } from 'antd';
import { PlusOutlined, SendOutlined, DeleteOutlined, MessageOutlined, SearchOutlined } from '@ant-design/icons';
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

const PAGE_SIZE = 5;

export default function ChatPage() {
  const [sessions, setSessions] = useState<ChatSession[]>([]);
  const [total, setTotal] = useState(0);
  const [active, setActive] = useState<ChatSession | null>(null);
  const [history, setHistory] = useState<ChatMessage[]>([]);
  const [events, setEvents] = useState<SseEvent[]>([]);
  const [tokenBuf, setTokenBuf] = useState('');
  const [pending, setPending] = useState(false);
  const [draft, setDraft] = useState('');
  const [page, setPage] = useState(1);
  const [search, setSearch] = useState('');
  const abortRef = useRef<AbortController | null>(null);

  const doFetchSessions = useCallback(async (currentPage: number, currentSearch: string) => {
    try {
      const list = await listSessions({
        offset: (currentPage - 1) * PAGE_SIZE,
        limit: PAGE_SIZE,
        search: currentSearch || undefined,
      });
      setSessions(list.items);
      setTotal(list.total);
    } catch (e) {
      void message.error(`加载失败：${(e as Error).message}`);
    }
  }, []);

  const refreshSessions = useCallback(async () => {
    doFetchSessions(page, search);
  }, [page, search, doFetchSessions]);

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
      setPage(1);
      setSearch('');
      await doFetchSessions(1, '');
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
      try {
        const msgs = await getMessages(active.id);
        setHistory(msgs);
      } catch {
        // ignore
      }
    }
  };

  return (
    <div
      style={{
        display: 'flex',
        gap: '20px',
        height: 'calc(100vh - 200px)',
        animation: 'fadeIn 400ms ease-out',
      }}
    >
      {/* Sidebar - Session List */}
      <div
        style={{
          width: '320px',
          background: 'var(--bg-card)',
          border: '1px solid var(--border-subtle)',
          borderRadius: 'var(--radius-md)',
          padding: '16px',
          display: 'flex',
          flexDirection: 'column',
          boxShadow: 'var(--shadow-sm)',
        }}
      >
        {/* Header: Title + New Button */}
        <Space
          style={{
            marginBottom: '12px',
            width: '100%',
            justifyContent: 'space-between',
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
            <MessageOutlined style={{ color: 'var(--accent-primary)' }} />
            <Title level={5} style={{ margin: 0, color: 'var(--text-primary)' }}>
              会话
            </Title>
          </div>
          <Button
            type="primary"
            size="small"
            icon={<PlusOutlined />}
            onClick={onNewSession}
            style={{
              background: 'var(--gradient-primary)',
              border: 'none',
              borderRadius: 'var(--radius-sm)',
              boxShadow: '0 2px 8px rgba(102, 126, 234, 0.3)',
            }}
          >
            新建
          </Button>
        </Space>

        {/* Search Input */}
        <Input
          prefix={<SearchOutlined style={{ color: 'var(--text-muted)' }} />}
          placeholder="搜索会话..."
          allowClear
          value={search}
          onChange={(e) => {
            setSearch(e.target.value);
            setPage(1);
          }}
          onPressEnter={() => {
            setPage(1);
            void doFetchSessions(1, search);
          }}
          style={{
            marginBottom: '12px',
            background: 'var(--bg-input)',
            border: '1px solid var(--border-subtle)',
            borderRadius: 'var(--radius-sm)',
            color: 'var(--text-primary)',
            transition: 'all var(--transition-fast)',
          }}
        />

        {/* Session List */}
        <div style={{ flex: 1, overflowY: 'auto', marginBottom: '12px' }}>
          {sessions.length === 0 ? (
            <div
              style={{
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'center',
                justifyContent: 'center',
                padding: '32px 0',
                color: 'var(--text-muted)',
                fontSize: '13px',
              }}
            >
              <MessageOutlined style={{ fontSize: '24px', marginBottom: '8px', opacity: 0.4 }} />
              {search ? '未找到匹配的会话' : '暂无会话'}
            </div>
          ) : (
            <List
              dataSource={sessions}
              renderItem={(s) => (
                <List.Item
                  onClick={() => void onSelectSession(s)}
                  style={{
                    cursor: 'pointer',
                    padding: '12px',
                    background: active?.id === s.id ? 'var(--accent-glow)' : 'transparent',
                    border: `1px solid ${active?.id === s.id ? 'var(--border-accent)' : 'transparent'}`,
                    borderRadius: 'var(--radius-sm)',
                    marginBottom: '8px',
                    transition: 'all var(--transition-fast)',
                  }}
                  onMouseEnter={(e) => {
                    if (active?.id !== s.id) {
                      e.currentTarget.style.background = 'var(--theme-toggle-hover)';
                    }
                  }}
                  onMouseLeave={(e) => {
                    if (active?.id !== s.id) {
                      e.currentTarget.style.background = 'transparent';
                    }
                  }}
                  actions={[
                    <Button
                      key="del"
                      type="text"
                      danger
                      size="small"
                      icon={<DeleteOutlined />}
                      onClick={(e) => {
                        e.stopPropagation();
                        void onDeleteSession(s);
                      }}
                    />,
                  ]}
                >
                  <List.Item.Meta
                    title={
                      <Text
                        style={{
                          color: active?.id === s.id ? 'var(--text-primary)' : 'var(--text-secondary)',
                          fontWeight: active?.id === s.id ? 600 : 400,
                        }}
                      >
                        {s.title ?? `Session #${s.id}`}
                      </Text>
                    }
                    description={
                      <Text style={{ color: 'var(--text-muted)', fontSize: '12px' }}>
                        {s.updated_at?.slice(0, 19)}
                      </Text>
                    }
                  />
                </List.Item>
              )}
            />
          )}
        </div>

        {/* Pagination */}
        {total > PAGE_SIZE && (
          <div style={{ display: 'flex', justifyContent: 'center' }}>
            <Pagination
              size="small"
              current={page}
              pageSize={PAGE_SIZE}
              total={total}
              onChange={(p) => {
                setPage(p);
                void doFetchSessions(p, search);
              }}
              showSizeChanger={false}
              hideOnSinglePage
              style={{ fontSize: '12px' }}
            />
          </div>
        )}
      </div>

      {/* Main Chat Area */}
      <div
        style={{
          flex: 1,
          display: 'flex',
          flexDirection: 'column',
          background: 'var(--bg-card)',
          border: '1px solid var(--border-subtle)',
          borderRadius: 'var(--radius-md)',
          boxShadow: 'var(--shadow-sm)',
          overflow: 'hidden',
        }}
      >
        {active ? (
          <>
            {/* Messages Area */}
            <div
              style={{
                flex: 1,
                overflowY: 'auto',
                padding: '24px',
                background: `
                  linear-gradient(180deg, var(--bg-card) 0%, var(--bg-secondary) 100%)
                `,
              }}
            >
              {history.map((m, index) => (
                <div
                  key={m.id}
                  style={{
                    display: 'flex',
                    justifyContent: m.role === 'user' ? 'flex-end' : 'flex-start',
                    marginBottom: '20px',
                    animation: 'fadeIn 300ms ease-out',
                    animationDelay: `${index * 50}ms`,
                    animationFillMode: 'both',
                  }}
                >
                  {/* Avatar */}
                  <div
                    style={{
                      width: '32px',
                      height: '32px',
                      borderRadius: '50%',
                      background: m.role === 'user' ? 'var(--gradient-primary)' : 'linear-gradient(135deg, #48bb78 0%, #38a169 100%)',
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

                  {/* Message Bubble */}
                  <div
                    style={{
                      maxWidth: '70%',
                      padding: '12px 16px',
                      borderRadius: m.role === 'user' ? '16px 16px 4px 16px' : '16px 16px 16px 4px',
                      background: m.role === 'user' ? 'var(--gradient-primary)' : 'var(--bg-input)',
                      border: m.role === 'user' ? 'none' : '1px solid var(--border-subtle)',
                      boxShadow: m.role === 'user' ? '0 2px 12px rgba(102, 126, 234, 0.2)' : 'var(--shadow-sm)',
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
              
            </div>
                </div>
              ))}
              <ChatStream events={events} tokenBuffer={tokenBuf} pending={pending} />
            </div>

            {/* Input Area */}
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
          </>
        ) : (
          /* Empty State */
          <div
            style={{
              flex: 1,
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              justifyContent: 'center',
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
              从左侧选择一个会话或创建新会话
            </Text>
          </div>
        )}
      </div>
    </div>
  );
}
