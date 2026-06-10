import { useCallback, useEffect, useRef, useState } from 'react';
import { Button, Input, Typography, App, Spin } from 'antd';
import {
  SendOutlined,
  ReloadOutlined,
  UserOutlined,
} from '@ant-design/icons';
import {
  sendMessage,
  getMessages,
  fetchTopRecommendedGames,
  executeRecommendation,
  type ChatMessage,
  type TopRecommendedGame,
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
  const [userId, setUserId] = useState<number>(() => {
    const saved = localStorage.getItem('chat_user_id');
    return saved ? parseInt(saved, 10) || 0 : 0;
  });
  const [inputUserId, setInputUserId] = useState<string>(() => {
    return localStorage.getItem('chat_user_id') || '';
  });
  const [channel, setChannel] = useState('web');
  const [clientType, setClientType] = useState('web');
  const [clientVersion, setClientVersion] = useState('1.0.0');
  const [history, setHistory] = useState<ChatMessage[]>([]);
  const [draft, setDraft] = useState('');
  const [pending, setPending] = useState(false);
  const [loadingHistory, setLoadingHistory] = useState(false);
  const [games, setGames] = useState<TopRecommendedGame[]>([]);
  const [loadingGames, setLoadingGames] = useState(false);
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

  // userId 持久化到 localStorage
  useEffect(() => {
    if (userId > 0) {
      localStorage.setItem('chat_user_id', String(userId));
    } else {
      localStorage.removeItem('chat_user_id');
    }
  }, [userId]);

  // 加载热门推荐游戏
  const loadGames = useCallback(async () => {
    if (userId <= 0) return;
    setLoadingGames(true);
    try {
      const data = await fetchTopRecommendedGames({
        user_id: userId,
        channel,
        client_type: clientType,
        client_version: clientVersion,
      });
      setGames(data);
    } catch (_e) {
      // 推荐游戏加载失败不影响聊天功能
      setGames([]);
    } finally {
      setLoadingGames(false);
    }
  }, [userId, channel, clientType, clientVersion]);

  // 参数变化时加载推荐游戏
  useEffect(() => {
    void loadGames();
  }, [loadGames]);

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
      const saved = await sendMessage({
        user_id: userId,
        message: text,
        channel,
        client_type: clientType,
        client_version: clientVersion,
      });

      // 后端返回完整持久化的 Assistant ChatMessage 记录
      // (id、session_id、role=assistant、content、elapsed_ms、extensions、created_at)
      // 追加到历史中；用户消息保持不变
      setHistory((h) => [...h, saved]);
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
        height: 'calc(100vh - 160px)',
        maxWidth: '1200px',
        margin: '0 auto',
        gap: '20px',
        padding: '8px',
        background: 'var(--bg-elevated)',
        borderRadius: 'var(--radius-lg)',
        boxShadow: '0 4px 24px rgba(0,0,0,0.06)',
      }}
    >
      {/* ── 左侧：热门推荐游戏 ── */}
      <div
        style={{
          width: '260px',
          flexShrink: 0,
          display: 'flex',
          flexDirection: 'column',
          background: 'var(--bg-card-alt)',
          border: '1px solid var(--border-default)',
          borderRadius: 'var(--radius-md)',
          boxShadow: 'var(--shadow-sm)',
          overflow: 'hidden',
        }}
      >
        <div
          style={{
            padding: '14px 16px',
            borderBottom: '2px solid var(--border-default)',
            background: 'linear-gradient(180deg, var(--bg-card-alt) 0%, var(--bg-secondary) 100%)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            gap: '8px',
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
            <div
              style={{
                width: '28px',
                height: '28px',
                borderRadius: '6px',
                background: 'linear-gradient(135deg, #f97316 0%, #ef4444 100%)',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                fontSize: '14px',
                boxShadow: '0 2px 6px rgba(249, 115, 22, 0.3)',
              }}
            >
              🔥
            </div>
            <Text strong style={{ color: 'var(--text-primary)', fontSize: '14px', letterSpacing: '0.02em' }}>
              热门推荐
            </Text>
          </div>
          <Button
            type="text"
            size="small"
            icon={<ReloadOutlined />}
            onClick={() => void loadGames()}
            loading={loadingGames}
            disabled={userId <= 0}
            style={{
              color: 'var(--text-muted)',
              fontSize: '12px',
              width: '28px',
              height: '28px',
              padding: 0,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
            }}
          />
        </div>
        <div
          style={{
            flex: 1,
            overflowY: 'auto',
            padding: '12px',
            background: 'linear-gradient(180deg, var(--bg-secondary) 0%, var(--bg-card-alt) 100%)',
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
                gap: '12px',
                opacity: 0.5,
              }}
            >
              <div style={{ fontSize: '36px', filter: 'grayscale(1)' }}>🎮</div>
              <Text style={{ color: 'var(--text-muted)', fontSize: '13px', textAlign: 'center' }}>
                输入 User ID 后<br/>加载推荐游戏
              </Text>
            </div>
          ) : loadingGames ? (
            <div style={{ display: 'flex', justifyContent: 'center', padding: '32px' }}>
              <Spin size="small" />
            </div>
          ) : games.length === 0 ? (
            <div
              style={{
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'center',
                justifyContent: 'center',
                height: '100%',
                gap: '12px',
                opacity: 0.5,
              }}
            >
              <div style={{ fontSize: '36px', filter: 'grayscale(1)' }}>📭</div>
              <Text style={{ color: 'var(--text-muted)', fontSize: '13px', textAlign: 'center' }}>
                暂无推荐游戏
              </Text>
            </div>
          ) : (
            games.map((game) => (
              <div
                key={game.game_id}
                onClick={async () => {
                  if (pending) return;
                  setPending(true);
                  try {
                    const assistantMsg = await executeRecommendation({
                      user_id: userId,
                      game_id: game.game_id,
                      channel,
                      client_type: clientType,
                      client_version: clientVersion,
                    });
                    // 构造用户消息（后端已持久化，前端用临时 id 展示）
                    const userMsg: ChatMessage = {
                      id: -Date.now(),
                      session_id: assistantMsg.session_id,
                      user_id: userId,
                      role: 'user',
                      content: game.reply,
                      elapsed_ms: null,
                      created_at: new Date().toISOString(),
                    };
                    setHistory((h) => [...h, userMsg, assistantMsg]);
                    scrollToBottom();
                  } catch (e) {
                    void appMessage.error(`推荐执行失败：${(e as Error).message}`);
                  } finally {
                    setPending(false);
                  }
                }}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: '10px',
                  padding: '10px 12px',
                  marginBottom: '6px',
                  background: 'var(--bg-card)',
                  borderRadius: 'var(--radius-sm)',
                  border: '1px solid var(--border-subtle)',
                  cursor: 'pointer',
                  transition: 'all 0.2s ease',
                }}
                onMouseEnter={(e) => {
                  (e.currentTarget as HTMLDivElement).style.borderColor =
                    'var(--accent-primary)';
                  (e.currentTarget as HTMLDivElement).style.background =
                    'var(--bg-elevated)';
                }}
                onMouseLeave={(e) => {
                  (e.currentTarget as HTMLDivElement).style.borderColor =
                    'var(--border-subtle)';
                  (e.currentTarget as HTMLDivElement).style.background =
                    'var(--bg-card)';
                }}
              >
                {game.tag && (
                  <span
                    style={{
                      flexShrink: 0,
                      background:
                        game.tag === '运营推荐'
                          ? '#eff6ff'
                          : game.tag === '新游上线'
                            ? '#ecfdf5'
                            : game.tag === '本周热玩'
                              ? '#fff7ed'
                              : '#f9fafb',
                      color:
                        game.tag === '运营推荐'
                          ? '#3b82f6'
                          : game.tag === '新游上线'
                            ? '#10b981'
                            : game.tag === '本周热玩'
                              ? '#f97316'
                              : '#6b7280',
                      padding: '2px 8px',
                      borderRadius: '4px',
                      fontSize: '11px',
                      fontWeight: 600,
                      whiteSpace: 'nowrap',
                    }}
                  >
                    {game.tag}
                  </span>
                )}
                <Text
                  style={{
                    fontSize: '13px',
                    color: 'var(--text-primary)',
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {game.game_name}
                </Text>
              </div>
            ))
          )}
        </div>
      </div>

      {/* ── 分隔线 ── */}
      <div
        style={{
          width: '1px',
          flexShrink: 0,
          background: 'var(--border-default)',
          borderRadius: '1px',
        }}
      />

      {/* ── 右侧：聊天面板 ── */}
      <div
        style={{
          flex: 1,
          display: 'flex',
          flexDirection: 'column',
          minWidth: 0,
          background: 'var(--bg-card)',
          border: '1px solid var(--border-default)',
          borderRadius: 'var(--radius-md)',
          boxShadow: 'var(--shadow-md)',
          overflow: 'hidden',
        }}
      >
      {/* 顶栏：参数配置 + 操作按钮 */}
      <div
        style={{
          padding: '14px 24px',
          borderBottom: '2px solid var(--border-default)',
          background: 'linear-gradient(180deg, var(--bg-card-alt) 0%, var(--bg-secondary) 100%)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: '16px',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: '12px', flex: 1, flexWrap: 'wrap' }}>
          <div
            style={{
              width: '32px',
              height: '32px',
              borderRadius: '8px',
              background: 'linear-gradient(135deg, #667eea 0%, #764ba2 100%)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              fontSize: '16px',
              boxShadow: '0 2px 8px rgba(102, 126, 234, 0.25)',
              flexShrink: 0,
            }}
          >
            💬
          </div>
          <Title level={5} style={{ margin: 0, color: 'var(--text-primary)', whiteSpace: 'nowrap' }}>
            AI 助手聊天
          </Title>
          <div
            style={{
              width: '1px',
              height: '20px',
              background: 'var(--border-default)',
              margin: '0 4px',
            }}
          />
          <Input
            prefix={<UserOutlined style={{ color: 'var(--text-muted)' }} />}
            placeholder="User ID"
            size="small"
            value={inputUserId}
            onChange={(e) => setInputUserId(e.target.value)}
            style={{
              maxWidth: '140px',
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
              } else if (e.target.value.trim() === '') {
                // 清空输入框时同步清除
                setUserId(0);
                setInputUserId('');
              }
            }}
          />
          <Input
            placeholder="Channel"
            value={channel}
            onChange={(e) => setChannel(e.target.value)}
            onBlur={(e) => setChannel(e.target.value || 'web')}
            size="small"
            style={{
              maxWidth: '88px',
              background: 'var(--bg-input)',
              border: '1px solid var(--border-subtle)',
              borderRadius: 'var(--radius-sm)',
              color: 'var(--text-primary)',
            }}
          />
          <Input
            placeholder="Client Type"
            value={clientType}
            onChange={(e) => setClientType(e.target.value)}
            onBlur={(e) => setClientType(e.target.value || 'web')}
            size="small"
            style={{
              maxWidth: '108px',
              background: 'var(--bg-input)',
              border: '1px solid var(--border-subtle)',
              borderRadius: 'var(--radius-sm)',
              color: 'var(--text-primary)',
            }}
          />
          <Input
            placeholder="Version"
            value={clientVersion}
            onChange={(e) => setClientVersion(e.target.value)}
            onBlur={(e) => setClientVersion(e.target.value || '1.0.0')}
            size="small"
            style={{
              maxWidth: '100px',
              background: 'var(--bg-input)',
              border: '1px solid var(--border-subtle)',
              borderRadius: 'var(--radius-sm)',
              color: 'var(--text-primary)',
            }}
          />
        </div>
        <Button
          size="small"
          icon={<ReloadOutlined />}
          onClick={() => void loadHistory()}
          loading={loadingHistory}
          disabled={userId <= 0}
          style={{
            borderRadius: 'var(--radius-sm)',
            flexShrink: 0,
          }}
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
          background: 'var(--bg-page)',
          borderTop: '1px solid var(--border-subtle)',
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
                width: '72px',
                height: '72px',
                borderRadius: '16px',
                background: 'linear-gradient(135deg, #e8ecf4 0%, #d5dbe8 100%)',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                fontSize: '32px',
                opacity: 0.6,
              }}
            >
              💬
            </div>
            <Title level={4} style={{ margin: 0, color: 'var(--text-muted)', fontWeight: 400 }}>
              欢迎使用 AI 助手
            </Title>
            <Text style={{ color: 'var(--text-muted)' }}>
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
                width: '72px',
                height: '72px',
                borderRadius: '16px',
                background: 'linear-gradient(135deg, #667eea 0%, #764ba2 100%)',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                fontSize: '32px',
                boxShadow: '0 4px 16px rgba(102, 126, 234, 0.3)',
              }}
            >
              🤖
            </div>
            <Title level={4} style={{ margin: 0, color: 'var(--text-primary)' }}>
              开始对话
            </Title>
            <Text style={{ color: 'var(--text-muted)' }}>
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
                    width: '34px',
                    height: '34px',
                    borderRadius: '10px',
                    background:
                      m.role === 'user'
                        ? 'linear-gradient(135deg, #667eea 0%, #764ba2 100%)'
                        : 'linear-gradient(135deg, #10b981 0%, #059669 100%)',
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    marginRight: m.role === 'user' ? '0' : '12px',
                    marginLeft: m.role === 'user' ? '12px' : '0',
                    order: m.role === 'user' ? '1' : '0',
                    flexShrink: 0,
                    fontSize: '15px',
                    boxShadow:
                      m.role === 'user'
                        ? '0 2px 8px rgba(102, 126, 234, 0.3)'
                        : '0 2px 8px rgba(16, 185, 129, 0.3)',
                  }}
                >
                  {m.role === 'user' ? '👤' : '🤖'}
                </div>
                <div
                  style={{
                    maxWidth: '70%',
                    padding: '12px 18px',
                    borderRadius:
                      m.role === 'user' ? '16px 16px 4px 16px' : '16px 16px 16px 4px',
                    background:
                      m.role === 'user'
                        ? 'linear-gradient(135deg, #667eea 0%, #764ba2 100%)'
                        : 'var(--bg-card)',
                    border: m.role === 'user' ? 'none' : '1px solid var(--border-subtle)',
                    boxShadow:
                      m.role === 'user'
                        ? '0 2px 12px rgba(102, 126, 234, 0.2)'
                        : '0 1px 3px rgba(0,0,0,0.06)',
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
                  {m.role === 'assistant' && m.extensions && Array.isArray(m.extensions) && m.extensions.length > 0 ? (
                    <div style={{ marginTop: '10px', display: 'flex', flexDirection: 'column', gap: '8px' }}>
                      {(m.extensions as Array<Record<string, unknown>>).map((ext, ei) => {
                        if (ext.content_type === 'card' && ext.payload) {
                          const p = ext.payload as Record<string, unknown>;
                          const info = p.info as Record<string, unknown> | undefined;
                          if (p.type === 'game' && info) {
                            return (
                              <div
                                key={ei}
                                style={{
                                  background: 'var(--bg-elevated)',
                                  borderRadius: 'var(--radius-md)',
                                  border: '1px solid var(--border-subtle)',
                                  overflow: 'hidden',
                                  boxShadow: '0 1px 4px rgba(0,0,0,0.06)',
                                }}
                              >
                                {(info.game_image as string) && (
                                  <div
                                    style={{
                                      height: '100px',
                                      background: `url(${info.game_image}) center/cover no-repeat`,
                                    }}
                                  />
                                )}
                                <div style={{ padding: '12px 14px' }}>
                                  <div style={{ display: 'flex', alignItems: 'center', gap: '8px', marginBottom: '6px' }}>
                                    <Text strong style={{ fontSize: '14px', color: 'var(--text-primary)' }}>
                                      {info.game_name as string}
                                    </Text>
                                    {(info.tag as string) && (
                                      <span
                                        style={{
                                          background: '#eff6ff',
                                          color: '#3b82f6',
                                          padding: '1px 8px',
                                          borderRadius: '4px',
                                          fontSize: '11px',
                                          fontWeight: 600,
                                        }}
                                      >
                                        {info.tag as string}
                                      </span>
                                    )}
                                  </div>
                                  {(info.reason as string) && (
                                    <Text style={{ fontSize: '12px', color: 'var(--text-muted)', lineHeight: '1.5' }}>
                                      {info.reason as string}
                                    </Text>
                                  )}
                                </div>
                              </div>
                            );
                          }
                        }
                        // fallback: raw JSON
                        return (
                          <pre
                            key={ei}
                            style={{
                              margin: 0,
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
                            {JSON.stringify(ext, null, 2)}
                          </pre>
                        );
                      })}
                    </div>
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
                    width: '34px',
                    height: '34px',
                    borderRadius: '10px',
                    background: 'linear-gradient(135deg, #10b981 0%, #059669 100%)',
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    marginRight: '12px',
                    flexShrink: 0,
                    fontSize: '15px',
                    boxShadow: '0 2px 8px rgba(16, 185, 129, 0.3)',
                  }}
                >
                  🤖
                </div>
                <div
                  style={{
                    padding: '14px 24px',
                    borderRadius: '16px 16px 16px 4px',
                    background: 'var(--bg-card)',
                    border: '1px solid var(--border-subtle)',
                    boxShadow: '0 1px 3px rgba(0,0,0,0.06)',
                    display: 'flex',
                    alignItems: 'center',
                    gap: '10px',
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
            borderTop: '2px solid var(--border-default)',
            background: 'linear-gradient(180deg, var(--bg-secondary) 0%, var(--bg-card-alt) 100%)',
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
              borderRadius: 'var(--radius-md)',
              color: 'var(--text-primary)',
              resize: 'none',
              transition: 'all var(--transition-fast)',
              boxShadow: 'inset 0 1px 2px rgba(0,0,0,0.04)',
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
              height: '44px',
              width: '56px',
              padding: 0,
              background: 'linear-gradient(135deg, #667eea 0%, #764ba2 100%)',
              border: 'none',
              borderRadius: 'var(--radius-md)',
              boxShadow: '0 2px 12px rgba(102, 126, 234, 0.3)',
              transition: 'all var(--transition-fast)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
            }}
          />
        </div>
      )}
      </div>{/* ── end 聊天面板 ── */}
    </div>
  );
}
