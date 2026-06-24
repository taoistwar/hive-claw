import { useState, useMemo, useCallback, type ReactNode } from 'react';
import { Button, Input, Space, Tooltip, Typography } from 'antd';
import { CopyOutlined, CheckOutlined } from '@ant-design/icons';

const { Text } = Typography;

const COLOR = {
  key: '#003a8c',
  string: '#a31515',
  number: '#098658',
  boolean: '#0000ff',
  null: '#808080',
  bracket: '#000',
  meta: '#8c8c8c',
};

const MONO_FONT = "'SFMono-Regular', Consolas, 'Liberation Mono', Menlo, Courier, monospace";

interface CollapsibleJsonViewProps {
  /** The data to display. */
  data: unknown;
  /** Initial expanded depth (default 2). */
  initialDepth?: number;
  /** Optional title shown in the toolbar. */
  title?: string;
  /** Max height for the scroll container (default '60vh'). */
  maxHeight?: number | string;
  /** Show the toolbar (copy / expand all). */
  showToolbar?: boolean;
  /** Compact mode uses smaller font/spacing. */
  compact?: boolean;
  /** Display "null" instead of empty for null/undefined. */
  emptyText?: string;
}

export function CollapsibleJsonView(props: CollapsibleJsonViewProps) {
  const {
    data,
    initialDepth = 2,
    title,
    maxHeight = '60vh',
    showToolbar = true,
    compact = false,
    emptyText = 'null',
  } = props;

  const [query, setQuery] = useState('');
  const [expandedAll, setExpandedAll] = useState<number | null>(null);
  const [copied, setCopied] = useState(false);

  const jsonText = useMemo(() => {
    if (data === undefined || data === null) return emptyText;
    try {
      return JSON.stringify(data, null, 2);
    } catch {
      return String(data);
    }
  }, [data, emptyText]);

  const handleCopy = useCallback(() => {
    void navigator.clipboard.writeText(jsonText).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    });
  }, [jsonText]);

  const handleExpandAll = useCallback(() => {
    setExpandedAll((prev) => (prev === null ? 1 : null));
  }, []);

  return (
    <div
      style={{
        border: '1px solid #d9d9d9',
        borderRadius: 6,
        background: '#fafafa',
        overflow: 'hidden',
      }}
    >
      {showToolbar && (
        <div
          style={{
            display: 'flex',
            justifyContent: 'space-between',
            alignItems: 'center',
            padding: compact ? '4px 8px' : '6px 10px',
            borderBottom: '1px solid #e8e8e8',
            background: '#fff',
            gap: 8,
          }}
        >
          <Space size={8} style={{ flex: 1, minWidth: 0 }}>
            {title && <Text strong style={{ fontSize: compact ? 12 : 13 }}>{title}</Text>}
            <Input
              size="small"
              placeholder="搜索 key / value"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              allowClear
              style={{ maxWidth: 220 }}
            />
          </Space>
          <Space size={4}>
            <Tooltip title={expandedAll === null ? '全部展开' : '全部折叠'}>
              <Button
                size="small"
                type="text"
                onClick={handleExpandAll}
              >
                {expandedAll === null ? '展开' : '折叠'}
              </Button>
            </Tooltip>
            <Tooltip title={copied ? '已复制' : '复制 JSON'}>
              <Button
                size="small"
                type="text"
                onClick={handleCopy}
                icon={copied ? <CheckOutlined style={{ color: '#52c41a' }} /> : <CopyOutlined />}
              />
            </Tooltip>
          </Space>
        </div>
      )}
      <div
        style={{
          padding: compact ? 8 : 12,
          fontFamily: MONO_FONT,
          fontSize: compact ? 11 : 12,
          lineHeight: 1.6,
          overflow: 'auto',
          maxHeight,
        }}
      >
        {data === undefined || data === null ? (
          <Text type="secondary" style={{ fontFamily: MONO_FONT, fontSize: compact ? 11 : 12 }}>{emptyText}</Text>
        ) : (
          <JsonNode
            value={data}
            depth={0}
            initialDepth={initialDepth}
            expandedAll={expandedAll}
            query={query.trim().toLowerCase()}
          />
        )}
      </div>
    </div>
  );
}

interface JsonNodeProps {
  value: unknown;
  /** Property key in the parent object (undefined = inside an array). */
  keyName?: string;
  depth: number;
  initialDepth: number;
  expandedAll: number | null;
  query: string;
  isLast?: boolean;
}

