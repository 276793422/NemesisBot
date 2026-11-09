/**
 * 配置页补全批次 2 四页视觉自测（2026-11-08，UI 视觉自测纪律）。
 *
 * 截图四处新增/改动 UI，人工看图（test-results/cfg-pages-visual/shots/）：
 *   1. /settings 数据治理 TAB（logs 16 族 TTL 预设档 + 高级逐族展开 +
 *      checkpoint.gc + 磁盘水位 + 失败诊断捕获）
 *   2. /forge 配置 TAB 高级设置折叠展开（forge side-file 8 键）
 *   3. /usage 设置 TAB（usage.retention_days / max_rows）
 *   4. /workflow 设置 TAB（workflow.wall_clock_budget_secs）
 */
import { test } from './fixtures'
import path from 'node:path'
import { newAuthedPage, gotoRoute } from './helpers'

const SHOT_DIR = path.join('test-results', 'cfg-pages-visual', 'shots')

function shotPath(name: string): string {
  return path.join(SHOT_DIR, name)
}

test('配置页补全批次2：数据治理/forge高级/usage设置/workflow设置四页截图', async ({ browser }) => {
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

  // —— 1. SettingsView 数据治理 TAB ——
  await gotoRoute(page, '/#/settings')
  const dataTab = page.locator('button.tab', { hasText: '数据治理' })
  await dataTab.waitFor({ state: 'visible' })
  await dataTab.click()
  await page.locator('h3', { hasText: '日志保留期' }).waitFor()
  // 展开高级逐族设置（16 族 grid）
  await page.locator('label.form-label', { hasText: '高级：逐族设置' }).click()
  await page.locator('input[type="number"]').first().waitFor()
  await page.screenshot({ path: shotPath('1-settings-data.png'), fullPage: true })

  // —— 2. ForgeView 配置 TAB 高级设置折叠展开 ——
  await gotoRoute(page, '/#/forge')
  const forgeCfgTab = page.locator('button.tab', { hasText: '配置' })
  await forgeCfgTab.waitFor({ state: 'visible' })
  await forgeCfgTab.click()
  const advHeader = page.locator('.card-header', { hasText: '高级设置' })
  await advHeader.waitFor({ state: 'visible' })
  await advHeader.click()
  await page.locator('h3', { hasText: '高级设置' }).waitFor()
  // 等高级内容渲染（toggle/输入框出现）
  await page.locator('input.form-input, input[type="checkbox"]').last().waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('2-forge-advanced.png'), fullPage: true })

  // —— 3. UsageView 设置 TAB ——
  await gotoRoute(page, '/#/usage')
  const usageSetTab = page.locator('button.tab-btn', { hasText: '设置' })
  await usageSetTab.waitFor({ state: 'visible' })
  await usageSetTab.click()
  await page.locator('h3', { hasText: '数据保留策略' }).waitFor()
  await page.screenshot({ path: shotPath('3-usage-settings.png'), fullPage: true })

  // —— 4. WorkflowView 设置 TAB ——
  await gotoRoute(page, '/#/workflows')
  const wfSetTab = page.locator('button.tab', { hasText: '设置' })
  await wfSetTab.waitFor({ state: 'visible' })
  await wfSetTab.click()
  await page.locator('h3', { hasText: '执行参数' }).waitFor()
  await page.screenshot({ path: shotPath('4-workflow-settings.png'), fullPage: true })
  if (consoleErrs.length > 0) {
    console.log('CONSOLE ERRORS:\n' + consoleErrs.join('\n'))
  }
})
