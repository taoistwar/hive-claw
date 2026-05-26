// AgentTree — 层级树展示，含 depth 限制提示 (T122)

import { Tree, Tag, Tooltip } from 'antd';
import type { DataNode } from 'antd/es/tree';
import type { AgentTreeNode } from '../services/agent';

const MAX_DEPTH = 10;

function toNodes(items: AgentTreeNode[]): DataNode[] {
  return items.map((n) => ({
    key: n.id,
    title: (
      <span>
        <span style={{ marginRight: 8 }}>{n.name}</span>
        <Tag>depth={n.depth}</Tag>
        {n.identifier === 'main' ? <Tag color="purple">main</Tag> : null}
        {n.model_preset ? <Tag color="blue">{n.model_preset}</Tag> : null}
        {n.depth >= MAX_DEPTH - 1 ? (
          <Tooltip title={`已接近 depth=${MAX_DEPTH} 上限`}>
            <Tag color="red">max depth</Tag>
          </Tooltip>
        ) : null}
      </span>
    ),
    children: n.children?.length ? toNodes(n.children) : undefined,
  }));
}

export interface AgentTreeProps {
  data: AgentTreeNode[];
  onSelect?: (id: number | null) => void;
}

export function AgentTree({ data, onSelect }: AgentTreeProps) {
  return (
    <Tree
      treeData={toNodes(data)}
      defaultExpandAll
      showLine
      onSelect={(keys) => onSelect?.((keys[0] as number) ?? null)}
    />
  );
}
