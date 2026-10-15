/**
 * Playwright e2e 配置。
 *
 * - 串行（workers=1）：全部 spec 共享同一个 gateway 实例与隔离 home，
 *   spec 内部各自自带前置（无跨文件顺序依赖），但共享状态（草稿目录、
 *   mock 证据文件）决定了并行会互相踩。
 * - 环境由 e2e/global-setup.ts 全权编排（onboard → 改配置 → mock → gateway），
 *   前置要求：nemesisbot 二进制已构建（E2E_BIN 或 target/{release,debug}）。
 * - 前端走 gateway 服务嵌入的 static/——改了 web/ 源码后需先 `npm run build`
 *   再构建 nemesisbot（或接受测的是上次构建进去的 UI）。CI 顺序已保证。
 */
import { defineConfig } from '@playwright/test'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))

export default defineConfig({
  testDir: path.join(here, 'e2e'),
  testMatch: '**/*.spec.ts',
  // 单条 spec 预算：含 UI 等待 + LLM 往返（mock 毫秒级）+ 历史轮询，90s 足够
  timeout: 90_000,
  expect: { timeout: 15_000 },
  fullyParallel: false,
  workers: 1,
  retries: 0, // 确定性 mock：失败就是失败，不重试掩盖
  reporter: [['list'], ['html', { open: 'never' }]],
  outputDir: path.join(here, 'test-results'),
  globalSetup: path.join(here, 'e2e', 'global-setup.ts'),
  globalTeardown: path.join(here, 'e2e', 'global-teardown.ts'),
  use: {
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    actionTimeout: 10_000,
    navigationTimeout: 30_000,
  },
})
