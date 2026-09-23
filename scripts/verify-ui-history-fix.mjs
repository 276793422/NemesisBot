// verify-ui-history-fix.mjs — 项目会话历史修复 UI 验证（BUG 2026-09-23，用户硬性要求）。
//
// 前置（由外部编排完成）：
//   - gateway 以 --local 跑在 bin/bugreproA/.nemesisbot（项目「专利」路径指向
//     不存在的 D:\AI\NemesisBot_Rust\docs\INFO —— 场景 A 病灶形态）
//   - Dashboard 在 http://127.0.0.1:49000，token=276793422（channels.web.auth_token）
//
// 覆盖（对应计划 §6.3）：
//   U1 侧栏项目区出现「专利」组（目录缺失 → 组头带 ⚠ unavailable 态）
//   U2 点击项目会话 → 历史消息行渲染（≥8 行，12 行存量）
//   U3 无「⚠ 项目…不可用」伪 assistant 消息（修复前唯一「消息」就是它）
//   U4 无「⚠️ 历史消息加载失败」横幅（10s 围栏未触发）
//   U5 切走（对话组）再切回 → 历史仍在（围栏重载路径）
//
// 截图输出：test-tools/resource/history-fix-verify/*.png
import { createRequire } from 'node:module'
import fs from 'node:fs'
import path from 'node:path'

const require = createRequire('C:/AI/NemesisBot_Rust/web/package.json')
const { chromium } = require('playwright')

const BASE = process.env.VERIFY_BASE || 'http://127.0.0.1:49000'
const AUTH_TOKEN = process.env.VERIFY_TOKEN || '276793422'
const OUT = 'C:/AI/NemesisBot_Rust/test-tools/resource/history-fix-verify'

fs.mkdirSync(OUT, { recursive: true })

const results = []
function record(id, name, ok, note = '') {
  results.push({ id, name, ok, note })
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${id}  ${name}${note ? '  — ' + note : ''}`)
}

async function main() {
  const browser = await chromium.launch()
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 950 } })
  await ctx.addInitScript((token) => {
    localStorage.setItem('nemesisbot_auth_token', token)
  }, AUTH_TOKEN)
  const page = await ctx.newPage()
  page.setDefaultTimeout(30000)
  const shot = (n) => page.screenshot({ path: path.join(OUT, n), fullPage: false })

  await page.goto(`${BASE}/#/`)
  await page.waitForLoadState('networkidle')
  await page.waitForTimeout(1500) // 侧栏 sessions.list 渲染

  // 会话侧栏默认收起（sessionStore.showSidebar=false）→ 点工具栏开关展开
  const sidebarToggle = page.locator('.toolbar-toggle').first()
  if (await sidebarToggle.isVisible().catch(() => false)) {
    await sidebarToggle.click()
    await page.waitForTimeout(800)
  }

  // U1: 项目组「专利」出现（目录缺失 → unavailable 组头，但组必须可见）
  const projGroup = page.locator('.group-header', { hasText: '专利' }).first()
  const projVisible = await projGroup.isVisible().catch(() => false)
  const unavailable = projVisible && await projGroup.evaluate((el) => el.classList.contains('unavailable')).catch(() => false)
  record('U1', '侧栏「专利」项目组可见（unavailable 态）', projVisible, `unavailable=${unavailable}`)
  await shot('01-sidebar-project-group.png')

  // U2: 点击项目组内的会话行 → 历史渲染
  const projSession = page
    .locator('.session-item', { hasText: /.*/ })
    .filter({
      has: page.locator('xpath=self::*'),
    })
  // 直接定位项目组之后的第一个 session-item（组头相邻兄弟容器内）
  const sessionRow = page
    .locator('.group-header', { hasText: '专利' })
    .first()
    .locator('xpath=following-sibling::div[contains(@class,"session-item")][1]')
  await sessionRow.click()
  await page.waitForTimeout(2500) // history_request → response → 渲染

  const msgCount = await page.locator('.message').count()
  record('U2', '项目会话历史消息行渲染（>=8）', msgCount >= 8, `rows=${msgCount}`)
  await shot('02-project-session-history.png')

  // U3: 无「当前不可用」伪消息
  const fakeMsg = await page
    .locator('.message', { hasText: '当前不可用' })
    .first()
    .isVisible()
    .catch(() => false)
  record('U3', '无「⚠ 项目…不可用」伪消息', !fakeMsg)

  // U4: 无历史加载失败横幅
  const failBanner = await page
    .locator('.message', { hasText: '历史消息加载失败' })
    .first()
    .isVisible()
    .catch(() => false)
  record('U4', '无「历史消息加载失败」横幅', !failBanner)

  // U5: 切走再切回 → 历史仍在
  const chatNewBtn = page.locator('.fn-new-chat')
  if (await chatNewBtn.isVisible().catch(() => false)) {
    await chatNewBtn.click()
    await page.waitForTimeout(1200)
  }
  await sessionRow.click()
  await page.waitForTimeout(2500)
  const msgCountBack = await page.locator('.message').count()
  const failAgain = await page
    .locator('.message', { hasText: '历史消息加载失败' })
    .first()
    .isVisible()
    .catch(() => false)
  record('U5', '切走再切回历史仍在（围栏重载）', msgCountBack >= 8 && !failAgain, `rows=${msgCountBack}`)
  await shot('03-switch-back-history.png')

  await browser.close()

  const failed = results.filter((r) => !r.ok)
  console.log(`\n==== ${results.length - failed.length}/${results.length} PASS ====`)
  if (failed.length > 0) {
    console.log('FAILED:', failed.map((f) => f.id).join(', '))
    process.exit(1)
  }
}

main().catch((e) => {
  console.error('FATAL:', e)
  process.exit(2)
})
