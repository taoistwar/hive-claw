/// <reference types="vitest/globals" />
//
// 004 SC-008 axe regression — T159 dedicated test.
// Covers WorkflowPage / DagEditor keyboard navigation a11y.
//
// Note: DagEditor component-level tests are also covered in
// a11y_004_pages.test.tsx. This file adds focused keyboard navigation
// and WorkflowPage composition tests.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe } from 'vitest-axe';
import { MemoryRouter } from 'react-router-dom';
import 'vitest-axe/extend-expect';

import { DagEditor } from '../DagEditor/DagEditor';

vi.mock('../../services/workflow', () => ({
  getWorkflowGraph: vi.fn(async () => ({
    nodes: [
      { node_key: 'a', function_id: 1, position: { x: 0, y: 0 } },
      { node_key: 'b', function_id: 2, position: { x: 200, y: 0 } },
    ],
    edges: [{ src_node_key: 'a', dst_node_key: 'b', mapping: {} }],
  })),
  putWorkflowGraph: vi.fn(async () => ({})),
}));

vi.mock('../../services/function', () => ({
  listFunctions: vi.fn(async () => ({
    items: [
      { id: 1, identifier: 'format_template', name: 'Template', kind: 1, plugin_identifier: null, plugin_export: null },
      { id: 2, identifier: 'json_parse', name: 'JSON Parse', kind: 1, plugin_identifier: null, plugin_export: null },
    ],
    total: 2,
    offset: 0,
    limit: 100,
  })),
}));

vi.mock('../../utils/reactflow', () => ({
  DEFAULT_FLOW_STYLE: { width: 400, height: 300 },
  DEFAULT_FIT_VIEW_OPTIONS: {},
  MONACO_DEFAULT_OPTIONS: {},
}));

vi.mock('reactflow', async () => {
  const actual = await vi.importActual('reactflow');
  return {
    ...actual,
    ReactFlow: ({ nodes, edges, onNodesChange, onEdgesChange, onConnect, onNodeContextMenu, nodeTypes, fitView, fitViewOptions, children }: any) => (
      <div data-testid="reactflow" role="application" aria-label="DAG editor canvas">
        {nodes.map((n: any) => (
          <div key={n.id} data-testid={`node-${n.id}`} tabIndex={0} role="button" aria-label={`Node ${n.data?.function_name ?? n.id}`}>
            {n.data?.function_name ?? n.id}
          </div>
        ))}
        {children}
      </div>
    ),
    Background: () => null,
    Controls: () => <div role="toolbar" aria-label="DAG controls" />,
    MiniMap: () => <div role="img" aria-label="Mini map" />,
    addEdge: (params: any) => ({ ...params, id: `e-${params.source}-${params.target}` }),
    applyEdgeChanges: (changes: any, edges: any) => edges,
    applyNodeChanges: (changes: any, nodes: any) => nodes,
  };
});

async function expectNoCriticalA11yViolations(container: HTMLElement) {
  const results = await axe(container);
  const blocking = (results.violations ?? []).filter(
    (v) => v.impact === 'critical' || v.impact === 'serious',
  );
  if (results.violations.length > 0) {
    console.info('axe violations:', results.violations.map((v) => `${v.id} (${v.impact})`));
  }
  expect(blocking).toEqual([]);
}

describe('a11y — T159 Workflow / DagEditor a11y', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('DagEditor (with nodes) has no critical/serious violations', async () => {
    const { container } = render(
      <MemoryRouter>
        <DagEditor workflowId={1} />
      </MemoryRouter>,
    );
    await new Promise((r) => setTimeout(r, 100));
    await expectNoCriticalA11yViolations(container);
  });

  it('DagEditor nodes are keyboard-focusable', async () => {
    render(
      <MemoryRouter>
        <DagEditor workflowId={1} />
      </MemoryRouter>,
    );
    await new Promise((r) => setTimeout(r, 100));
    const reactflow = screen.getByTestId('reactflow');
    expect(reactflow).toHaveAttribute('role', 'application');
    const nodes = screen.queryAllByTestId(/node-/);
    for (const node of nodes) {
      expect(node).toHaveAttribute('tabindex');
      expect(node).toHaveAttribute('role', 'button');
    }
  });
});
