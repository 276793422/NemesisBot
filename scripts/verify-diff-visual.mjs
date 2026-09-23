#!/usr/bin/env node
/**
 * verify-diff-visual.mjs — 草稿预览 diff 三色渲染 + 退出预览路径的视觉验证。
 *
 * 补主验证脚本（verify-ui-workflow-gen.mjs V6）的盲区：主脚本里草稿与已注册
 * 定义完全相同（diff 全 0），三色样式和 ghost 节点从未被真正渲染过；「退出预览」
 * 按钮路径也没被点过。本脚本构造两份与已注册 mock-news-summary 有差异的草稿：
 *   草稿 A：改 summarize 的 prompt（修改·黄）+ 追加 extra_delay（新增·绿）
 *   草稿 B：只留 fetch_news → summarize 记移除（移除·红幽灵虚线）
 *
 * 前置：mock LLM :18901、gateway :49000（隔离 workspace，mock-news-summary 已注册）。
 * 截图：test-tools/resource/wfgen-verify/10-11-12-*.png
 */

import { createRequire } from 'node:module'
import fs from 'node:fs'
import path from 'node:path'

const require = createRequire('C:/AI/NemesisBot_Rust/web/package.json')
const { chromium } = require('playwright')

const BASE = process.env.VERIFY_BASE || 'http://127.0.0.1:49000'
const AUTH_TOKEN = process.env.VERIFY_TOKEN || '276793422'
const OUT = 'C:/AI/NemesisBot_Rust/test-tools/resource/wfgen-verify'
const DRAFTS_DIR = 'C:/AI/wfgen-verify/bot1/.nemesisbot/workspace/workflow/drafts'
const DRAFT_FILE = path.join(DRAFTS_DIR, 'mock-news-summary.yaml')

fs.mkdirSync(OUT, { recursive: true })

// 草稿 A：基于已注册定义（fetch_news 不动）→ summarize 改 prompt + 新增 delay 节点
const DRAFT_A = [
  'name: mock-news-summary',
  'description: diff 视觉验证 A（修改+新增）',
  'version: 1.0.0',
  'triggers:',
  '  - trigger_type: cron',
  '    config:',
  '      schedule: 0 9 * * *',
  'nodes:',
  '  - id: fetch_news',
  '    node_type: http',
  '    config:',
  '      url: https://example.invalid/news',
  '      method: GET',
  '  - id: summarize',
  '    node_type: llm',
  '    config:',
  '      prompt: 改过的提示词：{{fetch_news}}',
  '  - id: extra_delay',
  '    node_type: delay',
  '    config:',
  '      seconds: 5',
  'edges:',
  '  - from_node: fetch_news',
  '    to_node: summarize',
  '  - from_node: summarize',
  '    to_node: extra_delay',
  '',
].join('\n')

// 草稿 B：只留 fetch_news → summarize 应记移除（ghost）
const DRAFT_B = [
  'name: mock-news-summary',
  'description: diff 视觉验证 B（移除）',
  'version: 1.0.0',
  'triggers: []',
  'nodes:',
  '  - id: fetch_news',
  '    node_type: http',
  '    config:',
  '      url: https://example.invalid/news',
  '      method: GET',
  'edges: []',
  '',
].join('\n')

