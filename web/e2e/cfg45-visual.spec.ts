/**
 * 配置页补全批次 4+5 九处视觉自测（2026-11-08，UI 视觉自测纪律）。
 *
 * 截图新增/改动 UI，人工看图（test-results/cfg45-visual/shots/）：
 *   1. /settings Agent TAB + 实验性高级区展开（批次 4 agent 高频键补全）
 *   2. /settings 工具 TAB exec 危险命令词表区（批次 4 死键接线）
 *   3. /settings Gateway TAB host/port 可编辑（批次 4）
 *   4. /settings 服务开关 TAB 高级 JSON 只读折叠展开（批次 5）
 *   5. /models 属性编辑器 api_base 行内编辑（批次 4 C-4）
 *   6. /coding delegate 三键卡（批次 4）
 *   7. /sandbox 沙箱执行卡（Windows 探针 → DACL 配置卡，批次 5）
 *   8. /connectors 执行策略设置区（批次 5）
 *   9. /settings 皮肤 TAB require_signed 开关（批次 5）
 */
import { test } from './fixtures'
import path from 'node:path'
import { newAuthedPage, gotoRoute } from './helpers'

const SHOT_DIR = path.join('test-results', 'cfg45-visual', 'shots')

function shotPath(name: string): string {
  return path.join(SHOT_DIR, name)
}

test('配置页补全批次4+5：settings四区/models属性/coding委托/sandbox ACL/connectors策略/皮肤开关截图', async ({ browser }) => {
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

  // —— 1. SettingsView Agent TAB + 实验性高级区展开 ——
  await gotoRoute(page, '/#/settings')
  const agentTab = page.locator('button.tab', { hasText: 'Agent' })
  await agentTab.waitFor({ state: 'visible' })
  await agentTab.click()
  await page.locator('[data-test="agent-max-tool-iterations"]').waitFor({ state: 'visible' })
  await page.locator('[data-test="exp-adv-toggle"]').click()
  await page.locator('[data-test="prompt-system"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('1-settings-agent-experimental.png'), fullPage: true })

  // —— 2. SettingsView 工具 TAB exec deny 区 ——
  await gotoRoute(page, '/#/settings')
  const toolsTab = page.locator('button.tab', { hasText: '工具' })
  await toolsTab.waitFor({ state: 'visible' })
  await toolsTab.click()
  await page.locator('[data-test="exec-deny-toggle"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('2-settings-tools-exec-deny.png'), fullPage: true })

  // —— 3. SettingsView Gateway TAB ——
  await gotoRoute(page, '/#/settings')
  const gwTab = page.locator('button.tab', { hasText: 'Gateway' })
  await gwTab.waitFor({ state: 'visible' })
  await gwTab.click()
  await page.locator('[data-test="gateway-host"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('3-settings-gateway.png'), fullPage: true })

  // —— 4. SettingsView 服务开关 TAB 高级 JSON 折叠展开 ——
  await gotoRoute(page, '/#/settings')
  const svcTab = page.locator('button.tab', { hasText: '服务开关' })
  await svcTab.waitFor({ state: 'visible' })
  await svcTab.click()
  await page.locator('[data-test="adv-json-toggle"]').waitFor({ state: 'visible' })
  await page.locator('[data-test="adv-json-toggle"]').click()
  await page.locator('[data-test="adv-json-blocks"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('4-settings-services-advjson.png'), fullPage: true })

  // —— 5. ModelsView 属性编辑器（api_base 行内编辑）——
  await gotoRoute(page, '/#/models')
  const attrBtn = page.locator('button', { hasText: '属性' }).first()
  await attrBtn.waitFor({ state: 'visible' })
  await attrBtn.click()
  await page.locator('[data-test="attr-api-base"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('5-models-attr-apibase.png'), fullPage: true })

  // —— 6. CodingView delegate 三键卡 ——
  await gotoRoute(page, '/#/coding')
  // toggle-switch 形态的 checkbox 视觉隐藏（slider 化）——断 attached 非 visible
  await page.locator('[data-test="delegate-enabled"]').waitFor({ state: 'attached' })
  await page.screenshot({ path: shotPath('6-coding-delegate.png'), fullPage: true })

  // —— 7. SandboxView（Windows → executor 沙箱卡 + DACL 配置卡）——
  await gotoRoute(page, '/#/sandbox')
  const aclCard = page.locator('[data-test="acl-card"]')
  try {
    await aclCard.waitFor({ state: 'visible', timeout: 8_000 })
    console.log('[sandbox] DACL 配置卡已渲染（Windows 探针在场）')
  } catch {
    // e2e 隔离 home 的 gateway 若 dacl 探针缺席（如 Linux runner），卡按平台
    // 语义不渲染——截图仍取（沙箱执行卡本身有价值），不判失败。
    console.log('[sandbox] DACL 配置卡未渲染（探针缺席，平台语义诚实降级）')
  }
  await page.screenshot({ path: shotPath('7-sandbox-acl.png'), fullPage: true })

  // —— 8. ConnectorsView 执行策略设置区 ——
  await gotoRoute(page, '/#/connectors')
  const connSettings = page.locator('[data-test="conn-settings"]')
  try {
    await connSettings.waitFor({ state: 'visible', timeout: 8_000 })
    console.log('[connectors] 执行策略区已渲染')
  } catch {
    console.log('[connectors] 执行策略区未渲染（config.get 失败，诚实降级注记）')
  }
  await page.screenshot({ path: shotPath('8-connectors-settings.png'), fullPage: true })

  // —— 9. SettingsView 皮肤 TAB（SkinsPanel require_signed 开关）——
  await gotoRoute(page, '/#/settings')
  const skinTab = page.locator('button.tab', { hasText: '皮肤' })
  await skinTab.waitFor({ state: 'visible' })
  await skinTab.click()
  // script-switch 形态同理，断 attached
  await page.locator('[data-test="require-signed-switch"]').waitFor({ state: 'attached' })
  await page.screenshot({ path: shotPath('9-settings-skins-require-signed.png'), fullPage: true })

  if (consoleErrs.length > 0) {
    console.log('CONSOLE ERRORS:\n' + consoleErrs.join('\n'))
  }
})
