/// <reference types="vitest/globals" />

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { message } from 'antd'

import { DagEditor } from '../DagEditor/DagEditor'
import { EndNode } from '../DagEditor/EndNode'
import { NodeDetailDrawer } from '../DagEditor/NodeDetailDrawer'
import { StartNode } from '../DagEditor/StartNode'

const workflowMocks = vi.hoisted(() => ({
  getWorkflowGraph: vi.fn(),
  putWorkflowGraph: vi.fn(),
}))

vi.mock('../../services/workflow', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../services/workflow')>()
  return {
    ...actual,
    getWorkflowGraph: workflowMocks.getWorkflowGraph,
    putWorkflowGraph: workflowMocks.putWorkflowGraph,
  }
})

vi.mock('../../services/function', () => ({
  listFunctions: vi.fn(async () => ({ items: [], total: 0, offset: 0, limit: 500 })),
}))

vi.mock('../../utils/reactflow', () => ({
  DEFAULT_FLOW_STYLE: { width: 800, height: 600 },
  DEFAULT_FIT_VIEW_OPTIONS: {},
}))

vi.mock('reactflow', () => ({
  ReactFlow: ({ children }: { children?: React.ReactNode }) => (
    <div data-testid="reactflow">{children}</div>
  ),
  ReactFlowProvider: ({ children }: { children?: React.ReactNode }) => <>{children}</>,
  MiniMap: () => null,
  Controls: () => null,
  Background: () => null,
  Handle: () => null,
  Position: { Top: 'top', Bottom: 'bottom' },
  MarkerType: { Arrow: 'arrow' },
  addEdge: (params: unknown, edges: unknown[]) => [...edges, params],
  applyNodeChanges: (_changes: unknown[], nodes: unknown[]) => nodes,
  applyEdgeChanges: (_changes: unknown[], edges: unknown[]) => edges,
}))

