#!/usr/bin/env node
/**
 * verify-ui-workflow-gen.mjs — 对话生成（AI workflow generator）UI 验证。
 *
 * 前置（由外部编排完成）：
 *   - mock LLM 已在 http://127.0.0.1:18901/v1（scripts/verify-mock-llm.mjs）
 *   - gateway 已在 http://127.0.0.1:49311（隔离 workspace，config 空鉴权）
 *   - <ws>/workflow/drafts/manual-draft-demo.yaml 已手工放置（V9）
 *
 * 覆盖（对应计划 §8.1 V1-V9）：
 *   V1 工作流页出现「🤖 对话生成」TAB（含草稿计数 dot）
 *   V2 agentGen 布局：工具栏 + 聊天面板 + 草稿面板（空态/手工草稿可见）
 *   V3 目标会话自动创建（localStorage sid 映射 + sessions.list 可见）
 *   V4 workflow_edit 注入链（mock LLM evidence 文件：首轮请求含注入区）
 *   V5 对话 → workflow_create 工具调用 → 草稿面板实时出现
 *   V6 画布预览第三数据态（palette 隐藏、diff 统计、退出按钮）
 *   V7 应用草稿 → 工作流列表出现 + 草稿消失
 *   V8 编辑已有工作流：切换目标后注入区含当前 YAML（evidence 第二轮）
 *   V9 放弃手工草稿 → 面板消失
 *
 * 截图输出：test-tools/resource/wfgen-verify/<NN>-*.png
 */

import { createRequire } from 'node:module'
import fs from 'node:fs'
import path from 'node:path'

const require = createRequire('C:/AI/NemesisBot_Rust/web/package.json')
const { chromium } = require('playwright')

// Dashboard = web channel（nemesis-web）端口（gateway.port 是 health server）。
// 鉴权 token = channels.web.auth_token（本隔离 workspace 模板默认值）。
const BASE = process.env.VERIFY_BASE || 'http://127.0.0.1:49000'
const AUTH_TOKEN = process.env.VERIFY_TOKEN || '276793422'
const OUT = 'C:/AI/NemesisBot_Rust/test-tools/resource/wfgen-verify'
const EVIDENCE_LOG = 'C:/AI/wfgen-verify/bot1/.nemesisbot/mock-evidence.log'
const EVIDENCE_REQ = 'C:/AI/wfgen-verify/bot1/.nemesisbot/mock-evidence.last-request.json'
const DRAFTS_DIR = 'C:/AI/wfgen-verify/bot1/.nemesisbot/workspace/workflow/drafts'

fs.mkdirSync(OUT, { recursive: true })

// V9 前置：手工放置一份草稿（幂等——上次运行可能已放弃/应用过）
fs.mkdirSync(DRAFTS_DIR, { recursive: true })
fs.writeFileSync(
  path.join(DRAFTS_DIR, 'manual-draft-demo.yaml'),
  [
    'name: manual-draft-demo',
    'description: 手工放置的草稿（V9 放弃验证）',
    'version: 1.0.0',
    'triggers: []',
    'nodes:',
    '  - id: wait_a_bit',
    '    node_type: delay',
    '    config:',
    '      seconds: 1',
    'edges: []',
    '',
  ].join('\n'),
)

