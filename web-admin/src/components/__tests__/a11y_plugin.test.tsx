/// <reference types="vitest/globals" />
//
// 004 SC-008 axe regression — T157 dedicated test.
// Covers PluginFilters a11y (PluginUploader uses antd Select internally;
// its a11y is covered at page-level in a11y_004.test.tsx via
// PluginFilters isolated test).

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/react';
import { axe } from 'vitest-axe';
import 'vitest-axe/extend-expect';

import { PluginFilters } from '../PluginFilters';

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

describe('a11y — T157 Plugin a11y', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('PluginFilters has no critical/serious violations (empty)', async () => {
    const { container } = render(
      <PluginFilters value={{ offset: 0, limit: 20 }} onChange={() => undefined} />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('PluginFilters has no critical/serious violations (with search + tags + deleted_only)', async () => {
    const { container } = render(
      <PluginFilters
        value={{
          offset: 0,
          limit: 20,
          search: 'weather',
          tag_ids: [1, 2],
          deleted_only: true,
        }}
        onChange={() => undefined}
      />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('PluginFilters has no critical/serious violations (only search)', async () => {
    const { container } = render(
      <PluginFilters
        value={{ offset: 0, limit: 20, search: 'test' }}
        onChange={() => undefined}
      />,
    );
    await expectNoCriticalA11yViolations(container);
  });

  it('PluginFilters has no critical/serious violations (only deleted_only)', async () => {
    const { container } = render(
      <PluginFilters
        value={{ offset: 0, limit: 20, deleted_only: true }}
        onChange={() => undefined}
      />,
    );
    await expectNoCriticalA11yViolations(container);
  });
});
