/**
 * 按浏览器选择的 Playwright 包装配置。
 *
 * 用法（主配置无 projects 段，默认只跑 chromium；本包装按 BROWSER 环境变量
 * 生成单项目配置，供三浏览器回归轮独立 invocation 使用）：
 *
 *   BROWSER=firefox npx playwright test --config=playwright.per-browser.config.ts
 *
 * 全局编排（globalSetup/globalTeardown）与主配置同源，每 invocation 仍是
 * 全新环境（rmSync home → onboard → mock → gateway）。
 */
import { defineConfig, type Config } from '@playwright/test'
import base from './playwright.config'

const browser = (process.env.BROWSER || 'chromium') as 'chromium' | 'firefox' | 'webkit'
if (!['chromium', 'firefox', 'webkit'].includes(browser)) {
  throw new Error(`BROWSER 必须是 chromium|firefox|webkit，收到: ${browser}`)
}

export default defineConfig({
  ...(base as Config),
  projects: [
    {
      name: browser,
      use: { browserName: browser },
    },
  ],
})
