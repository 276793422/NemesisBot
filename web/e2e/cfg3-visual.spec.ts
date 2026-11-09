/**
 * 配置页补全批次 3 四处视觉自测（2026-11-08，UI 视觉自测纪律）。
 *
 * 截图四处新增 UI，人工看图（test-results/cfg3-visual/shots/）：
 *   1. /security 配置 TAB 审批记忆规则卡（表单 + 规则表）
 *   2. /security 评估规则 TAB（表格 + 新增模态展开）
 *   3. /ziwei 门关横幅（立即启用按钮）→ 启用后总闸卡 + 高级配置展开
 *   4. /plugins WASM 分节总闸行 + 侧栏项目区 projects.max 输入
 */
import { test } from './fixtures'
import path from 'node:path'
import { newAuthedPage, gotoRoute } from './helpers'

const SHOT_DIR = path.join('test-results', 'cfg3-visual', 'shots')

function shotPath(name: string): string {
  return path.join(SHOT_DIR, name)
}

test('配置页补全批次3：security规则两卡/ziwei总闸/plugins总闸/侧栏上限输入截图', async ({ browser }) => {
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

  // —— 1. SecurityView 配置 TAB：审批记忆规则卡 ——
  await gotoRoute(page, '/#/security')
  await page.locator('[data-test="approval-rules-card"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('1-security-approval-rules.png'), fullPage: true })

  // —— 2. SecurityView 评估规则 TAB：表格 + 新增模态 ——
  const rulesTab = page.locator('button.tab', { hasText: '评估规则' })
  await rulesTab.waitFor({ state: 'visible' })
  await rulesTab.click()
  await page.locator('[data-test="eval-rules-tab"]').waitFor({ state: 'visible' })
  await page.locator('[data-test="eval-rules-table"], .empty-state').first().waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('2-security-eval-rules.png'), fullPage: true })
  // 打开新增模态再截一张
  await page.locator('[data-test="eval-rule-new"]').click()
  await page.locator('[data-test="eval-rule-modal"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('3-security-eval-modal.png'), fullPage: true })
  await page.locator('.er-modal-head button', { hasText: '取消' }).click()

  // —— 3. ZiweiView：门关横幅（若启用态则直接进总闸卡）——
  await gotoRoute(page, '/#/ziwei')
  const gateBanner = page.locator('[data-test="ziwei-enable"]')
  const gateCard = page.locator('[data-test="ziwei-config-card"]')
  if (await gateBanner.isVisible().catch(() => false)) {
    await page.screenshot({ path: shotPath('4-ziwei-gate-closed.png'), fullPage: true })
    await gateBanner.click()
    // 开启后总闸卡出现（refresh 拉完主视图）
    await gateCard.waitFor({ state: 'visible', timeout: 15000 })
  }
  await gateCard.waitFor({ state: 'visible' })
  // 展开高级配置
  await page.locator('.adv-summary').click()
  await page.locator('[data-test="ziwei-adv-save"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('5-ziwei-gate-card.png'), fullPage: true })

  // —— 4. PluginsView：WASM 总闸行（waitFor 带超时：loadWasmGate 是异步
  //      config.get，isVisible 立查会在数据回来前误判隐藏——上轮实证）——
  await gotoRoute(page, '/#/plugins')
  const wasmGateRow = page.locator('[data-test="wasm-gate-row"]')
  const gateShown = await wasmGateRow
    .waitFor({ state: 'visible', timeout: 10000 })
    .then(() => true)
    .catch(() => false)
  if (gateShown) {
    await page.screenshot({ path: shotPath('6-plugins-wasm-gate.png'), fullPage: true })
  } else {
    console.log('[skip] wasm 总闸行不可见（子系统未装配）——非失败，但需人工确认环境')
    await page.screenshot({ path: shotPath('6-plugins-wasm-gate-missing.png'), fullPage: true })
  }

  // —— 5. SessionSidebar：项目区 projects.max 输入（挂聊天页 + 侧栏默认
  //      收起：ChatView v-if="showSidebar"，须先点工具条「显示会话列表」；
  //      chat 是根路由 path='/'，'#/chat' 不匹配任何路由 → 主区空白）——
  await gotoRoute(page, '/#/')
  const sidebarToggle = page.locator('button[title="显示会话列表"]')
  await sidebarToggle.waitFor({ state: 'visible', timeout: 10000 })
  await sidebarToggle.click()
  const maxInput = page.locator('[data-test="project-max-input"]')
  const maxShown = await maxInput
    .waitFor({ state: 'visible', timeout: 10000 })
    .then(() => true)
    .catch(() => false)
  if (maxShown) {
    await maxInput.scrollIntoViewIfNeeded()
    await page.screenshot({ path: shotPath('7-sidebar-project-max.png'), fullPage: true })
  } else {
    console.log('[skip] 侧栏 projects.max 输入不可见（读取失败隐藏语义）——需人工确认')
    await page.screenshot({ path: shotPath('7-sidebar-project-max-missing.png'), fullPage: true })
  }

  if (consoleErrs.length > 0) {
    console.log('CONSOLE ERRORS:\n' + consoleErrs.join('\n'))
  }
})