const results = []
function record(id, name, ok, note = '') {
  results.push({ id, name, ok, note })
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${id}  ${name}${note ? '  — ' + note : ''}`)
}

async function main() {
  const browser = await chromium.launch()
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 950 } })
  // 预置 localStorage token → autoLogin 直接过（隔离 workspace 模板 token）
  await ctx.addInitScript((token) => {
    localStorage.setItem('nemesisbot_auth_token', token)
    localStorage.setItem('nemesisbot_wf_edit_sessions', '')
    localStorage.setItem('nemesisbot_wf_agentGen_target', '')
    localStorage.setItem('nemesisbot_wf_agentGen_sids', '')
  }, AUTH_TOKEN)
  const page = await ctx.newPage()
  page.setDefaultTimeout(30000)
  // 全局接受所有 window.confirm（应用同名替换确认 / 放弃确认）
  page.on('dialog', (d) => void d.accept())

  const shot = (n) => page.screenshot({ path: path.join(OUT, n), fullPage: false })

  // ---- 进入工作流页 ----
  await page.goto(`${BASE}/#/workflows`)
  await page.waitForLoadState('networkidle')

  // V1: 对话生成 TAB 存在
  const genTab = page.locator('button.tab', { hasText: '对话生成' })
  await genTab.waitFor({ state: 'visible', timeout: 20000 })
  record('V1', '工作流页出现「对话生成」TAB', true)
  await shot('01-workflow-tabs.png')

  // V2: 点进 agentGen，验证布局（工具栏 + 聊天 + 草稿面板 + 手工草稿 V9 前置可见）
  await genTab.click()
  await page.waitForTimeout(800)
  const targetSelect = page.locator('#wf-gen-target')
  const draftPanel = page.locator('.draft-panel')
  const chatPanel = page.locator('.gen-chat .chat-panel, .gen-chat [class*=chat]')
  const v2ok =
    (await targetSelect.isVisible()) &&
    (await draftPanel.isVisible()) &&
    (await chatPanel.first().isVisible().catch(() => false))
  const manualDraftVisible = await page
    .locator('.draft-item', { hasText: 'manual-draft-demo' })
    .first()
    .isVisible()
    .catch(() => false)
  record('V2', 'agentGen 布局（工具栏/聊天/草稿面板）', v2ok, `manual-draft 可见=${manualDraftVisible}`)
  await shot('02-agentgen-layout.png')

  // V3: 目标会话自动创建 —— 发送一条消息后检查 localStorage sid 映射
  const sendBox = page.locator('.gen-chat textarea').first()
  await sendBox.click()
  await sendBox.fill('帮我做一个每日新闻摘要工作流')
  await page.locator('.gen-chat button', { hasText: '发送' }).first().click()
  // 等 AI 回合完成（mock 两轮很快），草稿出现即代表全链路通了
  await page
    .locator('.draft-item', { hasText: 'mock-news-summary' })
    .first()
    .waitFor({ state: 'visible', timeout: 60000 })
    .catch(() => {})

  const sidMapRaw = await page.evaluate(() => localStorage.getItem('nemesisbot_wf_agentGen_sids'))
  const registered = await page.evaluate(() => localStorage.getItem('nemesisbot_wf_edit_sessions'))
  const sidMapped = !!sidMapRaw && sidMapRaw.includes('__new__')
  const sidRegistered = !!registered && registered.length > 2
  record('V3', '目标会话自动创建并登记', sidMapped && sidRegistered, `sidMap=${sidMapRaw}`)
  await shot('03-chat-draft-appeared.png')

  // V4: workflow_edit 注入链 —— evidence 文件首轮请求含注入区
  let v4ok = false
  let v4note = 'no evidence'
  try {
    const log = fs.readFileSync(EVIDENCE_LOG, 'utf-8').trim().split('\n')
    const first = log.find((l) => l.includes('last_role=null') || l.includes('last_role=user')) || log[0]
    v4ok = first.includes('wf_edit_section=true')
    v4note = first.trim()
  } catch (e) {
    v4note = String(e)
  }
  record('V4', 'workflow_edit system 注入（mock LLM 收到）', v4ok, v4note)

  // V5: 草稿面板出现 mock-news-summary（tool_event → 自动刷新）
  const mockDraft = page.locator('.draft-item', { hasText: 'mock-news-summary' }).first()
  const v5ok = await mockDraft.isVisible()
  record('V5', 'workflow_create 草稿实时出现在面板', v5ok)
  await shot('04-draft-panel-entry.png')

  // V6: 画布预览第三数据态
  await mockDraft.locator('button', { hasText: '画布预览' }).click()
  await page.waitForTimeout(1500)
  const previewBanner = await page
    .locator('h3', { hasText: '草稿预览：mock-news-summary' })
    .isVisible()
    .catch(() => false)
  const paletteHidden = !(await page.locator('.palette').isVisible().catch(() => false))
  const exitBtn = await page.locator('button', { hasText: '退出预览' }).isVisible().catch(() => false)
  const diffStat = await page.locator('.diff-stat.added', { hasText: '新增' }).isVisible().catch(() => false)
  record('V6', '画布草稿预览（只读 + diff + palette 隐藏）', previewBanner && paletteHidden && exitBtn && diffStat,
    `banner=${previewBanner} paletteHidden=${paletteHidden} exit=${exitBtn} diff=${diffStat}`)
  await shot('05-canvas-draft-preview.png')

  // V7: 应用草稿 → 工作流列表出现 + 草稿消失
  await page.locator('button', { hasText: '应用草稿' }).first().click()
  await page.waitForTimeout(1500)
  await page.goto(`${BASE}/#/workflows`)
  await page.waitForTimeout(500)
  const listTab = page.locator('button.tab', { hasText: '工作流列表' })
  await listTab.click()
  await page.waitForTimeout(1200)
  const listed = await page
    .getByText('mock-news-summary')
    .first()
    .isVisible()
    .catch(() => false)
  // 回 agentGen 确认草稿已消费
  await page.locator('button.tab', { hasText: '对话生成' }).click()
  await page.waitForTimeout(800)
  const draftGone = !(await mockDraft.isVisible().catch(() => false))
  record('V7', '应用草稿 → 注册成功 + 草稿消费', listed && draftGone, `listed=${listed} draftGone=${draftGone}`)
  await shot('06-after-apply.png')

  // V8: 编辑已有工作流 —— 切目标，发消息，检查注入区带当前 YAML
  await page.locator('#wf-gen-target').selectOption('mock-news-summary')
  await page.waitForTimeout(1000)
  const box2 = page.locator('.gen-chat textarea').first()
  await box2.click()
  await box2.fill('把触发时间改到晚上十点')
  await page.locator('.gen-chat button', { hasText: '发送' }).first().click()
  await page.waitForTimeout(5000)
  let v8ok = false
  let v8note = 'no evidence file'
  try {
    const req = JSON.parse(fs.readFileSync(EVIDENCE_REQ, 'utf-8'))
    const text = JSON.stringify(req.messages ?? [])
    v8ok = text.includes('Workflow Editor Session') && text.includes('mock-news-summary') && text.includes('trigger_type')
    v8note = `msg_count=${(req.messages ?? []).length}`
  } catch (e) {
    v8note = String(e)
  }
  record('V8', '编辑已有工作流（注入当前 YAML）', v8ok, v8note)
  await shot('07-edit-existing-target.png')

  // V9: 放弃手工草稿（confirm 由全局 dialog handler 接受）
  const manualDraft = page.locator('.draft-item', { hasText: 'manual-draft-demo' }).first()
  await manualDraft.locator('button', { hasText: '放弃' }).click()
  await page.waitForTimeout(1200)
  const manualGone = !(await manualDraft.isVisible().catch(() => false))
  record('V9', '放弃草稿 → 面板移除', manualGone)
  await shot('08-after-discard.png')

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
