/**
 * 弹层裁剪修复 + 事件页双 TAB 视觉自测（2026-11-18，UI 视觉自测纪律）。
 *
 * 用户实测两缺陷（同根因）：钩子/事件两页的会话 picker 都是 position:absolute
 * 挂卡片内行上，伸出卡片下缘被全局 .card{overflow:hidden} 裁掉（实测量化
 * pickerBottom 865 vs cardBottom 743 = 裁 122px）。修复 = Teleport to body +
 * position:fixed（仓内 ContextMenu 同款模式），坐标按锚按钮矩形收口。
 *
 * 事件页同批双 TAB 改版（用户拍板：首页只留信息展示，规则设置挪第二页）。
 *
 * 回归不变量：
 *   1. picker 是 body 直接子元素（Teleport 生效 = 逃出卡片裁剪域）；
 *   2. 打开的 picker 完整落在视口内（不被 .card 裁也不出屏）；
 *   3. 事件页默认落总览 TAB（信息展示），规则编辑在第二 TAB。
 * 截图人工看图（test-results/picker-clip-visual/shots/）。
 */
import { test, expect } from './fixtures'
import path from 'node:path'
import { newAuthedPage, gotoRoute } from './helpers'

const SHOT_DIR = path.join('test-results', 'picker-clip-visual', 'shots')

function shotPath(name: string): string {
  return path.join(SHOT_DIR, name)
}

/** picker 完整可见性断言：body 直挂 + 四边都在视口内。 */
async function assertPickerOnscreen(page: import('@playwright/test').Page, sel: string): Promise<void> {
  const verdict = await page.evaluate((sel) => {
    const el = document.querySelector(sel) as HTMLElement | null
    if (!el) return 'missing ' + sel
    if (el.parentElement !== document.body) {
      return `not teleported to body (parent=${el.parentElement?.className || el.parentElement?.tagName})`
    }
    const r = el.getBoundingClientRect()
    const problems: string[] = []
    if (r.top < 0) problems.push(`top ${Math.round(r.top)} < 0`)
    if (r.left < 0) problems.push(`left ${Math.round(r.left)} < 0`)
    if (r.bottom > window.innerHeight) problems.push(`bottom ${Math.round(r.bottom)} > winH ${window.innerHeight}（被裁/出屏）`)
    if (r.right > window.innerWidth) problems.push(`right ${Math.round(r.right)} > winW ${window.innerWidth}`)
    return problems.length ? problems.join('; ') : 'ok'
  }, sel)
  console.log(`[${sel}]`, verdict)
  expect(verdict).toBe('ok')
}

test('钩子页 picker：底部卡片上打开完整可见（不被 .card 裁剪）', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1920, height: 1080 })
  await gotoRoute(page, '/#/hooks')
  await page.locator('.tabs .tab', { hasText: '设置' }).click()
  // SessionEnd 卡片靠页底——旧版 picker 向下伸必被 .card 裁。
  const card = page.locator('.page-hooks .card', { hasText: 'SessionEnd' }).first()
  await card.locator('button', { hasText: '+ 添加钩子' }).click()
  await card.locator('.hooks-scope-btn').click()
  await page.locator('.hooks-scope-picker').waitFor({ state: 'visible' })
  await page.waitForTimeout(200)
  await assertPickerOnscreen(page, '.hooks-scope-picker')
  await page.screenshot({ path: shotPath('1-hooks-picker-fixed.png') })
})

test('事件页：默认总览 TAB（信息展示），规则设置第二 TAB，picker 完整可见', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1920, height: 1080 })
  await gotoRoute(page, '/#/events')
  await page.locator('[data-testid="events-tab-overview"]').waitFor({ state: 'visible' })
  // 默认落总览：信息展示在场、编辑区不在场。
  await page.locator('text=泵统计').first().waitFor({ state: 'visible' })
  expect(await page.locator('[data-testid="add-rule-btn"]').count()).toBe(0)
  await page.screenshot({ path: shotPath('2-events-overview.png'), fullPage: true })

  // 规则设置 TAB：编辑区在场。（tab 有 CSS transition——等转场走完再截，
  // 否则高亮停在旧 TAB 的转场伪影，2026-11-18 实拍踩过。）
  await page.locator('[data-testid="events-tab-rules"]').click()
  await page.locator('[data-testid="add-rule-btn"]').waitFor({ state: 'visible' })
  await page.waitForTimeout(300)
  await page.screenshot({ path: shotPath('3-events-rules-tab.png'), fullPage: true })

  // picker：卡片内行上打开 → Teleport 后完整可见。
  await page.locator('[data-testid="add-rule-btn"]').click()
  await page.locator('[data-testid="pick-session-0"]').click()
  await page.locator('.ev-picker').waitFor({ state: 'visible' })
  await page.waitForTimeout(200)
  await assertPickerOnscreen(page, '.ev-picker')
  await page.screenshot({ path: shotPath('4-events-picker-fixed.png') })
})
