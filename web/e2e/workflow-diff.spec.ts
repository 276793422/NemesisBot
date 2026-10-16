/**
 * e2e/workflow-diff.spec.ts — 草稿 diff 三色渲染（verify-diff-visual D1-D3 迁移，
 * 自足化：不改名复用 workflow-gen 的注册物，自己注册独立工作流 mock-diff-base）。
 *
 *   基座：2 节点（fetch_news → summarize）经 WSAPI draft_apply 注册
 *   草稿 A：改 summarize prompt（修改·黄）+ 追加 extra_delay（新增·绿）
 *     → 新增 1 / 修改 1 / 移除 0，画布 3 节点
 *   D2：退出预览 → 回对话生成 TAB
 *   草稿 B：只留 fetch_news → summarize 记移除（红幽灵虚线 + 将被移除）
 *     → 移除 1，画布 2 节点（1 实 + 1 ghost）
 */
import fs from 'node:fs'
import path from 'node:path'
import { expect, test, type Page } from '@playwright/test'
import { BASE_URL, DRAFTS_DIR } from './env'
import { newAuthedPage, wsapi } from './helpers'

const WF_NAME = 'mock-diff-base'

function writeDraft(yaml: string): void {
  fs.mkdirSync(DRAFTS_DIR, { recursive: true })
  fs.writeFileSync(path.join(DRAFTS_DIR, `${WF_NAME}.yaml`), yaml)
}

const BASE_YAML = [
  'name: mock-diff-base',
  'description: diff 基座',
  'version: 1.0.0',
  'triggers: []',
  'nodes:',
  '  - id: fetch_news',
  '    node_type: http',
  '    config:',
  '      url: https://example.invalid/news',
  '      method: GET',
  '  - id: summarize',
  '    node_type: llm',
  '    config:',
  '      prompt: 原始提示词：{{fetch_news}}',
  'edges:',
  '  - from_node: fetch_news',
  '    to_node: summarize',
  '',
].join('\n')

const VARIANT_A = [
  'name: mock-diff-base',
  'description: diff 视觉验证 A（修改+新增）',
  'version: 1.0.0',
  'triggers: []',
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

const VARIANT_B = [
  'name: mock-diff-base',
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

async function openGenTab(page: Page): Promise<void> {
  await page.goto(`${BASE_URL}/#/workflows`)
  const genTab = page.locator('button.tab', { hasText: '对话生成' })
  await genTab.waitFor({ state: 'visible', timeout: 20_000 })
  await genTab.click()
}

async function previewDraft(page: Page, itemText: string): Promise<void> {
  const item = page.locator('.draft-item', { hasText: itemText }).first()
  await expect(item).toBeVisible({ timeout: 20_000 })
  await item.locator('button', { hasText: '画布预览' }).click()
  await expect(page.locator('h3', { hasText: `草稿预览：${WF_NAME}` })).toBeVisible({ timeout: 20_000 })
  await page.waitForTimeout(1500) // vue-flow 布局稳定
}

test('草稿 diff：修改黄+新增绿 → 退出预览 → 移除红幽灵', async ({ browser }) => {
  test.setTimeout(180_000)
  const page = await newAuthedPage(browser)
  page.on('dialog', (d) => void d.accept())

  // 基座注册（draft_apply 消费草稿文件）
  writeDraft(BASE_YAML)
  const applied = await wsapi(page, 'workflow', 'draft_apply', { name: WF_NAME })
  expect(applied.ok, `draft_apply 失败: ${applied.error}`).toBe(true)

  // ---- 草稿 A：修改 + 新增 ----
  writeDraft(VARIANT_A)
  await openGenTab(page)
  await previewDraft(page, '3 节点')

  const statA = await page.evaluate(() => ({
    added: document.querySelector('.diff-stat.added')?.textContent?.trim() ?? null,
    changed: document.querySelector('.diff-stat.changed')?.textContent?.trim() ?? null,
    removed: document.querySelector('.diff-stat.removed')?.textContent?.trim() ?? null,
    addedNodes: document.querySelectorAll('.wf-node.diff-added').length,
    changedNodes: document.querySelectorAll('.wf-node.diff-changed').length,
    ghostNodes: document.querySelectorAll('.wf-node.diff-removed.ghost').length,
    total: document.querySelectorAll('.vue-flow__node').length,
  }))
  // D1: 修改黄 + 新增绿 正确渲染
  expect(statA).toEqual({
    added: '新增 1',
    changed: '修改 1',
    removed: '移除 0',
    addedNodes: 1,
    changedNodes: 1,
    ghostNodes: 0,
    total: 3,
  })

  // D2: 退出预览 → 回对话生成 TAB
  await page.locator('button', { hasText: '退出预览' }).click()
  await expect(page.locator('#wf-gen-target')).toBeVisible({ timeout: 20_000 })
  await expect(page.locator('.draft-panel')).toBeVisible()

  // ---- 草稿 B：移除 ghost ----
  writeDraft(VARIANT_B)
  await page.locator('.panel-head button').first().click() // ⟳ 刷新草稿列表
  await previewDraft(page, '1 节点')

  const statB = await page.evaluate(() => ({
    removed: document.querySelector('.diff-stat.removed')?.textContent?.trim() ?? null,
    ghostNodes: document.querySelectorAll('.wf-node.diff-removed.ghost').length,
    ghostLabel: document.querySelector('.wf-node.diff-removed.ghost .wf-node-label')?.textContent?.trim() ?? null,
    total: document.querySelectorAll('.vue-flow__node').length,
  }))
  // D3: 移除红幽灵（将被移除）正确渲染
  expect(statB.removed).toBe('移除 1')
  expect(statB.ghostNodes).toBe(1)
  expect(statB.ghostLabel ?? '').toContain('将被移除')
  expect(statB.total).toBe(2)

  await page.context().close()
})