const results = []
function record(id, name, ok, note = '') {
  results.push({ id, ok })
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${id}  ${name}${note ? '  — ' + note : ''}`)
}

async function main() {
  const browser = await chromium.launch()
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 950 } })
  await ctx.addInitScript((token) => {
    localStorage.setItem('nemesisbot_auth_token', token)
  }, AUTH_TOKEN)
  const page = await ctx.newPage()
  page.setDefaultTimeout(20000)
  page.on('dialog', (d) => void d.accept())

  // ---- 草稿 A：修改 + 新增 ----
  fs.writeFileSync(DRAFT_FILE, DRAFT_A)
  await page.goto(`${BASE}/#/workflows`)
  await page.waitForLoadState('networkidle')
  const genTab = page.locator('button.tab', { hasText: '对话生成' })
  await genTab.waitFor({ state: 'visible' })
  await genTab.click()
  // 面板条目只显示 name/节点数，不显示 description —— 用节点数区分 A（3 节点）
  const itemA = page.locator('.draft-item', { hasText: '3 节点' }).first()
  await itemA.waitFor({ state: 'visible' })
  await itemA.locator('button', { hasText: '画布预览' }).click()
  await page.locator('h3', { hasText: '草稿预览：mock-news-summary' }).waitFor({ state: 'visible' })
  await page.waitForTimeout(1500)

  const statA = await page.evaluate(() => ({
    added: document.querySelector('.diff-stat.added')?.textContent?.trim() ?? null,
    changed: document.querySelector('.diff-stat.changed')?.textContent?.trim() ?? null,
    removed: document.querySelector('.diff-stat.removed')?.textContent?.trim() ?? null,
    addedNodes: document.querySelectorAll('.wf-node.diff-added').length,
    changedNodes: document.querySelectorAll('.wf-node.diff-changed').length,
    ghostNodes: document.querySelectorAll('.wf-node.diff-removed.ghost').length,
    total: document.querySelectorAll('.vue-flow__node').length,
  }))
  const aOk =
    statA.added === '新增 1' &&
    statA.changed === '修改 1' &&
    statA.removed === '移除 0' &&
    statA.addedNodes === 1 &&
    statA.changedNodes === 1 &&
    statA.ghostNodes === 0 &&
    statA.total === 3
  record('D1', '草稿 A：修改黄 + 新增绿 正确渲染', aOk, JSON.stringify(statA))
  await page.screenshot({ path: path.join(OUT, '10-diff-added-changed.png') })

  // ---- 退出预览 → 回对话生成 ----
  await page.locator('button', { hasText: '退出预览' }).click()
  await page.waitForTimeout(800)
  const backOk =
    (await page.locator('#wf-gen-target').isVisible().catch(() => false)) &&
    (await page.locator('.draft-panel').isVisible().catch(() => false))
  record('D2', '退出预览 → 返回对话生成 TAB', backOk)
  await page.screenshot({ path: path.join(OUT, '11-exit-preview.png') })

  // ---- 草稿 B：移除 ghost ----
  fs.writeFileSync(DRAFT_FILE, DRAFT_B)
  await page.locator('.panel-head button').first().click() // ⟳ 刷新草稿列表
  await page.waitForTimeout(800)
  const itemB = page.locator('.draft-item', { hasText: '1 节点' }).first()
  await itemB.waitFor({ state: 'visible' })
  await itemB.locator('button', { hasText: '画布预览' }).click()
  await page.locator('h3', { hasText: '草稿预览：mock-news-summary' }).waitFor({ state: 'visible' })
  await page.waitForTimeout(1500)

  const statB = await page.evaluate(() => ({
    removed: document.querySelector('.diff-stat.removed')?.textContent?.trim() ?? null,
    ghostNodes: document.querySelectorAll('.wf-node.diff-removed.ghost').length,
    ghostLabel: document.querySelector('.wf-node.diff-removed.ghost .wf-node-label')?.textContent?.trim() ?? null,
    total: document.querySelectorAll('.vue-flow__node').length,
  }))
  const bOk =
    statB.removed === '移除 1' &&
    statB.ghostNodes === 1 &&
    (statB.ghostLabel ?? '').includes('将被移除') &&
    statB.total === 2
  record('D3', '草稿 B：移除红幽灵（将被移除）正确渲染', bOk, JSON.stringify(statB))
  await page.screenshot({ path: path.join(OUT, '12-diff-removed-ghost.png') })

  await browser.close()
  const failed = results.filter((r) => !r.ok)
  console.log(`\n==== ${results.length - failed.length}/${results.length} PASS ====`)
  if (failed.length > 0) process.exit(1)
}

main().catch((e) => {
  console.error('FATAL:', e)
  process.exit(2)
})
