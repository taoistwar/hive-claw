// DagEditor — reactflow DAG 编辑器 (T113)
//
// MVP scope：
// - 拖入 Function 创建节点
// - 拖动 + 连线
// - 客户端环检测红框预警
// - 保存按钮 → PUT /api/workflows/:id/graph
//
// 节点 mapping 配置 UI 留待后续 — 当前 mapping 默认 {}

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Background,
  Controls,
  MiniMap,
  ReactFlow,
  addEdge,
  applyEdgeChanges,
  applyNodeChanges,
  MarkerType,
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
  executeWorkflow,
  type AnswerNodeConfig,
  type GraphEdge,
  type GraphNode,
  type NodeType,
  type WorkflowExecuteResult,
} from '../../services/workflow';
import { listFunctions, type FunctionItem } from '../../services/function';
import { detectCycle, type SimpleEdge } from './CycleDetector';
import { DEFAULT_FIT_VIEW_OPTIONS, DEFAULT_FLOW_STYLE } from '../../utils/reactflow';
import { CustomNode } from './CustomNode';
import { StartNode } from './StartNode';
import { EndNode } from './EndNode';
import { AnswerNode } from './AnswerNode';
import { ArrowEdge } from './ArrowEdge';
import { ContextMenu } from './ContextMenu';
import { FunctionDetail } from '../FunctionDetail';
import { NodeDetailDrawer } from './NodeDetailDrawer';

interface NodeData {
  node_key: string;
  function_id?: number | null;
  node_type?: NodeType;
  function_name?: string;
  input_schema?: Record<string, unknown> | null;
  output_schema?: Record<string, unknown> | null;
  node_config?: AnswerNodeConfig | null;
  execution_result?: unknown;
}

export interface DagEditorProps {
  workflowId: number;
  readonly?: boolean;
  onSaved?: () => void;
}

const nodeTypes = {
  custom: CustomNode,
  start: StartNode,
  end: EndNode,
  answer: AnswerNode,
};

const edgeTypes = {
  default: ArrowEdge,
};

function EdgeContextMenu({ x, y, edgeId, onDelete, onClose }: {
  x: number;
  y: number;
  edgeId: string;
  onDelete: (edgeId: string) => void;
  onClose: () => void;
}) {
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [onClose]);

  return (
    <div
      ref={menuRef}
      style={{
        position: 'fixed',
        top: y,
        left: x,
        background: '#fff',
        border: '1px solid #e8e8e8',
        borderRadius: 4,
        boxShadow: '0 4px 16px rgba(0, 0, 0, 0.15)',
        padding: 4,
        zIndex: 9999,
        minWidth: 100,
      }}
    >
      <button
        onClick={() => onDelete(edgeId)}
        style={{
          width: '100%',
          padding: '8px 12px',
          textAlign: 'left',
          border: 'none',
          background: 'none',
          cursor: 'pointer',
          fontSize: 14,
          color: '#ff4d4f',
          borderRadius: 4,
        }}
        onMouseEnter={(e) => {
          (e.target as HTMLElement).style.background = '#fff2f0';
        }}
        onMouseLeave={(e) => {
          (e.target as HTMLElement).style.background = 'none';
        }}
      >
        删除连线
      </button>
    </div>
  );
}