describe('Workflow end_description', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
    localStorage.clear()
    workflowMocks.getWorkflowGraph.mockReset()
    workflowMocks.putWorkflowGraph.mockReset()
    workflowMocks.putWorkflowGraph.mockResolvedValue({})
    vi.spyOn(message, 'success').mockImplementation(() => undefined as never)
    vi.spyOn(message, 'warning').mockImplementation(() => undefined as never)
    vi.spyOn(message, 'error').mockImplementation(() => undefined as never)
  })

  it('shows the closing description on the EndNode', () => {
    render(
      <EndNode
        id="end"
        type="end"
        data={{
          node_key: 'end',
          output_schema: { type: 'object', properties: {} },
          end_description: '报告生成完毕',
        }}
        dragging={false}
        isConnectable
        selected={false}
        xPos={0}
        yPos={0}
        zIndex={0}
      />
    )

    expect(screen.getByText('报告生成完毕')).toBeInTheDocument()
  })

  it('shows the opening description on the StartNode', () => {
    render(
      <StartNode
        id="start"
        type="start"
        data={{
          node_key: 'start',
          input_schema: { type: 'object', properties: {} },
          start_description: '请描述报告主题',
        }}
        dragging={false}
        isConnectable
        selected={false}
        xPos={0}
        yPos={0}
        zIndex={0}
      />
    )

    expect(screen.getByText('请描述报告主题')).toBeInTheDocument()
  })

  it('loads and edits the closing description in NodeDetailDrawer', async () => {
    const onUpdate = vi.fn()
    render(
      <NodeDetailDrawer
        open
        onClose={vi.fn()}
        nodeType="end"
        nodeKey="end"
        outputSchema={{ type: 'object', properties: {} }}
        endDescription="报告生成完毕"
        onUpdateEndNode={onUpdate}
      />
    )

    const input = screen.getByLabelText('结束描述')
    expect(input).toHaveValue('报告生成完毕')
    fireEvent.change(input, { target: { value: '请下载最终报告' } })
    await userEvent.click(screen.getByRole('button', { name: '保存配置' }))

    expect(onUpdate).toHaveBeenCalledWith(
      { type: 'object', properties: {}, required: [] },
      '请下载最终报告'
    )
  })

  it('loads and edits the opening description in NodeDetailDrawer', async () => {
    const onUpdate = vi.fn()
    render(
      <NodeDetailDrawer
        open
        onClose={vi.fn()}
        nodeType="start"
        nodeKey="start"
        inputSchema={{ type: 'object', properties: {} }}
        startDescription="请描述报告主题"
        onUpdateStartNode={onUpdate}
      />
    )

    const input = screen.getByLabelText('开始描述')
    expect(input).toHaveValue('请描述报告主题')
    fireEvent.change(input, { target: { value: '请输入报告主题和范围' } })
    await userEvent.click(screen.getByRole('button', { name: '保存配置' }))

    expect(onUpdate).toHaveBeenCalledWith(
      { type: 'object', properties: {}, required: [] },
      '请输入报告主题和范围'
    )
  })

  it('restores and serializes end_description through the virtual end node position', async () => {
    workflowMocks.getWorkflowGraph.mockResolvedValue({
      workflow: {
        id: 7,
        identifier: 'daily-report',
        name: 'Daily report',
        description: null,
        timeout_ms: 33_000,
        category_id: null,
        input_schema: { type: 'object', properties: {} },
        start_description: 'metadata 开始描述',
        output_schema: {
          type: 'object',
          properties: { report: { type: 'string' } },
        },
        end_description: 'metadata 结束描述',
        required_capabilities: [],
        created_at: '2026-07-24T00:00:00Z',
        updated_at: '2026-07-24T00:00:00Z',
      },
      nodes: [
        {
          node_key: 'start',
          node_type: 'start_node',
          function_id: null,
          position: {
            x: 100,
            y: 100,
            input_schema: { type: 'object', properties: {} },
            start_description: '虚拟开始描述',
          },
        },
        {
          node_key: 'end',
          node_type: 'end_node',
          function_id: null,
          position: {
            x: 100,
            y: 500,
            output_schema: {
              type: 'object',
              properties: { report: { type: 'string' } },
            },
            end_description: '虚拟结束描述',
          },
        },
      ],
      edges: [],
    })

    render(<DagEditor workflowId={7} />)

    await waitFor(() => expect(workflowMocks.getWorkflowGraph).toHaveBeenCalledWith(7))
    await userEvent.click(screen.getByRole('button', { name: /保\s*存/ }))

    await waitFor(() => expect(workflowMocks.putWorkflowGraph).toHaveBeenCalledTimes(1))
    const [, payload] = workflowMocks.putWorkflowGraph.mock.calls[0]
    const startNode = payload.nodes.find((node: { node_key: string }) => node.node_key === 'start')
    const endNode = payload.nodes.find((node: { node_key: string }) => node.node_key === 'end')
    expect(startNode.position.start_description).toBe('虚拟开始描述')
    expect(endNode.position.end_description).toBe('虚拟结束描述')
  })

  it('falls back to workflow metadata when virtual node descriptions are absent', async () => {
    workflowMocks.getWorkflowGraph.mockResolvedValue({
      workflow: {
        id: 8,
        identifier: 'fallback-report',
        name: 'Fallback report',
        description: null,
        timeout_ms: 33_000,
        category_id: null,
        input_schema: { type: 'object', properties: {} },
        start_description: 'metadata 开始描述',
        output_schema: { type: 'object', properties: {} },
        end_description: 'metadata 结束描述',
        required_capabilities: [],
        created_at: '2026-07-24T00:00:00Z',
        updated_at: '2026-07-24T00:00:00Z',
      },
      nodes: [
        {
          node_key: 'start',
          node_type: 'start_node',
          function_id: null,
          position: {
            x: 100,
            y: 100,
            input_schema: { type: 'object', properties: {} },
          },
        },
        {
          node_key: 'end',
          node_type: 'end_node',
          function_id: null,
          position: {
            x: 100,
            y: 500,
            output_schema: { type: 'object', properties: {} },
          },
        },
      ],
      edges: [],
    })

    render(<DagEditor workflowId={8} />)

    await waitFor(() => expect(workflowMocks.getWorkflowGraph).toHaveBeenCalledWith(8))
    await userEvent.click(screen.getByRole('button', { name: /保\s*存/ }))

    await waitFor(() => expect(workflowMocks.putWorkflowGraph).toHaveBeenCalledTimes(1))
    const [, payload] = workflowMocks.putWorkflowGraph.mock.calls[0]
    const startNode = payload.nodes.find((node: { node_key: string }) => node.node_key === 'start')
    const endNode = payload.nodes.find((node: { node_key: string }) => node.node_key === 'end')
    expect(startNode.position.start_description).toBe('metadata 开始描述')
    expect(endNode.position.end_description).toBe('metadata 结束描述')
  })

  it('migrates a v2 draft without losing local graph edits and fills missing descriptions', async () => {
    localStorage.setItem(
      'dag_editor_v2_9',
      JSON.stringify({
        nodes: [
          {
            id: 'start',
            type: 'start',
            position: { x: 10, y: 20 },
            data: {
              node_key: 'start',
              node_type: 'start_node',
              input_schema: { type: 'object', properties: {} },
            },
          },
          {
            id: 'local-draft',
            type: 'custom',
            position: { x: 30, y: 40 },
            data: {
              node_key: 'local-draft',
              node_type: 'function_node',
              function_id: 11,
              function_name: 'Local draft node',
            },
          },
          {
            id: 'end',
            type: 'end',
            position: { x: 50, y: 60 },
            data: {
              node_key: 'end',
              node_type: 'end_node',
              output_schema: { type: 'object', properties: {} },
              end_description: '本地结束草稿',
            },
          },
        ],
        edges: [
          {
            id: 'local-edge',
            source: 'local-draft',
            target: 'end',
            data: { mapping: {} },
          },
        ],
      })
    )
    workflowMocks.getWorkflowGraph.mockResolvedValue({
      workflow: {
        id: 9,
        identifier: 'cached-report',
        name: 'Cached report',
        description: null,
        timeout_ms: 33_000,
        category_id: null,
        input_schema: { type: 'object', properties: {} },
        start_description: 'metadata 开始描述',
        output_schema: { type: 'object', properties: {} },
        end_description: 'metadata 结束描述',
        required_capabilities: [],
        created_at: '2026-07-24T00:00:00Z',
        updated_at: '2026-07-24T00:00:00Z',
      },
      nodes: [
        {
          id: null,
          node_key: 'start',
          node_type: 'start_node',
          function_id: null,
          position: {
            x: 100,
            y: 100,
            input_schema: { type: 'object', properties: {} },
            start_description: '服务端开始描述',
          },
        },
        {
          id: null,
          node_key: 'end',
          node_type: 'end_node',
          function_id: null,
          position: {
            x: 100,
            y: 500,
            output_schema: { type: 'object', properties: {} },
            end_description: '服务端结束描述',
          },
        },
      ],
      edges: [],
    })

    render(<DagEditor workflowId={9} />)

    await waitFor(() => expect(workflowMocks.getWorkflowGraph).toHaveBeenCalledWith(9))
    await userEvent.click(screen.getByRole('button', { name: /保\s*存/ }))
    await waitFor(() => expect(workflowMocks.putWorkflowGraph).toHaveBeenCalledTimes(1))

    const [, payload] = workflowMocks.putWorkflowGraph.mock.calls[0]
    expect(payload.nodes.some((node: { node_key: string }) => node.node_key === 'local-draft')).toBe(
      true
    )
    const startNode = payload.nodes.find((node: { node_key: string }) => node.node_key === 'start')
    const endNode = payload.nodes.find((node: { node_key: string }) => node.node_key === 'end')
    expect(startNode.position.start_description).toBe('服务端开始描述')
    expect(endNode.position.end_description).toBe('本地结束草稿')
    expect(localStorage.getItem('dag_editor_v2_9')).toBeNull()
    expect(localStorage.getItem('dag_editor_v3_9')).not.toBeNull()
  })

  it('keeps the v2 draft retryable when the migration graph refresh fails', async () => {
    const legacyDraft = {
      nodes: [
        {
          id: 'start',
          type: 'start',
          position: { x: 10, y: 20 },
          data: { node_key: 'start', node_type: 'start_node' },
        },
        {
          id: 'end',
          type: 'end',
          position: { x: 30, y: 40 },
          data: { node_key: 'end', node_type: 'end_node' },
        },
      ],
      edges: [],
    }
    localStorage.setItem('dag_editor_v2_10', JSON.stringify(legacyDraft))
    workflowMocks.getWorkflowGraph.mockRejectedValue(new Error('graph unavailable'))

    render(<DagEditor workflowId={10} />)

    await waitFor(() => expect(workflowMocks.getWorkflowGraph).toHaveBeenCalledWith(10))
    await waitFor(() => expect(message.error).toHaveBeenCalled())
    expect(localStorage.getItem('dag_editor_v2_10')).toBe(JSON.stringify(legacyDraft))
    expect(localStorage.getItem('dag_editor_v3_10')).toBeNull()
  })

  it.each([
    {
      name: 'primitive',
      workflowId: 11,
      outputs: 'primitive result',
      expectedText: /primitive result/,
    },
    {
      name: 'multi-terminal',
      workflowId: 12,
      outputs: { alpha: { answer: 'a' }, zeta: { answer: 'z' } },
      expectedText: '"alpha"',
    },
  ])('shows $name public outputs directly on the End node', async ({
    workflowId,
    outputs,
    expectedText,
  }) => {
    localStorage.setItem(
      `dag_editor_v3_${workflowId}`,
      JSON.stringify({
        nodes: [
          {
            id: 'start',
            type: 'start',
            position: { x: 10, y: 20 },
            data: { node_key: 'start', node_type: 'start_node' },
          },
          {
            id: 'end',
            type: 'end',
            position: { x: 30, y: 40 },
            data: { node_key: 'end', node_type: 'end_node' },
          },
        ],
        edges: [],
        executionResult: {
          workflow_id: workflowId,
          outputs,
          node_results: { final: { wrong: 'diagnostic result' } },
          node_inputs: {},
          node_agent_contexts: {},
          elapsed_ms: 7,
          agent_context: null,
        },
      })
    )

    render(<DagEditor workflowId={workflowId} />)
    fireEvent(
      window,
      new CustomEvent('node-view-result', {
        detail: { nodeKey: 'end' },
      })
    )

    expect(await screen.findByText(expectedText)).toBeInTheDocument()
    expect(screen.queryByText(/diagnostic result/)).not.toBeInTheDocument()
  })
})
