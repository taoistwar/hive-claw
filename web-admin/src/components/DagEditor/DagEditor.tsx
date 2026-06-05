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
  ReactFlow,
  ReactFlowProvider,
  MiniMap,
  addEdge,
  applyNodeChanges,
  applyEdgeChanges,
  useNodesState,
  useEdgesState,
  Controls,
  Background,
  Handle,
  Position,
  MarkerType,
  type Connection,
  type Edge,
  type Node,
  type EdgeChange,
  type NodeChange,
} from 'reactflow';
import {
  Alert,
  Button,
  Col,
  Form,
  Input,
  InputNumber,
  Modal,
  Row,
  Select,
  Space,
  Spin,
  Switch,
  Typography,
  message,
} from 'antd';

import {
  getWorkflowGraph,
  putWorkflowGraph,
  executeWorkflow,
  type AnswerNodeConfig,
  type GraphEdge,
  type GraphNode,
  type NodeType,
  type WorkflowExecuteResult,
  type WorkflowUserInput,
} from '../../services/workflow';

const { Text, Paragraph } = Typography;

interface SchemaProperty {
  type?: string;
  description?: string;
  enum?: unknown[];
  default?: unknown;
  [key: string]: unknown;
}
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

/**
 * 从 JSON Schema 中解析 properties，生成表单字段配置
 * 支持类型：string, number, integer, boolean, enum
 */
function parseSchemaProperties(schema: unknown): Array<{ key: string; prop: SchemaProperty }> {
  if (!schema || typeof schema !== 'object') return [];
  const schemaObj = schema as Record<string, unknown>;
  const properties = schemaObj.properties as Record<string, SchemaProperty> | undefined;
  if (!properties) return [];
  return Object.entries(properties).map(([key, prop]) => ({ key, prop }));
}

/**
 * 根据结束节点的 output_schema，从上游节点结果中提取匹配的字段
 * e.g. output_schema.properties = { answer: {...} }, upstream = { answer: "x", model: "y" }
 *   → 返回 { answer: "x" }
 */
function extractOutputFields(
  outputSchema: Record<string, unknown> | null | undefined,
  upstreamResults: Record<string, unknown>,
): Record<string, unknown> | null {
  const schemaFields = outputSchema?.properties
    ? Object.keys(outputSchema.properties as Record<string, unknown>)
    : [];
  if (schemaFields.length === 0) {
    // 未配置 output_schema → 不显示结果
    return null;
  }
  const extracted: Record<string, unknown> = {};
  for (const key of schemaFields) {
    if (key in upstreamResults) {
      extracted[key] = (upstreamResults as Record<string, unknown>)[key];
    }
  }
  return extracted;
}

/** 从 form values 中分离工作流输入参数和 UserInput 上下文（_ctx_ 前缀） */
function splitRunValues(values: Record<string, unknown>): {
  input: Record<string, unknown>;
  user_input?: WorkflowUserInput;
} {
  const input: Record<string, unknown> = {};
  const ctx: Record<string, string> = {};
  for (const [k, v] of Object.entries(values)) {
    if (k.startsWith('_ctx_')) {
      const field = k.slice('_ctx_'.length);
      if (typeof v === 'string' && v.length > 0) {
        ctx[field] = v;
      }
    } else {
      input[k] = v;
    }
  }
  const user_input = Object.keys(ctx).length > 0 ? (ctx as WorkflowUserInput) : undefined;
  return { input, user_input };
}

/** 渲染单个表单字段 */
function renderFormField(key: string, prop: SchemaProperty) {
  const type = prop.type ?? 'string';
  const label = prop.description ? (
    <span>
      {key}
      <Text type="secondary" style={{ marginLeft: 8, fontSize: 12 }}>
        {prop.description}
      </Text>
    </span>
  ) : (
    key
  );

  switch (type) {
    case 'string':
      if (prop.enum && Array.isArray(prop.enum)) {
        return (
          <Form.Item key={key} name={key} label={label} initialValue={prop.default}>
            <Select placeholder={`选择 ${key}`}>
              {prop.enum.map((v) => (
                <Select.Option key={String(v)} value={v}>
                  {String(v)}
                </Select.Option>
              ))}
            </Select>
          </Form.Item>
        );
      }
      return (
        <Form.Item key={key} name={key} label={label} initialValue={prop.default}>
          <Input placeholder={`输入 ${key}`} />
        </Form.Item>
      );

    case 'number':
    case 'integer':
      return (
        <Form.Item key={key} name={key} label={label} initialValue={prop.default}>
          <InputNumber
            placeholder={`输入 ${key}`}
            style={{ width: '100%' }}
            step={type === 'integer' ? 1 : 0.1}
          />
        </Form.Item>
      );

    case 'boolean':
      return (
        <Form.Item key={key} name={key} label={label} valuePropName="checked" initialValue={prop.default}>
          <Switch />
        </Form.Item>
      );

    default:
      return (
        <Form.Item key={key} name={key} label={label} initialValue={prop.default}>
          <Input placeholder={`输入 ${key} (${type})`} />
        </Form.Item>
      );
  }
}

