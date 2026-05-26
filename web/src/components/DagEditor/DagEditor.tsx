// DagEditor — reactflow DAG 编辑器 (T113)
//
// MVP scope：
// - 拖入 Function 创建节点
// - 拖动 + 连线
// - 客户端环检测红框预警
// - 保存按钮 → PUT /api/workflows/:id/graph
//
// 节点 mapping 配置 UI 留待后续 — 当前 mapping 默认 {}

import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  Background,
  Controls,
  MiniMap,
  ReactFlow,
  addEdge,
  applyEdgeChanges,
  applyNodeChanges,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node,
  type NodeChange,
} from 'reactflow';
import { Alert, Button, Modal, Select, Space, message } from 'antd';

import {
  getWorkflowGraph,
  putWorkflowGraph,
  type GraphEdge,
  type GraphNode,
} from '../../services/workflow';
import { listFunctions, type FunctionItem } from '../../services/function';
import { detectCycle, type SimpleEdge } from './CycleDetector';
import { DEFAULT_FIT_VIEW_OPTIONS, DEFAULT_FLOW_STYLE } from '../../utils/reactflow';

interface NodeData {
  node_key: string;
  function_id: number;
  function_name?: string;
}

export interface DagEditorProps {
  workflowId: number;
  onSaved?: () => void;
}

export function DagEditor({ workflowId, onSaved }: DagEditorProps) {
  const [nodes, setNodes] = useState<Node<NodeData>[]>([]);
  const [edges, setEdges] = useState<Edge[]>([]);
  const [functions, setFunctions] = useState<FunctionItem[]>([]);
  const [addOpen, setAddOpen] = useState(false);
  const [pickedFn, setPickedFn] = useState<number | undefined>();
  const [pickedKey, setPickedKey] = useState<string>('');

  // 加载已有图 + function 列表
  useEffect(() => {
    void (async () => {
      try {
        const [graph, fnList] = await Promise.all([
          getWorkflowGraph(workflowId),
          listFunctions({ limit: 100 }),
        ]);
        setFunctions(fnList.items);
        const fnMap = new Map(fnList.items.map((f) => [f.id, f]));
        setNodes(
          graph.nodes.map((n: GraphNode, i: number) => ({
            id: n.node_key,
            type: 'default',
            position: n.position ?? { x: 80 + i * 200, y: 80 },
            data: {
              node_key: n.node_key,
              function_id: n.function_id,
              function_name: fnMap.get(n.function_id)?.name,
            },
            style: { padding: 8, border: '1px solid #91caff', borderRadius: 6 },
          })),
        );
        setEdges(
          graph.edges.map((e: GraphEdge, i: number) => ({
            id: `e${i}-${e.src_node_key}-${e.dst_node_key}`,
            source: e.src_node_key,
            target: e.dst_node_key,
            data: { mapping: e.mapping },
          })),
        );
      } catch (e) {
        void message.error(`加载失败：${(e as Error).message}`);
      }
    })();
  }, [workflowId]);

  const onNodesChange = useCallback(
    (changes: NodeChange[]) => setNodes((nds) => applyNodeChanges(changes, nds)),
    [],
  );
  const onEdgesChange = useCallback(
    (changes: EdgeChange[]) => setEdges((es) => applyEdgeChanges(changes, es)),
    [],
  );
  const onConnect = useCallback(
    (params: Connection) =>
      setEdges((es) => addEdge({ ...params, data: { mapping: {} } }, es)),
    [],
  );

  const cycle = useMemo(() => {
    const simpleEdges: SimpleEdge[] = edges
      .filter((e) => e.source && e.target)
      .map((e) => ({ src: e.source!, dst: e.target! }));
    return detectCycle(
      nodes.map((n) => n.id),
      simpleEdges,
    );
  }, [nodes, edges]);

  // 高亮成环节点
  const styledNodes = useMemo(
    () =>
      nodes.map((n) =>
        cycle?.includes(n.id)
          ? { ...n, style: { ...n.style, border: '2px solid #ff4d4f' } }
          : n,
      ),
    [nodes, cycle],
  );

  const onAdd = () => {
    if (!pickedFn || !pickedKey) {
      void message.error('请填写 node_key 和选择 function');
      return;
    }
    if (nodes.some((n) => n.id === pickedKey)) {
      void message.error(`node_key「${pickedKey}」已存在`);
      return;
    }
    const fn = functions.find((f) => f.id === pickedFn);
    setNodes((nds) => [
      ...nds,
      {
        id: pickedKey,
        type: 'default',
        position: { x: 100 + nds.length * 60, y: 100 },
        data: {
          node_key: pickedKey,
          function_id: pickedFn,
          function_name: fn?.name,
        },
        style: { padding: 8, border: '1px solid #91caff', borderRadius: 6 },
      },
    ]);
    setAddOpen(false);
    setPickedFn(undefined);
    setPickedKey('');
  };

  const onSave = async () => {
    if (cycle) {
      void message.error('图中存在环，请先消除');
      return;
    }
    const payload = {
      nodes: nodes.map((n) => ({
        node_key: n.data.node_key,
        function_id: n.data.function_id,
        position: { x: n.position.x, y: n.position.y },
      })),
      edges: edges.map((e) => ({
        src_node_key: e.source!,
        dst_node_key: e.target!,
        mapping: ((e.data as { mapping?: Record<string, string> })?.mapping) ?? {},
      })),
    };
    try {
      await putWorkflowGraph(workflowId, payload);
      void message.success('保存成功');
      onSaved?.();
    } catch (e: unknown) {
      const err = e as { response?: { data?: { code?: number; message?: string } } };
      const m = err.response?.data?.message ?? (e as Error).message;
      void message.error(`保存失败：${m}`);
    }
  };

  return (
    <div>
      <Space style={{ marginBottom: 12 }}>
        <Button onClick={() => setAddOpen(true)}>添加节点</Button>
        <Button type="primary" onClick={onSave} disabled={!!cycle}>
          保存
        </Button>
      </Space>
      {cycle ? (
        <Alert
          type="error"
          message={`检测到环：${cycle.join(' → ')}`}
          style={{ marginBottom: 12 }}
        />
      ) : null}
      <div style={DEFAULT_FLOW_STYLE}>
        <ReactFlow
          nodes={styledNodes}
          edges={edges}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          fitView
          fitViewOptions={DEFAULT_FIT_VIEW_OPTIONS}
        >
          <Controls />
          <MiniMap />
          <Background gap={16} />
        </ReactFlow>
      </div>

      <Modal
        title="添加节点"
        open={addOpen}
        onCancel={() => setAddOpen(false)}
        onOk={onAdd}
      >
        <Space direction="vertical" style={{ width: '100%' }}>
          <input
            placeholder="node_key (workflow 内唯一)"
            value={pickedKey}
            onChange={(e) => setPickedKey(e.target.value)}
            style={{ width: '100%', padding: 8 }}
            aria-label="node_key 输入"
          />
          <Select
            placeholder="选择 function"
            value={pickedFn}
            onChange={setPickedFn}
            style={{ width: '100%' }}
            showSearch
            optionFilterProp="label"
            options={functions.map((f) => ({
              value: f.id,
              label: `${f.name} (${f.identifier})`,
            }))}
          />
        </Space>
      </Modal>
    </div>
  );
}
