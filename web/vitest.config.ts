import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/__tests__/**/*.{test,spec}.{ts,tsx}'],
    // a11y.test.tsx 需要 vitest-axe 与 axe-core 依赖（package.json devDeps
    // 已声明，但本地未 npm install 时会让整个 suite 因导入失败而炸）。
    // 装好依赖后把它从 exclude 移除即可。
    exclude: ['**/node_modules/**', '**/dist/**', '**/__tests__/a11y.test.tsx'],
  },
});
