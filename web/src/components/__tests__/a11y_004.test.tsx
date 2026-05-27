/// <reference types="vitest/globals" />
//
// 004 SC-008 axe regression — T157/T158/T159/T160/T161 combined.
// Each new component / page from Agent Runtime is rendered isolated and
// scanned by axe-core; only critical/serious findings fail the build.

import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/react';
import { axe, toHaveNoViolations } from 'vitest-axe';
import { MemoryRouter } from 'react-router-dom';
import 'vitest-axe/extend-expect';

import { PluginFilters } from '../PluginFilters';
import { SchemaEditor } from '../SchemaEditor';
import { ModelPresetSelect } from '../ModelPresetSelect';
import { AgentTree } from '../AgentTree';
import { CapabilityPicker } from '../CapabilityPicker';
import { ChatStream } from '../ChatStream';

expect.extend({ toHaveNoViolations: toHaveNoViolations as never });

// Mock services so isolated components don't fire HTTP
vi.mock('../../services/capability', () => ({
  listCapabilities: vi.fn(async () => [
    { name: 'time.now', description: 'time', is_dangerous: false },
    { name: 'network.http', description: 'http', is_dangerous: true },
  ]),
  getPoolStats: vi.fn(async () => ({
    global: { in_use: 0, idle: 0, created_total: 0, cache_misses: 0, wait_count: 0, reset_failures: 0 },
    per_plugin: [],
  })),
}));
vi.mock('../../services/agent', () => ({
  listModelPresets: vi.fn(async () => [
    { name: 'cheap-fast', description: 'default preset', is_default: true },
    { name: 'code-expert', description: 'opus', is_default: false },
  ]),
}));

async function expectNoCriticalA11yViolations(container: HTMLElement) {
  const results = await axe(container);
  const blocking = (results.violations ?? []).filter(
    (v) => v.impact === 'critical' || v.impact === 'serious',
  );
  if (results.violations.length > 0) {
    // eslint-disable-next-line no-console
    console.info('axe violations:', results.violations.map((v) => `${v.id} (${v.impact})`));
  }
  expect(blocking).toEqual([]);
}

describe('a11y — 004 SC-008 gate', () => {
  it('T157 PluginFilters has no critical/serious violations', async () => {
    const { container } = render(
      <MemoryRouter>
        <PluginFilters value={{}} onChange={() => undefined} />
      </MemoryRouter>,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('T158 SchemaEditor has no critical/serious violations', async () => {
    const { container } = render(
      <SchemaEditor value={{ type: 'object' }} onChange={() => undefined} label="Input schema" />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('T160 ModelPresetSelect has no critical/serious violations', async () => {
    const { container } = render(
      <ModelPresetSelect value={null} onChange={() => undefined} />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('T160 AgentTree has no critical/serious violations', async () => {
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
                identifier: 'sub',
                name: 'Sub',
                description: null,
                system_prompt: '',
                parent_agent_id: 1,
                depth: 1,
                model_preset: 'cheap-fast',
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

  it('T160 CapabilityPicker has no critical/serious violations (Super role)', async () => {
    const { container } = render(
      <CapabilityPicker value={['time.now']} onChange={() => undefined} currentRole={3} />,
    );
    // Wait microtask for the mocked async listCapabilities
    await new Promise((r) => setTimeout(r, 50));
    await expectNoCriticalA11yViolations(container);
  });

  it('T161 ChatStream (idle) has no critical/serious violations', async () => {
    const { container } = render(
      <ChatStream events={[]} tokenBuffer="" pending={false} />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('T161 ChatStream (with events) has no critical/serious violations', async () => {
    const { container } = render(
      <ChatStream
        events={[
          { type: 'token', text: 'Hello ' },
          {
            type: 'tool_call',
            tool_call_id: 'tc1',
            name: 'weather.lookup',
            args: { city: 'Tokyo' },
          },
          {
            type: 'tool_result',
            tool_call_id: 'tc1',
            result: { temp_c: 21, summary: 'Sunny' },
          },
          { type: 'routed', agent_id: 2, agent_identifier: 'rust-expert' },
          { type: 'fallback_used', from: 'gpt-4o-mini', to: 'claude-haiku', reason: '5xx' },
          { type: 'done', elapsed_ms: 1842, final_agent_id: 2 },
        ]}
        tokenBuffer="Hello world"
        pending={false}
      />,
    );
    await expectNoCriticalA11yViolations(container);
  });
});