interface NodeData {
  node_key: string;
  function_id?: number | null;
  node_type?: NodeType;
  function_name?: string;
  input_schema?: Record<string, unknown> | null;
  output_schema?: Record<string, unknown> | null;
  node_config?: AnswerNodeConfig | null;
  input_mapping?: Record<string, { source: 'upstream' | 'custom'; source_node_key?: string; source_field?: string; custom_value?: string }> | null;
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
      if (menuRef.current && !menuRef.current.contains(e.target as HTMLElement)) {
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
  const [form] = Form.useForm();
  const [nodes, setNodes] = useState<Node<NodeData>[]>([]);
  const [edges, setEdges] = useState<Edge[]>([]);
  const [functions, setFunctions] = useState<FunctionItem[]>([]);
  const [workflow, setWorkflow] = useState<any>(null);
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
    inputMapping?: Record<string, { source: 'upstream' | 'custom'; source_node_key?: string; source_field?: string; custom_value?: string }> | null;
  } | null>(null);
  // 运行相关状态
  const [runModalOpen, setRunModalOpen] = useState(false);
  const [isRunning, setIsRunning] = useState(false);
  const [executionResult, setExecutionResult] = useState<WorkflowExecuteResult | null>(null);
  const [lastRunInput, setLastRunInput] = useState<Record<string, unknown> | null>(null);
  const [resultModalOpen, setResultModalOpen] = useState(false);
  const [selectedNodeResult, setSelectedNodeResult] = useState<{
    nodeKey: string;
    result: unknown;
  } | null>(null);
  /** 是否展开 UserInput 上下文配置 */
  const [showContext, setShowContext] = useState(false);

  const STORAGE_KEY = `dag_editor_v2_${workflowId}`;

