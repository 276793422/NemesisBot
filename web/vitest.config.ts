import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  plugins: [vue()],
  test: {
    environment: 'jsdom',
    globals: true,
    // src/tests/setup.ts：Node ≥26 惰性 localStorage 全局遮蔽 jsdom 实现
    // 的兜底 shim（见该文件头注释）。
    setupFiles: ['src/tests/setup.ts'],
    include: ['src/**/*.spec.ts'],
  },
})
