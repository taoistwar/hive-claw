/// <reference types="vitest/globals" />
//
// 004 SC-008 axe regression — T160 dedicated test.
// Covers AgentPage / AgentTree / AgentEditor / ModelPresetSelect / CapabilityPicker a11y.
//
// Note: Component-level tests for ModelPresetSelect, AgentTree, and
// CapabilityPicker are also covered in a11y_004.test.tsx. This file adds
// focused AgentEditor and page-level composition tests.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { axe } from 'vitest-axe';
import { MemoryRouter } from 'react-router-dom';
import 'vitest-axe/extend-expect';

import { AgentTree } from '../AgentTree';
import { AgentEditor } from '../AgentEditor';
import { ModelPresetSelect } from '../ModelPresetSelect';
import { CapabilityPicker } from '../CapabilityPicker';

vi.mock('../../services/agent', () => ({
  listAgentTree: vi.fn(async () => [
    {
      id: 1,
      identifier: 'main',
      name: 'Main',
      description: null,
      system_prompt: '',
      parent_agent_id: null,
      depth: 0,
      model_preset: null,
      created_at: '',
      updated_at: '',
      children: [
        {
          id: 2,
          identifier: 'rust-expert',
          name: 'Rust Expert',
          description: null,
          system_prompt: '',
          parent_agent_id: 1,
          depth: 1,
          model_preset: 'code-expert',
          created_at: '',
          updated_at: '',
          children: [],
        },
      ],
    },
  ]),
  listModelPresets: vi.fn(async () => [
    { name: 'cheap-fast', description: 'default preset', is_default: true },
    { name: 'code-expert', description: 'opus', is_default: false },
  ]),
  getAgent: vi.fn(async (id: number) => ({
    id,
    identifier: id === 1 ? 'main' : 'rust-expert',
    name: id === 1 ? 'Main' : 'Rust Expert',
    description: null,
    system_prompt: 'You are a helpful assistant.',
    parent_agent_id: id === 1 ? null : 1,
    depth: id === 1 ? 0 : 1,
    model_preset: id === 1 ? null : 'code-expert',
    tools: [],
    skills: [],
    permissions: [],
    created_at: '',
    updated_at: '2026-01-01T00:00:00Z',
  })),
}));

vi.mock('../../services/capability', () => ({
  listCapabilities: vi.fn(async () => [
    { name: 'time.now', description: 'Current time', is_dangerous: false },
    { name: 'log.emit', description: 'Log message', is_dangerous: false },
    { name: 'network.http', description: 'HTTP request', is_dangerous: true },
  ]),
  getPoolStats: vi.fn(async () => ({
    global: { in_use: 0, idle: 0, created_total: 0, cache_misses: 0, wait_count: 0, reset_failures: 0 },
    per_plugin: [],
  })),
}));

vi.mock('../../services/tool', () => ({
  listTools: vi.fn(async () => ({
    items: [
      { id: 1, identifier: 'format_template', name: 'Template', kind: 1 },
      { id: 2, identifier: 'json_parse', name: 'JSON Parse', kind: 1 },
    ],
    total: 2,
    offset: 0,
    limit: 100,
  })),
}));

vi.mock('../../services/skill', () => ({
  listSkills: vi.fn(async () => ({
    items: [
      { id: 1, identifier: 'code-review', name: 'Code Review', source: 'builtin', content: '', frontmatter: null },
    ],
    total: 1,
    offset: 0,
    limit: 100,
  })),
}));

vi.mock('@monaco-editor/react', () => ({
  default: ({ value, onChange, language, height }: { value: string; onChange: (v: string) => void; language: string; height?: string }) => (
    <textarea
      value={value}
      onChange={(e) => onChange(e.target.value)}
      aria-label={`${language} editor`}
      style={{ width: '100%', height: height || '200px' }}
    />
  ),
}));

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

describe('a11y — T160 Agent a11y', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('AgentTree has no critical/serious violations', async () => {
    const { container } = render(
      <AgentTree
        data={[
          {
            id: 1,
            identifier: 'main',
            name: 'Main',
            description: null,
            system_prompt: '',
            parent_agent_id: null,
            depth: 0,
            model_preset: null,
            created_at: '',
            updated_at: '',
            children: [
              {
                id: 2,
                identifier: 'rust-expert',
                name: 'Rust Expert',
                description: null,
                system_prompt: '',
                parent_agent_id: 1,
                depth: 1,
                model_preset: 'code-expert',
                created_at: '',
                updated_at: '',
                children: [],
              },
            ],
          },
        ]}
      />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('AgentEditor (create mode) has no critical/serious violations', async () => {
    const { container } = render(
      <AgentEditor
        initial={null}
        currentRole={3}
        onSubmit={async () => undefined}
        onCancel={() => undefined}
      />,
    );
    await new Promise((r) => setTimeout(r, 100));
    await expectNoCriticalA11yViolations(container);
  });

  it('AgentEditor (edit mode) has no critical/serious violations', async () => {
    const { container } = render(
      <AgentEditor
        initial={{
          id: 1,
          identifier: 'main',
          name: 'Main',
          description: null,
          system_prompt: 'You are a helpful assistant.',
          parent_agent_id: null,
          depth: 0,
          model_preset: null,
          tools: [],
          skills: [],
          permissions: ['time.now'],
          created_at: '',
          updated_at: '2026-01-01T00:00:00Z',
        }}
        currentRole={3}
        onSubmit={async () => undefined}
        onCancel={() => undefined}
      />,
    );
    await new Promise((r) => setTimeout(r, 100));
    await expectNoCriticalA11yViolations(container);
  });

  it('ModelPresetSelect has no critical/serious violations', async () => {
    const { container } = render(
      <ModelPresetSelect value={null} onChange={() => undefined} />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('CapabilityPicker (System role, dangerous hidden) has no critical/serious violations', async () => {
    const { container } = render(
      <CapabilityPicker value={[]} onChange={() => undefined} currentRole={2} />,
    );
    await new Promise((r) => setTimeout(r, 50));
    await expectNoCriticalA11yViolations(container);
  });
});
