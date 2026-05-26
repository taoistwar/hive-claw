import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach, vi } from 'vitest';

// jsdom does not implement window.matchMedia, but Ant Design's responsive
// observer calls it on mount. Provide a permissive stub so antd components
// (Table, Form, Pagination, etc.) can render in tests.
Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: vi.fn().mockImplementation((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(), // deprecated
    removeListener: vi.fn(), // deprecated
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })),
});

// rc-table measures scrollbar size via getComputedStyle on a temporary
// element. jsdom returns CSSStyleDeclaration with empty strings, which is
// fine — but warns to stderr. Silence the specific "Not implemented"
// chatter without hiding real errors.
const originalErr = console.error;
console.error = (...args: unknown[]) => {
  const msg = typeof args[0] === 'string' ? args[0] : '';
  if (msg.includes('Not implemented: window.getComputedStyle')) return;
  originalErr(...args);
};

afterEach(() => {
  cleanup();
});
