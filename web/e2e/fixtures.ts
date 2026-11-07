/**
 * e2e 共享 test fixture：context 卫生（2026-11-07 WK 泄漏冻结根修）。
 *
 * 背景：各测试用 newAuthedPage()/newContext() 手工建的 context 从不关闭，
 * 跨 spec 泄漏存活。WebKit（Playwright WinPort）所有 page 共享一个进程：
 * 泄漏 page 的 JS/raster 与前景 page 争抢同一进程资源，前景 page 被饿死——
 * 碎裂绘制、screencast 停帧、点击 handler 不跑（wk6 全量 edge:99 冻结实录，
 * 10+ 泄漏 page 在场）；且泄漏 page 画面污染失败取证（error-context aria
 * 快照/失败截图采到泄漏 page——888888 模态与子代理台账模态之谜实锤）。
 *
 * 本 fixture 在每个测试结束后关闭该 browser fixture 下全部 context。
 * context 是纯浏览器侧资源，服务器态（后台注册表/会话/看板）不受影响。
 * auto fixture：spec 只需从这里 import test，无需显式调用。
 */
import { test as base, expect, type BrowserContext } from '@playwright/test'

type CleanupFixtures = { contextCleanup: void }

export const test = base.extend<CleanupFixtures>({
  contextCleanup: [
    async ({ browser }, use): Promise<void> => {
      await use()
      const closes: Promise<void>[] = browser.contexts().map((c: BrowserContext) => c.close())
      await Promise.all(closes)
    },
    { auto: true },
  ],
})

export { expect }
export type { APIRequestContext, Locator, Page } from '@playwright/test'
