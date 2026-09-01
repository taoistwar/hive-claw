/// <reference types="vitest/globals" />

import type {
  GraphEdge,
  GraphNode,
  WorkflowExecuteResult,
} from '../workflow'

describe('Workflow API TypeScript contract', () => {
  it('accepts null ids for virtual graph entities', () => {
    const node: GraphNode = {
      id: null,
      node_key: 'start',
      node_type: 'start_node',
      function_id: null,
      position: { x: 100, y: 300 },
    }
    const edge: GraphEdge = {
      id: null,
      src_node_key: 'start',
      dst_node_key: 'first',
      mapping: {},
    }

    expect(node.id).toBeNull()
    expect(edge.id).toBeNull()
  })

  it('models public workflow outputs independently from raw node results', () => {
    const result: WorkflowExecuteResult = {
      workflow_id: 7,
      outputs: 'primitive result',
      node_results: { final: { raw: true } },
      node_inputs: {},
      node_agent_contexts: {},
      elapsed_ms: 42,
    }

    expect(result.outputs).toBe('primitive result')
  })
})
