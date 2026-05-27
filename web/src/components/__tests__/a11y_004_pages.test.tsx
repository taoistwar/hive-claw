/// <reference types="vitest/globals" />
//
// 004 SC-008 a11y regression — T159/T158 + Page components (T139).
// Covers the remaining 5 components/pages not in a11y_004.test.tsx:
//   - DagEditor (T159)
//   - SkillMarkdownEditor (T158 continued)
//   - FunctionPage (T158)
//   - CategoryPage (T139)
//   - TagPage (T139)
//
// Note: PluginPage/WorkflowPage/AgentPage/ToolPage/SkillPage are higher-order
// page components that wire multiple sub-components + async data fetching.
// They are tested here via their **renderable sub-components only** in
// isolation, consistent with the approach in a11y_004.test.tsx.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/react';
import { axe, toHaveNoViolations } from 'vitest-axe';
import { MemoryRouter } from 'react-router-dom';
import 'vitest-axe/extend-expect';

import { DagEditor } from '../DagEditor/DagEditor';
import { SkillMarkdownEditor } from '../SkillMarkdownEditor';
import FunctionPage from '../../pages/FunctionPage';

expect.extend({ toHaveNoViolations: toHaveNoViolations as never });

// Mock services to avoid HTTP in isolated component tests
vi.mock('../../services/workflow', () => ({
  getWorkflowGraph: vi.fn(async () => ({ nodes: [], edges: [] })),
  putWorkflowGraph: vi.fn(async () => ({})),
}));

vi.mock('../../services/function', () => ({
  listFunctions: vi.fn(async () => ({
    items: [
      { id: 1, identifier: 'format.template', name: 'Template', kind: 1, plugin_identifier: null, plugin_export: null },
      { id: 2, identifier: 'json.parse', name: 'JSON Parse', kind: 1, plugin_identifier: null, plugin_export: null },
      { id: 3, identifier: 'fn-custom', name: 'Custom Fn', kind: 2, plugin_identifier: 'my-plugin', plugin_export: 'handle' },
    ],
    total: 3,
    offset: 0,
    limit: 20,
  })),
}));

vi.mock('../../services/category', () => ({
  listCategories: vi.fn(async () => [
    { id: 1, parent_id: null, slug: 'coding', name: 'Coding', description: null, created_at: '' },
    { id: 2, parent_id: 1, slug: 'rust', name: 'Rust', description: null, created_at: '' },
  ]),
}));

vi.mock('../../services/tag', () => ({
  listTags: vi.fn(async () => [
    { id: 1, name: 'network', description: null, created_at: '' },
    { id: 2, name: 'data', description: null, created_at: '' },
  ]),
}));

vi.mock('../../services/plugin', () => ({
  listPlugins: vi.fn(async () => ({ items: [], total: 0, offset: 0, limit: 20 })),
}));

vi.mock('../../services/agent', () => ({
  listAgentTree: vi.fn(async () => []),
  listModelPresets: vi.fn(async () => []),
}));

vi.mock('../../services/tool', () => ({
  listTools: vi.fn(async () => ({ items: [], total: 0, offset: 0, limit: 20 })),
}));

vi.mock('../../services/skill', () => ({
  listSkills: vi.fn(async () => ({ items: [], total: 0, offset: 0, limit: 20 })),
}));

vi.mock('../../hooks/useAuth', () => ({
  useAuth: () => ({ user: { id: 1, role: 3, phone: '13900000000', nickname: 'super' } }),
}));

vi.mock('../../utils/reactflow', () => ({
  DEFAULT_FLOW_STYLE: { width: 400, height: 300 },
  DEFAULT_FIT_VIEW_OPTIONS: {},
  MONACO_DEFAULT_OPTIONS: {},
}));

vi.mock('@monaco-editor/react', () => ({
  default: ({ value, onChange, language }: { value: string; onChange: (v: string | undefined) => void; language: string }) => (
    <textarea
      data-language={language}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      aria-label={`${language} editor`}
      style={{ width: '100%', height: '200px' }}
    />
  ),
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

describe('a11y — 004 SC-008 gate (remaining components)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('T159 DagEditor (idle) has no critical/serious violations', async () => {
    const { container } = render(
      <MemoryRouter>
        <DagEditor workflowId={1} />
      </MemoryRouter>,
    );
    await new Promise((r) => setTimeout(r, 100));
    await expectNoCriticalA11yViolations(container);
  });

  it('T158 SkillMarkdownEditor (empty) has no critical/serious violations', async () => {
    const { container } = render(
      <SkillMarkdownEditor
        content=""
        frontmatter={null}
        onContentChange={() => undefined}
        onFrontmatterChange={() => undefined}
      />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('T158 SkillMarkdownEditor (with content) has no critical/serious violations', async () => {
    const { container } = render(
      <SkillMarkdownEditor
        content="# Hello\n\nSome **markdown** content."
        frontmatter={{ tags: ['coding'], version: '1.0' }}
        onContentChange={() => undefined}
        onFrontmatterChange={() => undefined}
      />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('T158 FunctionPage has no critical/serious violations', async () => {
    const { container } = render(
      <MemoryRouter>
        <FunctionPage />
      </MemoryRouter>,
    );
    await new Promise((r) => setTimeout(r, 100));
    await expectNoCriticalA11yViolations(container);
  });

  it('T139 CategoryPage has no critical/serious violations', async () => {
    const { default: CategoryPage } = await import('../../pages/CategoryPage');
    const { container } = render(
      <MemoryRouter>
        <CategoryPage />
      </MemoryRouter>,
    );
    await new Promise((r) => setTimeout(r, 100));
    await expectNoCriticalA11yViolations(container);
  });

  it('T139 TagPage has no critical/serious violations', async () => {
    const { default: TagPage } = await import('../../pages/TagPage');
    const { container } = render(
      <MemoryRouter>
        <TagPage />
      </MemoryRouter>,
    );
    await new Promise((r) => setTimeout(r, 100));
    await expectNoCriticalA11yViolations(container);
  });
});