export function DagEditor({ workflowId, readonly, onSaved }: DagEditorProps) {
  const [nodes, setNodes] = useState<Node<NodeData>[]>([]);
  const [edges, setEdges] = useState<Edge[]>([]);
  const [functions, setFunctions] = useState<FunctionItem[]>([]);
  const [addOpen, setAddOpen] = useState(false);
  const [pickedFn, setPickedFn] = useState<number | undefined>();
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    nodeId: string;
    functionId?: number | null;
    isStartNode?: boolean;
    isEndNode?: boolean;
    isAnswerNode?: boolean;
  } | null>(null);
  const [edgeContextMenu, setEdgeContextMenu] = useState<{
    x: number;
    y: number;
    edgeId: string;
  } | null>(null);
  const [viewFnOpen, setViewFnOpen] = useState(false);
  const [viewingFn, setViewingFn] = useState<FunctionItem | null>(null);
  const [detailDrawerOpen, setDetailDrawerOpen] = useState(false);
  const [selectedNode, setSelectedNode] = useState<{
    nodeType: 'start' | 'end' | 'function' | 'generate_answer';
    nodeKey: string;
    functionId?: number | null;
    inputSchema?: Record<string, unknown> | null;
    outputSchema?: Record<string, unknown> | null;
    answerConfig?: AnswerNodeConfig | null;
  } | null>(null);
  // 运行相关状态
  const [runModalOpen, setRunModalOpen] = useState(false);
  const [runInputJson, setRunInputJson] = useState<string>('{}');
  const [isRunning, setIsRunning] = useState(false);
  const [executionResult, setExecutionResult] = useState<WorkflowExecuteResult | null>(null);
  const [resultModalOpen, setResultModalOpen] = useState(false);
  const [selectedNodeResult, setSelectedNodeResult] = useState<{
    nodeKey: string;
    result: unknown;
  } | null>(null);

  const STORAGE_KEY = `dag_editor_v2_${workflowId}`;

  const fetchGraph = useCallback(async (skipCache = false) => {
    if (!skipCache) {
      try {
        const cached = localStorage.getItem(STORAGE_KEY);
        if (cached) {
          const { nodes: cachedNodes, edges: cachedEdges, functions: cachedFunctions } = JSON.parse(cached);
          setNodes(cachedNodes);
          setEdges(cachedEdges);
          setFunctions(cachedFunctions);
          return;
        }
      } catch { /* ignore parse error, fallback to API */ }
    }

    try {
      const [graph, fnList] = await Promise.all([
        getWorkflowGraph(workflowId),
        listFunctions({ limit: 100 }),
      ]);
      setFunctions(fnList.items);
      const fnMap = new Map(fnList.items.map((f) => [f.id, f]));
      setNodes(
        graph.nodes.map((n: GraphNode, i: number) => {
          const isStart = n.node_type === 'start_node' || n.node_key === 'start';
          const isEnd = n.node_type === 'end_node' || n.node_key === 'end';
          const isAnswer = n.node_type === 'generate_answer_node';
          return {
            id: n.node_key,
            type: isStart ? 'start' : isEnd ? 'end' : isAnswer ? 'answer' : 'custom',
            position: n.position ?? { x: 80 + i * 200, y: 80 },
            data: {
              node_key: n.node_key,
              function_id: isStart || isEnd || isAnswer ? undefined : n.function_id,
              node_type: isStart ? 'start_node' as NodeType : isEnd ? 'end_node' as NodeType : isAnswer ? 'generate_answer_node' as NodeType : 'function_node' as NodeType,
              function_name: isStart || isEnd || isAnswer ? undefined : (n.function_id ? fnMap.get(n.function_id)?.name : undefined),
              input_schema: isStart ? (graph.workflow.input_schema ?? null) : undefined,
              output_schema: isEnd ? (graph.workflow.output_schema ?? null) : undefined,
              node_config: isAnswer ? (n.node_config ?? null) : undefined,
            },
          };
        }),
      );
      setEdges(
        graph.edges.map((e: GraphEdge, i: number) => ({
          id: `e${i}-${e.src_node_key}-${e.dst_node_key}`,
          source: e.src_node_key,
          target: e.dst_node_key,
          data: { mapping: e.mapping },
          markerEnd: { type: MarkerType.Arrow, color: '#555' },
          style: { stroke: '#555', strokeWidth: 2 },
        })),
      );
    } catch (e) {
      void message.error(`加载失败：${(e as Error).message}`);
    }
  }, [workflowId, STORAGE_KEY]);

  useEffect(() => {
    void fetchGraph();
  }, [fetchGraph]);

  // 自动同步到 localStorage（覆盖拖拽、连线、配置修改等所有操作）
  useEffect(() => {
    if (nodes.length === 0) return;
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify({ nodes, edges, functions }));
    } catch { /* ignore quota errors */ }
  }, [nodes, edges, functions, STORAGE_KEY]);

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
      setEdges((es) =>
        addEdge(
          {
            ...params,
            data: { mapping: {} },
            markerEnd: { type: MarkerType.Arrow, color: '#555' },
            style: { stroke: '#555', strokeWidth: 2 },
          },
          es,
        ),
      ),
    [],
  );

  const onNodeContextMenu = useCallback((event: React.MouseEvent, node: Node<NodeData>) => {
    event.preventDefault();
    const isStart = node.data.node_type === 'start_node' || node.id === 'start';
    const isEnd = node.data.node_type === 'end_node' || node.id === 'end';
    const isAnswer = node.data.node_type === 'generate_answer_node';
    setContextMenu({
      x: event.clientX,
      y: event.clientY,
      nodeId: node.id,
      functionId: node.data.function_id,
      isStartNode: isStart,
      isEndNode: isEnd,
      isAnswerNode: isAnswer,
    });
  }, []);

  const onEdgeContextMenu = useCallback(
    (event: React.MouseEvent, edge: Edge) => {
      event.preventDefault();
      setEdgeContextMenu({
        x: event.clientX,
        y: event.clientY,
        edgeId: edge.id,
      });
    },
    [],
  );

  const onDeleteEdge = useCallback((edgeId: string) => {
    setEdges((es) => es.filter((e) => e.id !== edgeId));
    void message.success('连线已删除');
  }, []);

  const onNodeClick = useCallback((_event: React.MouseEvent, node: Node<NodeData>) => {
    if (executionResult) {
      const result = executionResult.node_results[node.data.node_key];
      if (result !== undefined) {
        setSelectedNodeResult({ nodeKey: node.data.node_key, result });
        setResultModalOpen(true);
        return;
      }
    }
    const isStart = node.data.node_type === 'start_node' || node.id === 'start';
    const isEnd = node.data.node_type === 'end_node' || node.id === 'end';
    const isAnswer = node.data.node_type === 'generate_answer_node';
    setSelectedNode({
      nodeType: isStart ? 'start' : isEnd ? 'end' : isAnswer ? 'generate_answer' : 'function',
      nodeKey: node.data.node_key,
      functionId: node.data.function_id,
      inputSchema: isStart ? node.data.input_schema : undefined,
      outputSchema: isEnd ? node.data.output_schema : undefined,
      answerConfig: isAnswer ? (node.data.node_config ?? null) : undefined,
    });
    setDetailDrawerOpen(true);
  }, [executionResult]);

  const onUpdateStartNode = useCallback((vars: Record<string, unknown>) => {
    setNodes((nds) =>
      nds.map((n) =>
        n.id === 'start'
          ? { ...n, data: { ...n.data, input_schema: vars } }
          : n,
      ),
    );
    void message.success('起始节点配置已更新');
  }, []);

  const onUpdateEndNode = useCallback((vars: Record<string, unknown>) => {
    setNodes((nds) =>
      nds.map((n) =>
        n.id === 'end'
          ? { ...n, data: { ...n.data, output_schema: vars } }
          : n,
      ),
    );
    void message.success('结束节点配置已更新');
  }, []);

  const onUpdateAnswerNode = useCallback((nodeKey: string, config: AnswerNodeConfig) => {
    setNodes((nds) =>
      nds.map((n) =>
        n.id === nodeKey
          ? { ...n, data: { ...n.data, node_config: config } }
          : n,
      ),
    );
    // Update selectedNode as well so it reflects immediately
    setSelectedNode((prev) =>
      prev?.nodeKey === nodeKey ? { ...prev, answerConfig: config } : prev
    );
    void message.success('回答节点配置已更新');
  }, []);

  const onDeleteNode = useCallback((nodeId: string) => {
    if (nodeId === 'start') {
      void message.error('起始节点不能删除');
      return;
    }
    if (nodeId === 'end') {
      void message.error('结束节点不能删除');
      return;
    }
    setNodes((nds) => nds.filter((n) => n.id !== nodeId));
    setEdges((es) => es.filter((e) => e.source !== nodeId && e.target !== nodeId));
    void message.success('节点已删除');
  }, []);

  const onViewFunction = useCallback((functionId: number) => {
    const fn = functions.find((f) => f.id === functionId);
    if (fn) {
      setViewingFn(fn);
      setViewFnOpen(true);
    }
  }, [functions]);

  const cycle = useMemo(() => {
    const simpleEdges: SimpleEdge[] = edges
      .filter((e) => e.source && e.target)
      .map((e) => ({ src: e.source!, dst: e.target! }));
    return detectCycle(
      nodes.map((n) => n.id),
      simpleEdges,
    );
  }, [nodes, edges]);

  const styledNodes = useMemo(
    () =>
      nodes.map((n) => {
        let node = n;
        if (cycle?.includes(n.id)) {
          node = { ...n, data: { ...n.data, style: { border: '2px solid #ff4d4f' } } };
        }
        if (executionResult) {
          const result = executionResult.node_results[n.data.node_key];
          if (result !== undefined) {
            node = { ...node, data: { ...node.data, execution_result: result } };
          }
        }
        return node;
      }),
    [nodes, cycle, executionResult],
  );

  const onAdd = () => {
    if (!pickedFn) {
      void message.error('请选择 function');
      return;
    }
    // 找到所有现有的 function_ 开头的 node_key，找出最大的 seq
    let maxSeq = 0;
    for (const node of nodes) {
      if (typeof node.id === 'string' && node.id.startsWith('function_')) {
        const match = node.id.match(/function_(\d+)/);
        if (match) {
          const seq = parseInt(match[1], 10);
          if (seq > maxSeq) {
            maxSeq = seq;
          }
        }
      }
    }
    const newSeq = maxSeq + 1;
    const newKey = `function_${newSeq}`;
    
    const fn = functions.find((f) => f.id === pickedFn);
    setNodes((nds) => [
      ...nds,
      {
        id: newKey,
        type: 'custom',
        position: { x: 100 + nds.length * 60, y: 100 },
        data: {
          node_key: newKey,
          function_id: pickedFn,
          node_type: 'function_node' as NodeType,
          function_name: fn?.name,
        },
      },
    ]);
    setAddOpen(false);
    setPickedFn(undefined);
  };

  const onAddAnswer = () => {
    const generatedKey = `answer_${nodes.length + 1}`;
    if (nodes.some((n) => n.id === generatedKey)) {
      void message.error(`node_key「${generatedKey}」已存在，请先删除同名节点`);
      return;
    }
    setNodes((nds) => {
      const startNode = nds.find((n) => n.id === 'start');
      const baseX = startNode ? startNode.position.x + 200 : 100;
      const baseY = startNode ? startNode.position.y + 150 : 100;
      return [
        ...nds,
        {
          id: generatedKey,
          type: 'answer',
          position: { x: baseX, y: baseY },
        data: {
          node_key: generatedKey,
          node_type: 'generate_answer_node' as NodeType,
          node_config: {
            system_prompt: '你是一个智能助手，请根据以下内容回答用户问题：\n{{query}}',
            history_window: 5,
            variables: [],
          },
        },
      },
      ];
    });
    void message.success('回答节点已添加');
  };

  const onSave = async () => {
    if (cycle) {
      void message.error('图中存在环，请先消除');
      return;
    }
    const payload = {
      nodes: nodes.map((n) => {
        const isStart = n.data.node_type === 'start_node' || n.id === 'start';
        const isEnd = n.data.node_type === 'end_node' || n.id === 'end';
        const isAnswer = n.data.node_type === 'generate_answer_node';
        return {
          node_key: n.data.node_key,
          node_type: isStart ? ('start_node' as const) : isEnd ? ('end_node' as const) : isAnswer ? ('generate_answer_node' as const) : ('function_node' as const),
          function_id: isStart || isEnd || isAnswer ? null : n.data.function_id,
          position: {
            x: n.position.x,
            y: n.position.y,
            ...(isStart && n.data.input_schema && {
              input_schema: n.data.input_schema,
            }),
            ...(isEnd && n.data.output_schema && {
              output_schema: n.data.output_schema,
            }),
          },
          ...(isAnswer && n.data.node_config && {
            node_config: n.data.node_config,
          }),
        };
      }),
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

  const onReset = useCallback(() => {
    localStorage.removeItem(STORAGE_KEY);
    void fetchGraph(true);
    void message.success('已重置为服务器保存的版本');
  }, [STORAGE_KEY, fetchGraph]);

  const handleRunClick = useCallback(() => {
    setRunModalOpen(true);
  }, []);

  const handleRun = useCallback(async () => {
    let input: Record<string, unknown>;
    try {
      input = JSON.parse(runInputJson);
    } catch (e) {
      void message.error('JSON 格式错误');
      return;
    }
    setIsRunning(true);
    try {
      const result = await executeWorkflow(workflowId, input);
      setExecutionResult(result);
      setRunModalOpen(false);
      void message.success(`执行成功，耗时 ${result.elapsed_ms}ms`);
    } catch (e) {
      void message.error(`执行失败: ${(e as Error).message}`);
    } finally {
      setIsRunning(false);
    }
  }, [workflowId, runInputJson]);



  return (
    <div>
      {!readonly && (
        <Space style={{ marginBottom: 12 }}>
          <Button onClick={() => setAddOpen(true)}>添加函数节点</Button>
          <Button onClick={onAddAnswer}>添加回答节点</Button>
          <Button type="default" onClick={handleRunClick} disabled={!!cycle}>
            运行
          </Button>
          <Button type="primary" onClick={onSave} disabled={!!cycle}>
            保存
          </Button>
          <Button onClick={onReset}>重置</Button>
        </Space>
      )}
      {/* 运行参数输入弹窗 */}
      <Modal
        title="运行工作流"
        open={runModalOpen}
        onCancel={() => setRunModalOpen(false)}
        onOk={handleRun}
        confirmLoading={isRunning}
        width={600}
      >
        <div style={{ marginBottom: 12 }}>
          <label style={{ display: 'block', marginBottom: 8, fontWeight: 500 }}>
            输入参数（JSON 格式）
          </label>
          <textarea
            value={runInputJson}
            onChange={(e) => setRunInputJson(e.target.value)}
            style={{
              width: '100%',
              height: 200,
              fontFamily: 'monospace',
              fontSize: 14,
              padding: 12,
              border: '1px solid #d9d9d9',
              borderRadius: 6,
            }}
            placeholder='{"query": "请输入您的问题..."}'
          />
        </div>
      </Modal>
      {/* 节点结果查看弹窗 */}
      <Modal
        title={selectedNodeResult ? `节点 ${selectedNodeResult.nodeKey} 运行结果` : '节点结果'}
        open={resultModalOpen}
        onCancel={() => setResultModalOpen(false)}
        footer={[
          <Button key="close" onClick={() => setResultModalOpen(false)}>
            关闭
          </Button>,
        ]}
        width={700}
      >
        {selectedNodeResult && (
          <div>
            <pre
              style={{
                background: '#f5f5f5',
                padding: 16,
                borderRadius: 6,
                fontSize: 13,
                overflow: 'auto',
                maxHeight: 500,
              }}
            >
              {JSON.stringify(selectedNodeResult.result, null, 2)}
            </pre>
          </div>
        )}
      </Modal>
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
          onNodeContextMenu={onNodeContextMenu}
          onNodeClick={onNodeClick}
          onEdgeContextMenu={onEdgeContextMenu}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          defaultEdgeOptions={{
            markerEnd: { type: MarkerType.Arrow, color: '#555' },
            style: { stroke: '#555', strokeWidth: 2 },
          }}
          deleteKeyCode={['Backspace', 'Delete']}
          connectionRadius={20}
          nodesConnectable
          fitView
          fitViewOptions={DEFAULT_FIT_VIEW_OPTIONS}
        >
          <Controls />
          <MiniMap />
          <Background gap={16} />
        </ReactFlow>
      </div>

      <Modal
        title="添加函数节点"
        open={addOpen}
        onCancel={() => setAddOpen(false)}
        onOk={onAdd}
      >
        <Space direction="vertical" style={{ width: '100%' }}>
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

      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          nodeId={contextMenu.nodeId}
          functionId={contextMenu.functionId}
          onDelete={onDeleteNode}
          onViewFunction={onViewFunction}
          onClose={() => setContextMenu(null)}
        />
      )}

      {edgeContextMenu && (
        <EdgeContextMenu
          x={edgeContextMenu.x}
          y={edgeContextMenu.y}
          edgeId={edgeContextMenu.edgeId}
          onDelete={(id) => { onDeleteEdge(id); setEdgeContextMenu(null); }}
          onClose={() => setEdgeContextMenu(null)}
        />
      )}

      <Modal
        title="函数详情"
        open={viewFnOpen}
        onCancel={() => setViewFnOpen(false)}
        footer={null}
        width={600}
      >
        {viewingFn && <FunctionDetail fn={viewingFn} />}
      </Modal>

      {selectedNode && (
        <NodeDetailDrawer
          open={detailDrawerOpen}
          onClose={() => setDetailDrawerOpen(false)}
          nodeType={selectedNode.nodeType}
          nodeKey={selectedNode.nodeKey}
          functionId={selectedNode.functionId}
          inputSchema={selectedNode.inputSchema}
          outputSchema={selectedNode.outputSchema}
          answerConfig={selectedNode.answerConfig}
          onUpdateStartNode={onUpdateStartNode}
          onUpdateEndNode={onUpdateEndNode}
          onUpdateAnswerNode={selectedNode.nodeType === 'generate_answer' ? (config: AnswerNodeConfig) => onUpdateAnswerNode(selectedNode.nodeKey, config) : undefined}
          allNodes={nodes}
          allEdges={edges}
          allFunctions={functions}
        />
      )}
    </div>
  );
}