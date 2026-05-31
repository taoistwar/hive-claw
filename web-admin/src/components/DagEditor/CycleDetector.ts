// 客户端环检测（T114）— 与后端 services/workflow::detect_cycle 对齐的 white/gray/black DFS。
//
// 输入：edges 列表（按 src_node_key → dst_node_key）
// 输出：成环节点列表（任一）；无环则 null
//
// 客户端检测的目的：在 PUT graph 之前给用户即时红框反馈，不等服务端 4092

export interface SimpleEdge {
  src: string;
  dst: string;
}

export function detectCycle(nodeKeys: string[], edges: SimpleEdge[]): string[] | null {
  const adj = new Map<string, string[]>();
  for (const k of nodeKeys) adj.set(k, []);
  for (const e of edges) {
    if (!adj.has(e.src) || !adj.has(e.dst)) continue;
    adj.get(e.src)!.push(e.dst);
  }

  const White = 0, Gray = 1, Black = 2;
  const color = new Map<string, number>();
  for (const k of nodeKeys) color.set(k, White);

  for (const start of nodeKeys) {
    if (color.get(start) !== White) continue;
    const stack: { node: string; path: string[] }[] = [
      { node: start, path: [start] },
    ];
    color.set(start, Gray);
    while (stack.length) {
      const { node, path } = stack[stack.length - 1];
      const succs = adj.get(node) ?? [];
      let advanced = false;
      for (const next of succs) {
        const c = color.get(next) ?? White;
        if (c === White) {
          color.set(next, Gray);
          stack.push({ node: next, path: [...path, next] });
          advanced = true;
          break;
        } else if (c === Gray) {
          // 找到环：从 next 开始回溯
          const idx = path.indexOf(next);
          return idx >= 0 ? [...path.slice(idx), next] : [next];
        }
      }
      if (!advanced) {
        color.set(node, Black);
        stack.pop();
      }
    }
  }
  return null;
}