  const fetchGraph = useCallback(async (skipCache = false) => {
    if (!skipCache) {
      try {
        const cached = localStorage.getItem(STORAGE_KEY);
        if (cached) {
          const { nodes: cachedNodes, edges: cachedEdges, executionResult: cachedResult, lastRunInput: cachedInput } = JSON.parse(cached);
          setNodes(cachedNodes);
          setEdges(cachedEdges);
          if (cachedResult) setExecutionResult(cachedResult);
          if (cachedInput) setLastRunInput(cachedInput);
          // 始终从 API 获取最新的函数列表，避免缓存导致新函数不显示
          listFunctions({ limit: 500 })
            .then((fnList) => setFunctions(fnList.items))
            .catch(() => {});
          return;
        }
      } catch { /* ignore parse error, fallback to API */ }
    }

    try {
      const [graph, fnList] = await Promise.all([
        getWorkflowGraph(workflowId),
        listFunctions({ limit: 500 }),
      ]);
      setWorkflow(graph.workflow);
      setFunctions(fnList.items);
      const fnMap = new Map(fnList.items.map((f) => [f.id, f]));
      setNodes(
        graph.nodes.map((n: GraphNode, i: number) => {
          const isStart = n.node_type === 'start_node' || n.node_key === 'start';
          const isEnd = n.node_type === 'end_node' || n.node_key === 'end';
          const isAnswer = n.node_type === 'generate_answer_node';
          // 从 node_config 中恢复函数节点的 input_mapping
          const isFunctionNode = !isStart && !isEnd && !isAnswer;
          const cfg = n.node_config as Record<string, unknown> | null | undefined;
          const inputMapping = (isFunctionNode && cfg?.input_mapping)
            ? cfg.input_mapping as Record<string, { source: 'upstream' | 'custom'; source_node_key?: string; source_field?: string; custom_value?: string }>
            : undefined;

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
              node_config: isAnswer ? ((n.node_config ?? null) as AnswerNodeConfig | null) : undefined,
              input_mapping: inputMapping ?? undefined,
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

  // 自动同步到 localStorage
  useEffect(() => {
    if (nodes.length === 0) return;
    try {
      localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify({ nodes, edges, functions, executionResult, lastRunInput }),
      );
    } catch { /* ignore quota errors */ }
  }, [nodes, edges, functions, executionResult, lastRunInput, STORAGE_KEY]);

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
    const isStart = node.data.node_type === 'start_node' || node.id === 'start';
    const isEnd = node.data.node_type === 'end_node' || node.id === 'end';
    const isAnswer = node.data.node_type === 'generate_answer_node';
    const isFunction = !isStart && !isEnd && !isAnswer;
    setSelectedNode({
      nodeType: isStart ? 'start' : isEnd ? 'end' : isAnswer ? 'generate_answer' : 'function',
      nodeKey: node.data.node_key,
      functionId: node.data.function_id,
      inputSchema: isStart ? node.data.input_schema : undefined,
      outputSchema: isEnd ? node.data.output_schema : undefined,
      answerConfig: isAnswer ? (node.data.node_config ?? null) : undefined,
      inputMapping: isFunction ? (node.data.input_mapping ?? null) : undefined,
    });
    setDetailDrawerOpen(true);
  }, []);

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

  const onUpdateFunctionNode = useCallback(
    (nodeKey: string, inputMapping: Record<string, { source: 'upstream' | 'custom'; source_node_key?: string; source_field?: string; custom_value?: string }>) => {
      setNodes((nds) =>
        nds.map((n) =>
          n.id === nodeKey || n.data.node_key === nodeKey
            ? { ...n, data: { ...n.data, input_mapping: inputMapping } }
            : n,
        ),
      );
      setSelectedNode((prev) =>
        prev?.nodeKey === nodeKey ? { ...prev, inputMapping } : prev
      );
      void message.success('函数节点输入映射已更新');
    },
    [],
  );

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

  /** 从当前 nodes 中提取 start 节点的 input_schema，用于"测试"弹窗 */
  const startInputSchema = useMemo(() => {
    const startNode = nodes.find((n) => n.data.node_type === 'start_node' || n.id === 'start');
    return startNode?.data.input_schema ?? null;
  }, [nodes]);

  /** 找到 DAG 中连接到结束节点的上游节点 key（即最终输出节点） */
  const finalOutputNodeKeys = useMemo(() => {
    const endNode = nodes.find((n) => n.data.node_type === 'end_node' || n.id === 'end');
    const endKey = endNode?.data.node_key ?? 'end';
    // 找到所有指向 end 节点的边，收集上游 source
    return edges
      .filter((e) => e.target === endKey && e.source)
      .map((e) => e.source!);
  }, [nodes, edges]);

  /** 结束节点的 output_schema */
  const endOutputSchema = useMemo(() => {
    const endNode = nodes.find((n) => n.data.node_type === 'end_node' || n.id === 'end');
    return endNode?.data.output_schema ?? null;
  }, [nodes]);

  /** 为每个节点附加执行结果标识（含开始/结束节点） */
  const styledNodes = useMemo(
    () =>
      nodes.map((n) => {
        let node = n;
        const isStart = n.data.node_type === 'start_node' || n.id === 'start';
        const isEnd = n.data.node_type === 'end_node' || n.id === 'end';

        if (cycle?.includes(n.id)) {
          node = { ...n };
        }

        // 开始节点：有上次运行输入时标记为有结果
        if (isStart && lastRunInput) {
          node = { ...node, data: { ...node.data, execution_result: lastRunInput } };
        }

        // 结束节点：根据 output_schema 提取上游输出字段
        if (isEnd && executionResult) {
          // 收集所有上游结果
          const allUpstream: Record<string, unknown> = {};
          for (const key of finalOutputNodeKeys) {
            const r = executionResult.node_results[key];
            if (r !== undefined && typeof r === 'object' && r !== null) {
              Object.assign(allUpstream, r as Record<string, unknown>);
            }
          }
          if (Object.keys(allUpstream).length > 0) {
            const extracted = extractOutputFields(n.data.output_schema, allUpstream);
            if (extracted !== null) {
              node = { ...node, data: { ...node.data, execution_result: extracted } };
            }
          }
        }

        // 普通节点：从 node_results 中查找
        if (executionResult && !isStart && !isEnd) {
          const result = executionResult.node_results[n.data.node_key];
          if (result !== undefined) {
            node = { ...node, data: { ...node.data, execution_result: result } };
          }
        }
        return node;
      }),
    [nodes, cycle, executionResult, lastRunInput, finalOutputNodeKeys],
  );

  // 监听节点组件发出的「查看结果」事件
  useEffect(() => {
    const handler = (e: Event) => {
      const { nodeKey } = (e as CustomEvent<{ nodeKey: string }>).detail;
      const isStart = nodeKey === 'start' || nodeKey === 'start_node';
      const isEnd = nodeKey === 'end' || nodeKey === 'end_node';

      if (isStart && lastRunInput) {
        setSelectedNodeResult({ nodeKey, result: lastRunInput });
        setResultModalOpen(true);
        return;
      }

      if (isEnd && executionResult) {
        // 根据结束节点的 output_schema 提取上游输出字段
        const allUpstream: Record<string, unknown> = {};
        for (const key of finalOutputNodeKeys) {
          const r = executionResult.node_results[key];
          if (r !== undefined && typeof r === 'object' && r !== null) {
            Object.assign(allUpstream, r as Record<string, unknown>);
          }
        }
        if (Object.keys(allUpstream).length > 0) {
          const extracted = extractOutputFields(endOutputSchema, allUpstream);
          if (extracted !== null) {
            setSelectedNodeResult({ nodeKey, result: extracted });
            setResultModalOpen(true);
          }
        }
        return;
      }

      // 普通节点：从 executionResult 中查找
      if (executionResult) {
        const result = executionResult.node_results[nodeKey];
        if (result !== undefined) {
          setSelectedNodeResult({ nodeKey, result });
          setResultModalOpen(true);
        }
      }
    };

    window.addEventListener('node-view-result', handler);
    return () => window.removeEventListener('node-view-result', handler);
  }, [executionResult, lastRunInput, finalOutputNodeKeys]);

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
    setNodes((nds) => {
      // 找到开始节点位置，将新节点放在其正下方
      const startNode = nds.find((n) => n.id === 'start' || n.data.node_type === 'start_node');
      const baseX = startNode ? startNode.position.x : 100;
      const baseY = startNode ? startNode.position.y + 120 : 300;
      const offsetX = (nds.filter((n) => n.type === 'custom' || n.type === 'answer').length % 3) * 200;
      return [
        ...nds,
        {
          id: newKey,
          type: 'custom',
          position: { x: baseX + offsetX, y: baseY },
        data: {
          node_key: newKey,
          function_id: pickedFn,
          node_type: 'function_node' as NodeType,
          function_name: fn?.name,
        },
      },
    ];
    });
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
    // 检查结束节点是否配置了输出变量
    const endOutputProps = endOutputSchema?.properties
      ? (endOutputSchema.properties as Record<string, unknown>)
      : {};
    if (Object.keys(endOutputProps).length === 0) {
      void message.warning('结束节点尚未配置输出变量，保存后执行结果将不会包含结束节点输出');
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
            ...(isStart && { input_schema: n.data.input_schema ?? { type: 'object', properties: {} } }),
            ...(isEnd && n.data.output_schema && { output_schema: n.data.output_schema }),
          },
          ...(isAnswer && n.data.node_config && {
            node_config: n.data.node_config,
          }),
          // 函数节点：将 input_mapping 写入 node_config，供后端执行引擎读取
          ...(!isStart && !isEnd && !isAnswer && n.data.input_mapping && {
            node_config: { input_mapping: n.data.input_mapping },
          }),
        };
      }),
      edges: edges.map((e) => {
        // 查找目标函数节点的 input_mapping，合并到边 mapping
        const dstNode = nodes.find((n) => n.id === e.target);
        const inputMapping = dstNode?.data.input_mapping as Record<string, { source: 'upstream' | 'custom'; source_node_key?: string; source_field?: string; custom_value?: string }> | undefined;
        const edgeMapping: Record<string, string> = {};
        if (inputMapping) {
          for (const [field, m] of Object.entries(inputMapping)) {
            if (m.source === 'upstream' && m.source_node_key) {
              edgeMapping[`dst.input.${field}`] = m.source_field
                ? `${m.source_node_key}.output.${m.source_field}`
                : `${m.source_node_key}.output`;
            } else if (m.source === 'custom' && m.custom_value !== undefined) {
              edgeMapping[`dst.input.${field}`] = m.custom_value;
            }
          }
        }
        return {
          src_node_key: e.source!,
          dst_node_key: e.target!,
          mapping: Object.keys(edgeMapping).length > 0 ? edgeMapping : ((e.data as { mapping?: Record<string, string> })?.mapping) ?? {},
        };
      }),
    };
    try {
      await putWorkflowGraph(workflowId, payload as Parameters<typeof putWorkflowGraph>[1]);
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
    try {
      const values = await form.validateFields();
      setIsRunning(true);
      setExecutionResult(null); // 清除上次结果，准备新执行
      // 分离工作流输入参数和 UserInput 上下文（带 _ctx_ 前缀）
      const { input, user_input } = splitRunValues(values);
      setLastRunInput(input); // 保存输入，供开始节点查看
      const result = await executeWorkflow(workflowId, { input, user_input });
      setExecutionResult(result);
      // 保持在弹窗内展示结果，不关闭
      void message.success(`执行成功，耗时 ${result.elapsed_ms}ms`);
    } catch (e: unknown) {
      if (e && typeof e === 'object' && 'errorFields' in e) {
        void message.error('请检查表单输入');
      } else {
        void message.error(`执行失败: ${(e as Error).message}`);
      }
    } finally {
      setIsRunning(false);
    }
  }, [workflowId, form]);

  const handleResetRun = useCallback(() => {
    form.resetFields();
    setExecutionResult(null);
    setLastRunInput(null);
  }, [form]);



  return (
    <div>
      {!readonly && (
        <Space style={{ marginBottom: 12 }}>
          <Button onClick={() => setAddOpen(true)}>添加函数节点</Button>
          <Button onClick={onAddAnswer}>添加回答节点</Button>
            <Button type="default" onClick={handleRunClick} disabled={!!cycle}>
              测试
          </Button>
          <Button type="primary" onClick={onSave} disabled={!!cycle}>
            保存
          </Button>
          <Button onClick={onReset}>重置</Button>
        </Space>
      )}
      {/* 运行参数输入弹窗 */}
      <Modal
        title={`${executionResult ? '执行结果' : '运行'}工作流「${workflow?.name || workflow?.identifier || ''}」`}
        open={runModalOpen}
        onCancel={() => setRunModalOpen(false)}
        footer={[
          <Button key="reset" onClick={handleResetRun} disabled={isRunning}>
            重置
          </Button>,
          <Button key="close" onClick={() => setRunModalOpen(false)} disabled={isRunning}>
            关闭
          </Button>,
          <Button key="run" type="primary" onClick={handleRun} loading={isRunning}>
            执行
          </Button>,
        ]}
        width={executionResult ? 1200 : 700}
      >
        <Row gutter={16}>
          {/* 左侧：工作流信息 + 输入参数 + UserInput 上下文 */}
          <Col span={executionResult ? 11 : 24}>
            <Space direction="vertical" style={{ width: '100%' }} size="large">
              {/* 工作流信息 */}
              <div>
                <Text strong>工作流信息：</Text>
                {workflow?.description && (
                  <Paragraph type="secondary" style={{ margin: '8px 0 0 0' }}>
                    {workflow.description}
                  </Paragraph>
                )}
              </div>

              {/* 输入表单 */}
              <div>
                <Text strong style={{ marginBottom: 8, display: 'block' }}>输入参数：</Text>
                {(() => {
                  const fields = parseSchemaProperties(startInputSchema);
                  if (fields.length > 0) {
                    return (
                      <Form form={form} layout="vertical" size="small">
                        {fields.map(({ key, prop }) => renderFormField(key, prop))}
                      </Form>
                    );
                  }
                  return <Text type="secondary">该工作流无输入参数（空 schema）</Text>;
                })()}
              </div>

              {/* UserInput 上下文配置（可选，用于依赖 AgentContext 的内置函数） */}
              <div>
                <div
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 8,
                    marginBottom: 8,
                    cursor: 'pointer',
                  }}
                  onClick={() => setShowContext(!showContext)}
                >
                  <Text strong>UserInput 上下文（可选）：</Text>
                  <Button size="small" type="link">
                    {showContext ? '收起 ▲' : '展开 ▼'}
                  </Button>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    用于依赖用户上下文的函数（如 query_balance 需要 actor_id）
                  </Text>
                </div>
                {showContext && (
                  <div
                    style={{
                      border: '1px solid #d9d9d9',
                      borderRadius: 6,
                      padding: '12px 16px',
                      background: '#fafafa',
                    }}
                  >
                    <Form form={form} layout="vertical" size="small">
                      <Form.Item
                        name="_ctx_raw_text"
                        label={
                          <span>
                            raw_text
                            <Text type="secondary" style={{ marginLeft: 8, fontSize: 12 }}>
                              用户原始输入文本
                            </Text>
                          </span>
                        }
                      >
                        <Input placeholder="例如：帮我查一下余额" />
                      </Form.Item>
                      <Form.Item
                        name="_ctx_actor_id"
                        label={
                          <span>
                            actor_id
                            <Text type="secondary" style={{ marginLeft: 8, fontSize: 12 }}>
                              用户/玩家 ID
                            </Text>
                          </span>
                        }
                      >
                        <Input placeholder="例如：10086" />
                      </Form.Item>
                      <Form.Item
                        name="_ctx_channel"
                        label={
                          <span>
                            channel
                            <Text type="secondary" style={{ marginLeft: 8, fontSize: 12 }}>
                              来源渠道
                            </Text>
                          </span>
                        }
                      >
                        <Input placeholder="例如：weixin / qq" />
                      </Form.Item>
                      <Form.Item
                        name="_ctx_platform"
                        label={
                          <span>
                            platform
                            <Text type="secondary" style={{ marginLeft: 8, fontSize: 12 }}>
                              平台
                            </Text>
                          </span>
                        }
                      >
                        <Input placeholder="例如：ios / android" />
                      </Form.Item>
                      <Form.Item
                        name="_ctx_app_version"
                        label={
                          <span>
                            app_version
                            <Text type="secondary" style={{ marginLeft: 8, fontSize: 12 }}>
                              应用版本
                            </Text>
                          </span>
                        }
                      >
                        <Input placeholder="例如：3.2.1" />
                      </Form.Item>
                    </Form>
                  </div>
                )}
              </div>
            </Space>
          </Col>

          {/* 右侧：执行结果 */}
          {executionResult && (
            <Col span={13}>
              <div
                style={{
                  borderLeft: '1px solid #f0f0f0',
                  paddingLeft: 16,
                  height: '100%',
                }}
              >
                <Text strong style={{ marginBottom: 8, display: 'block' }}>
                  执行结果（耗时 {executionResult.elapsed_ms}ms）：
                </Text>
                <pre
                  style={{
                    background: '#fafafa',
                    border: '1px solid #d9d9d9',
                    borderRadius: 6,
                    padding: 12,
                    fontSize: 12,
                    overflow: 'auto',
                    maxHeight: 'calc(100vh - 360px)',
                    minHeight: 200,
                    margin: 0,
                  }}
                >
                  {JSON.stringify(executionResult, null, 2)}
                </pre>
              </div>
            </Col>
          )}
        </Row>
      </Modal>
      {/* 节点结果查看弹窗 */}
      <Modal
        title={(() => {
          if (!selectedNodeResult) return '节点结果';
          if (selectedNodeResult.nodeKey === 'start' || selectedNodeResult.nodeKey === 'start_node') return '输入参数（开始节点）';
          if (selectedNodeResult.nodeKey === 'end' || selectedNodeResult.nodeKey === 'end_node') return '最终输出（结束节点）';
          return `节点 ${selectedNodeResult.nodeKey} 运行结果`;
        })()}
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
            filterOption={(input, option) => {
              if (!option || option.value === undefined) return false;
              const fn = functions.find((f) => f.id === option.value);
              if (!fn) return false;
              const searchText = `${fn.name} ${fn.identifier} ${fn.description || ''}`.toLowerCase();
              return searchText.includes(input.toLowerCase());
            }}
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
          onUpdateFunctionNode={selectedNode.nodeType === 'function' ? (nodeKey: string, mapping: Record<string, { source: 'upstream' | 'custom'; source_node_key?: string; source_field?: string; custom_value?: string }>) => onUpdateFunctionNode(nodeKey, mapping) : undefined}
          functionInputMapping={selectedNode.inputMapping ?? null}
          allNodes={nodes}
          allEdges={edges}
          allFunctions={functions}
        />
      )}
    </div>
  );
}