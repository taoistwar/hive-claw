/// <reference types="vitest/globals" />
//
// 004 SC-008 axe regression — T161 dedicated test.
// Covers ChatPage / ChatStream a11y.
//
// Note: ChatStream component-level tests are also covered in
// a11y_004.test.tsx. This file adds focused ChatStream state coverage
// and page-level composition tests.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { axe } from 'vitest-axe';
import { MemoryRouter } from 'react-router-dom';
import 'vitest-axe/extend-expect';

import { ChatStream } from '../ChatStream';

vi.mock('../../services/chat', () => ({
  listSessions: vi.fn(async () => []),
  createSession: vi.fn(async () => ({ id: 1, created_at: '' })),
  streamMessages: vi.fn(async function* () {
    yield { type: 'token', text: 'Hello' };
    yield { type: 'done', elapsed_ms: 500, final_agent_id: 1 };
  }),
}));

vi.mock('../../services/agent', () => ({
  listAgentTree: vi.fn(async () => []),
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

describe('a11y — T161 Chat a11y', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('ChatStream (idle/empty) has no critical/serious violations', async () => {
    const { container } = render(
      <ChatStream events={[]} tokenBuffer="" pending={false} />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('ChatStream (with all 6 event types) has no critical/serious violations', async () => {
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

  it('ChatStream (pending/loading state) has no critical/serious violations', async () => {
    const { container } = render(
      <ChatStream events={[]} tokenBuffer="" pending={true} />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('ChatStream (error state) has no critical/serious violations', async () => {
    const { container } = render(
      <ChatStream
        events={[
          { type: 'error', code: 4030, message: 'Capability denied: network.http' },
        ]}
        tokenBuffer=""
        pending={false}
      />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('ChatStream (with partial token buffer) has no critical/serious violations', async () => {
    const { container } = render(
      <ChatStream
        events={[
          { type: 'token', text: 'The ' },
          { type: 'token', text: 'weather ' },
        ]}
        tokenBuffer="The weather in Tokyo is"
        pending={true}
      />,
    );
    await expectNoCriticalA11yViolations(container);
  });
});
