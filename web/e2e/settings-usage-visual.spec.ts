/**
 * e2e/settings-usage-visual.spec.ts — 占用摘要条视觉自测（2026-11-22 批二 C）
 *
 * UI 视觉自测纪律：截图新增/改动 UI，人工看图
 * （test-results/settings-usage-visual/shots/）：
 *   1. /settings 数据治理 TAB 占用摘要条折叠态（懒触发——未展开零请求，
 *      usage-total 不渲染）
 *   2. 展开逐族（首拉 logs.usage + 16 族行 + 总量徽标）
 *   3. 「刷新」按钮二次拉取后仍正常渲染
 *   4. /settings 日志 TAB detail_level 重启生效 + summary 自愈回写 hint
 *      （B 联动文案）与 save_raw 重启生效 hint
 */
import path from 'node:path'
import { expect, test } from './fixtures'
import { gotoRoute, newAuthedPage, sendChat, waitAssistantReply } from './helpers'

const SHOT_DIR = path.join('test-results', 'settings-usage-visual', 'shots')

function shotPath(name: string): string {
  return path.join(SHOT_DIR, name)
}

test('占用摘要条三态 + 日志 TAB hints 截图', async ({ browser }) => {
  test.setTimeout(180_000)
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1440, height: 900 })
  const consoleErrs: string[] = []
  page.on('console', (m) => {
    if (m.type() === 'error') {
      consoleErrs.push(m.text().slice(0, 300))
      console.log('[console-err] ' + m.text().slice(0, 300))
    }
  })
  page.on('pageerror', (e) => {
    consoleErrs.push('PAGE-ERR: ' + String(e).slice(0, 400))
    console.log('[page-err] ' + String(e).slice(0, 400))
  })

  // 种子一条会话消息 → session_logs 有真实数据可统计（截图数字非全零）
  await sendChat(page, `占用条种子消息 ${Date.now()}`)
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY', 45_000)

  // —— 1. 数据治理 TAB：折叠态（懒触发，未展开零请求）——
  await gotoRoute(page, '/#/settings')
  const dataTab = page.locator('button.tab', { hasText: '数据治理' })
  await dataTab.waitFor({ state: 'visible' })
  await dataTab.click()
  const strip = page.locator('[data-test="usage-strip"]')
  await strip.waitFor({ state: 'visible' })
  // 折叠态未加载：总量徽标不在场（vitest 已断零请求；此处断渲染面一致）
  await expect(page.locator('[data-test="usage-total"]')).toHaveCount(0)
  await page.screenshot({ path: shotPath('1-usage-collapsed.png'), fullPage: true })

  // —— 2. 展开逐族：首拉 + 总量徽标 + 16 族行 ——
  await page.locator('[data-test="usage-toggle"]').click()
  await page.locator('[data-test="usage-families"]').waitFor({ state: 'visible' })
  await expect(page.locator('[data-test="usage-total"]')).toBeVisible()
  await expect(page.locator('[data-test="usage-family-value"]')).toHaveCount(16)
  await page.screenshot({ path: shotPath('2-usage-expanded.png'), fullPage: true })

  // —— 3. 刷新按钮（二次拉取后仍正常渲染）——
  const refresh = page.locator('[data-test="usage-refresh"]')
  await refresh.click()
  await expect(refresh).toBeEnabled({ timeout: 30_000 })
  await expect(page.locator('[data-test="usage-family-value"]')).toHaveCount(16)
  await page.screenshot({ path: shotPath('3-usage-refreshed.png'), fullPage: true })

  // —— 4. 日志 TAB：detail_level 自愈回写 hint + save_raw 重启生效 hint ——
  const logTab = page.locator('button.tab', { hasText: '日志' })
  await logTab.click()
  await expect(page.getByText('历史遗留的 summary 档已按 truncated 生效')).toBeVisible({ timeout: 15_000 })
  await expect(page.getByText('已禁用（默认）；重启生效。')).toBeVisible()
  await page.screenshot({ path: shotPath('4-logging-hints.png'), fullPage: true })

  if (consoleErrs.length > 0) {
    console.log('CONSOLE ERRORS:\n' + consoleErrs.join('\n'))
  }
  expect(consoleErrs, '页面 console 零报错').toHaveLength(0)

  await page.context().close()
})