function JsonNode(props: JsonNodeProps) {
  const { value, keyName, depth, initialDepth, expandedAll, query, isLast } = props;
  const type = getType(value);

  // 决定默认是否展开
  const isContainer = type === 'object' || type === 'array';
  const [expanded, setExpanded] = useState(depth < initialDepth);

  // expandedAll 变化时同步
  const forceState = expandedAll === 1 ? true : expandedAll === 0 ? false : null;
  const isExpanded = forceState === null ? expanded : forceState;

  // 搜索匹配
  const matchesQuery = useMemo(() => {
    if (!query) return false;
    if (keyName && keyName.toLowerCase().includes(query)) return true;
    if (type === 'string' && typeof value === 'string' && value.toLowerCase().includes(query)) return true;
    if (type === 'number' && String(value).includes(query)) return true;
    if (type === 'boolean' && String(value).includes(query)) return true;
    return false;
  }, [query, keyName, type, value]);

  const indent = depth * 14;

  // 键名渲染（只在有 keyName 时显示）
  const keyPart = keyName !== undefined ? (
    <span>
      <span style={{ color: COLOR.key }}>"{keyName}"</span>
      <span style={{ color: COLOR.meta }}>: </span>
    </span>
  ) : null;

  if (!isContainer) {
    return (
      <div
        style={{
          paddingLeft: indent,
          background: matchesQuery ? '#fff7e6' : undefined,
        }}
      >
        {keyPart}
        <ValueText value={value} type={type} />
        {!isLast && <span style={{ color: COLOR.meta }}>,</span>}
      </div>
    );
  }

  const entries = type === 'array'
    ? (value as unknown[]).map((v, i) => [String(i), v] as const)
    : Object.entries(value as Record<string, unknown>);

  const open = isContainer ? isExpanded : false;
  const bracketOpen = type === 'array' ? '[' : '{';
  const bracketClose = type === 'array' ? ']' : '}';
  const summary = type === 'array'
    ? `Array(${(value as unknown[]).length})`
    : `Object(${Object.keys(value as Record<string, unknown>).length})`;

  if (entries.length === 0) {
    return (
      <div style={{ paddingLeft: indent, background: matchesQuery ? '#fff7e6' : undefined }}>
        {keyPart}
        <span style={{ color: COLOR.bracket }}>
          {bracketOpen}
          {bracketClose}
        </span>
        {!isLast && <span style={{ color: COLOR.meta }}>,</span>}
      </div>
    );
  }

  return (
    <div style={{ background: matchesQuery ? '#fff7e6' : undefined }}>
      <div
        style={{
          paddingLeft: indent,
          cursor: 'pointer',
          userSelect: 'none',
        }}
        onClick={() => setExpanded((v) => !v)}
      >
        {keyPart}
        <ToggleIcon expanded={open} />
        <span style={{ color: COLOR.bracket }}>{bracketOpen}</span>
        {!open && (
          <span style={{ color: COLOR.meta, marginLeft: 4 }}>
            {' '}… {summary}
          </span>
        )}
      </div>
      {open && (
        <div>
          {entries.map(([k, v], i) => (
            <JsonNode
              key={k}
              keyName={k}
              value={v}
              depth={depth + 1}
              initialDepth={initialDepth}
              expandedAll={expandedAll}
              query={query}
              isLast={i === entries.length - 1}
            />
          ))}
          <div style={{ paddingLeft: indent }}>
            <span style={{ color: COLOR.bracket }}>{bracketClose}</span>
            {!isLast && <span style={{ color: COLOR.meta }}>,</span>}
          </div>
        </div>
      )}
    </div>
  );
}

function ToggleIcon({ expanded }: { expanded: boolean }) {
  return (
    <span
      style={{
        display: 'inline-block',
        width: 12,
        color: COLOR.meta,
        fontSize: 10,
        userSelect: 'none',
      }}
    >
      {expanded ? '▼' : '▶'}
    </span>
  );
}

function ValueText({ value, type }: { value: unknown; type: string }): ReactNode {
  if (type === 'string') {
    return (
      <span style={{ color: COLOR.string }}>
        "{value as string}"
      </span>
    );
  }
  if (type === 'number') {
    return <span style={{ color: COLOR.number }}>{String(value)}</span>;
  }
  if (type === 'boolean') {
    return <span style={{ color: COLOR.boolean }}>{String(value)}</span>;
  }
  if (type === 'null') {
    return <span style={{ color: COLOR.null }}>null</span>;
  }
  if (type === 'undefined') {
    return <span style={{ color: COLOR.null }}>undefined</span>;
  }
  return <span>{String(value)}</span>;
}

function getType(value: unknown): string {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  return typeof value;
}
