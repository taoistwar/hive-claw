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
  Tabs,
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
  type InputSource,
  type InputSpec,
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
import { CollapsibleJsonView } from '../CollapsibleJsonView';

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
  start_description?: string | null;
  output_schema?: Record<string, unknown> | null;
  end_description?: string | null;
  node_config?: AnswerNodeConfig | null;
  /** Structured input mapping (function_node & generate_answer_node). */
  input_mapping?: InputSpec;
  execution_result?: unknown;
}

interface DagEditorCache {
  nodes: Node<NodeData>[];
  edges: Edge[];
  functions?: FunctionItem[];
  executionResult?: WorkflowExecuteResult | null;
  lastRunInput?: Record<string, unknown> | null;
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
    startDescription?: string | null;
    outputSchema?: Record<string, unknown> | null;
    endDescription?: string | null;
    answerConfig?: AnswerNodeConfig | null;
    inputMapping?: InputSpec | null;
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
  const cacheWriteEnabledRef = useRef(false);

  const STORAGE_KEY = `dag_editor_v3_${workflowId}`;
  const LEGACY_STORAGE_KEY = `dag_editor_v2_${workflowId}`;

  const fetchGraph = useCallback(async (skipCache = false) => {
    // Do not let state from another workflow, or an unverified v2 draft, be
    // auto-persisted under this workflow's v3 key while the refresh is pending.
    cacheWriteEnabledRef.current = false;
    let legacyCache: DagEditorCache | null = null;
    if (!skipCache) {
      try {
        const cached = localStorage.getItem(STORAGE_KEY);
        if (cached) {
          const {
            nodes: cachedNodes,
            edges: cachedEdges,
            executionResult: cachedResult,
            lastRunInput: cachedInput,
          } = JSON.parse(cached) as DagEditorCache;
          cacheWriteEnabledRef.current = true;
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

        const legacy = localStorage.getItem(LEGACY_STORAGE_KEY);
        if (legacy) {
          legacyCache = JSON.parse(legacy) as DagEditorCache;
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

      if (legacyCache) {
        const startNode = graph.nodes.find(
          (node) => node.node_type === 'start_node' || node.node_key === 'start',
        );
        const endNode = graph.nodes.find(
          (node) => node.node_type === 'end_node' || node.node_key === 'end',
        );
        const migratedNodes = legacyCache.nodes.map((node) => {
          const data = { ...node.data };
          const isStart = data.node_type === 'start_node' || node.id === 'start';
          const isEnd = data.node_type === 'end_node' || node.id === 'end';
          if (isStart && data.start_description === undefined) {
            data.start_description =
              startNode?.position?.start_description ?? graph.workflow.start_description ?? null;
          }
          if (isEnd && data.end_description === undefined) {
            data.end_description =
              endNode?.position?.end_description ?? graph.workflow.end_description ?? null;
          }
          return { ...node, data };
        });
        const migratedCache: DagEditorCache = {
          ...legacyCache,
          nodes: migratedNodes,
          edges: legacyCache.edges ?? [],
          functions: fnList.items,
        };

        // Only retire the legacy draft after the enriched v3 copy is durable.
        localStorage.setItem(STORAGE_KEY, JSON.stringify(migratedCache));
        localStorage.removeItem(LEGACY_STORAGE_KEY);
        cacheWriteEnabledRef.current = true;
        setNodes(migratedNodes);
        setEdges(migratedCache.edges);
        if (legacyCache.executionResult) setExecutionResult(legacyCache.executionResult);
        if (legacyCache.lastRunInput) setLastRunInput(legacyCache.lastRunInput);
        return;
      }

      const fnMap = new Map(fnList.items.map((f) => [f.id, f]));
      cacheWriteEnabledRef.current = true;
      setNodes(
        graph.nodes.map((n: GraphNode, i: number) => {
          const isStart = n.node_type === 'start_node' || n.node_key === 'start';
          const isEnd = n.node_type === 'end_node' || n.node_key === 'end';
          const isAnswer = n.node_type === 'generate_answer_node';
          const isFunctionNode = !isStart && !isEnd && !isAnswer;
          const cfg = n.node_config as Record<string, unknown> | null | undefined;
          // 恢复结构化 InputSpec：function_node 和 generate_answer_node 都用 input_mapping
          const inputMapping = (isFunctionNode || isAnswer) && cfg?.input_mapping
            ? (cfg.input_mapping as InputSpec)
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
              start_description: isStart
                ? (n.position?.start_description ?? graph.workflow.start_description ?? null)
                : undefined,
              output_schema: isEnd ? (graph.workflow.output_schema ?? null) : undefined,
              end_description: isEnd
                ? (n.position?.end_description ?? graph.workflow.end_description ?? null)
                : undefined,
              node_config: isAnswer ? ((n.node_config ?? null) as AnswerNodeConfig | null) : undefined,
              input_mapping: inputMapping,
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
      if (legacyCache) {
        setNodes(legacyCache.nodes);
        setEdges(legacyCache.edges ?? []);
        if (legacyCache.executionResult) setExecutionResult(legacyCache.executionResult);
        if (legacyCache.lastRunInput) setLastRunInput(legacyCache.lastRunInput);
      }
      void message.error(`加载失败：${(e as Error).message}`);
    }
  }, [workflowId, STORAGE_KEY, LEGACY_STORAGE_KEY]);

  useEffect(() => {
    void fetchGraph();
  }, [fetchGraph]);

  // 自动同步到 localStorage
  useEffect(() => {
    if (!cacheWriteEnabledRef.current || nodes.length === 0) return;
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
      startDescription: isStart ? node.data.start_description : undefined,
      outputSchema: isEnd ? node.data.output_schema : undefined,
      endDescription: isEnd ? node.data.end_description : undefined,
      answerConfig: isAnswer ? (node.data.node_config ?? null) : undefined,
      inputMapping: (isFunction || isAnswer) ? (node.data.input_mapping ?? null) : undefined,
    });
    setDetailDrawerOpen(true);
  }, []);

  const onUpdateStartNode = useCallback((vars: Record<string, unknown>, startDescription: string) => {
    setNodes((nds) =>
      nds.map((n) =>
        n.id === 'start'
          ? {
              ...n,
              data: {
                ...n.data,
                input_schema: vars,
                start_description: startDescription,
              },
            }
          : n,
      ),
    );
    setSelectedNode((prev) =>
      prev?.nodeType === 'start'
        ? { ...prev, inputSchema: vars, startDescription }
        : prev,
    );
    void message.success('起始节点配置已更新');
  }, []);

  const onUpdateEndNode = useCallback((vars: Record<string, unknown>, endDescription: string) => {
    setNodes((nds) =>
      nds.map((n) =>
        n.id === 'end'
          ? {
              ...n,
              data: {
                ...n.data,
                output_schema: vars,
                end_description: endDescription,
              },
            }
          : n,
      ),
    );
    setSelectedNode((prev) =>
      prev?.nodeType === 'end'
        ? { ...prev, outputSchema: vars, endDescription }
        : prev,
    );
    void message.success('结束节点配置已更新');
  }, []);

  const onUpdateAnswerNode = useCallback((nodeKey: string, config: AnswerNodeConfig) => {
    // Answer node uses `input_mapping` (structured) under node_config
    const inputMapping = config.input_mapping;
    setNodes((nds) =>
      nds.map((n) =>
        n.id === nodeKey
          ? { ...n, data: { ...n.data, node_config: config, input_mapping: inputMapping } }
          : n,
      ),
    );
    // Update selectedNode as well so it reflects immediately
    setSelectedNode((prev) =>
      prev?.nodeKey === nodeKey
        ? { ...prev, answerConfig: config, inputMapping: inputMapping ?? null }
        : prev
    );
    void message.success('回答节点配置已更新');
  }, []);

  const onUpdateFunctionNode = useCallback(
    (nodeKey: string, inputMapping: InputSpec) => {
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

        // 结束节点只展示服务端公开的工作流 outputs；node_results 是诊断信息。
        if (isEnd && executionResult) {
          node = {
            ...node,
            data: { ...node.data, execution_result: executionResult.outputs },
          };
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
    [nodes, cycle, executionResult, lastRunInput],
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
        setSelectedNodeResult({ nodeKey, result: executionResult.outputs });
        setResultModalOpen(true);
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
  }, [executionResult, lastRunInput]);

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
            system_prompt: '你是一个智能助手，请根据以下内容回答用户问题：\n{query}',
            history_window: 0,
            input_mapping: {},
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
        const isFunction = !isStart && !isEnd && !isAnswer;
        // node_config 基础：函数节点写 input_mapping；回答节点合并 input_mapping
        let nodeConfig: Record<string, unknown> | undefined;
        if (isFunction) {
          nodeConfig = n.data.input_mapping ? { input_mapping: n.data.input_mapping } : undefined;
        } else if (isAnswer) {
          const base = (n.data.node_config && typeof n.data.node_config === 'object')
            ? (n.data.node_config as unknown as Record<string, unknown>)
            : {};
          nodeConfig = { ...base, input_mapping: n.data.input_mapping ?? {} };
        }
        return {
          node_key: n.data.node_key,
          node_type: isStart ? ('start_node' as const) : isEnd ? ('end_node' as const) : isAnswer ? ('generate_answer_node' as const) : ('function_node' as const),
          function_id: isStart || isEnd || isAnswer ? null : n.data.function_id,
          position: {
            x: n.position.x,
            y: n.position.y,
            ...(isStart && { input_schema: n.data.input_schema ?? { type: 'object', properties: {} } }),
            ...(isStart && n.data.start_description !== undefined && {
              start_description: n.data.start_description,
            }),
            ...(isEnd && n.data.output_schema && { output_schema: n.data.output_schema }),
            ...(isEnd && n.data.end_description !== undefined && {
              end_description: n.data.end_description,
            }),
          },
          ...(nodeConfig && { node_config: nodeConfig }),
        };
      }),
      edges: edges.map((e) => {
        // Edge mapping column is deprecated; the structured spec is stored on
        // the dst node's node_config.input_mapping (function_node and
        // generate_answer_node share the same field). Edge row only carries topology.
        const prior = (e.data as { mapping?: Record<string, string> } | undefined)?.mapping;
        return {
          src_node_key: e.source!,
          dst_node_key: e.target!,
          mapping: prior ?? {},
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
    localStorage.removeItem(LEGACY_STORAGE_KEY);
    void fetchGraph(true);
    void message.success('已重置为服务器保存的版本');
  }, [STORAGE_KEY, LEGACY_STORAGE_KEY, fetchGraph]);

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
                        name="_ctx_client_type"
                        label={
                          <span>
                            client_type
                            <Text type="secondary" style={{ marginLeft: 8, fontSize: 12 }}>
                              平台
                            </Text>
                          </span>
                        }
                      >
                        <Input placeholder="例如：ios / android" />
                      </Form.Item>
                      <Form.Item
                        name="_ctx_client_version"
                        label={
                          <span>
                            client_version
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
          {executionResult && (() => {
            const { agent_context: agentContext, ...resultFields } = executionResult as unknown as Record<string, unknown> & { agent_context?: unknown };
            const hasAgentCtx = agentContext !== undefined && agentContext !== null;
            return (
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
                  {/* 结果区块（去除 agent_context） */}
                  <div style={{ marginBottom: 4 }}>
                    <Text strong type="secondary" style={{ fontSize: 13 }}>结果</Text>
                  </div>
                  <CollapsibleJsonView
                    data={resultFields}
                    initialDepth={2}
                    maxHeight="calc(100vh - 460px)"
                    title="结果 JSON"
                  />
                  {/* AgentContext 区块 */}
                  <div style={{ margin: '12px 0 4px 0' }}>
                    <Text strong type="secondary" style={{ fontSize: 13 }}>AgentContext</Text>
                  </div>
                  <CollapsibleJsonView
                    data={agentContext}
                    initialDepth={2}
                    maxHeight="calc(100vh - 460px)"
                    title="AgentContext 快照"
                    emptyText={hasAgentCtx ? 'null' : '本次执行无 AgentContext 数据'}
                  />
                </div>
              </Col>
            );
          })()}
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
        {selectedNodeResult && (() => {
          const nodeInput = executionResult?.node_inputs?.[selectedNodeResult.nodeKey];
          const agentCtx = executionResult?.node_agent_contexts?.[selectedNodeResult.nodeKey];
          const items = [];
          // 节点输出（默认）
          items.push({
            key: 'output',
            label: '节点输出',
            children: (
              <CollapsibleJsonView
                data={selectedNodeResult.result}
                initialDepth={3}
                maxHeight={500}
                title="节点输出 JSON"
              />
            ),
          });
          // 节点输入
          items.push({
            key: 'input',
            label: '节点输入',
            children: nodeInput && typeof nodeInput === 'object' && Object.keys(nodeInput as object).length > 0 ? (
              <CollapsibleJsonView
                data={nodeInput}
                initialDepth={3}
                maxHeight={500}
                title="节点输入 JSON"
              />
            ) : (
              <Text type="secondary">（无输入）</Text>
            ),
          });
          // AgentContext
          if (agentCtx) {
            items.push({
              key: 'agentctx',
              label: 'AgentContext',
              children: (
                <CollapsibleJsonView
                  data={agentCtx}
                  initialDepth={2}
                  maxHeight={500}
                  title="AgentContext 快照"
                />
              ),
            });
          }
          return <Tabs defaultActiveKey="output" items={items} />;
        })()}
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
          startDescription={selectedNode.startDescription}
          outputSchema={selectedNode.outputSchema}
          endDescription={selectedNode.endDescription}
          answerConfig={selectedNode.answerConfig}
          onUpdateStartNode={onUpdateStartNode}
          onUpdateEndNode={onUpdateEndNode}
          onUpdateAnswerNode={selectedNode.nodeType === 'generate_answer' ? (config: AnswerNodeConfig) => onUpdateAnswerNode(selectedNode.nodeKey, config) : undefined}
          onUpdateFunctionNode={selectedNode.nodeType === 'function' ? (nodeKey: string, mapping: InputSpec) => onUpdateFunctionNode(nodeKey, mapping) : undefined}
          functionInputMapping={selectedNode.inputMapping ?? null}
          allNodes={nodes}
          allEdges={edges}
          allFunctions={functions}
        />
      )}
    </div>
  );
}
