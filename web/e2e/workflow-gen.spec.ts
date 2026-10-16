/**
 * e2e/workflow-gen.spec.ts — 对话生成全链路（verify-ui-workflow-gen V1-V9 迁移，
 * 自足化：manual 草稿 spec 内自己放置，e2e 隔离 home 无历史残留）。
 *
 *   V1 工作流页「对话生成」TAB
 *   V2 agentGen 布局：目标下拉 + 草稿面板 + 聊天面板（manual 草稿可见）
 *   V3 目标会话自动创建（localStorage sid 映射 + 编辑会话登记）
 *   V4 workflow_edit 注入链（mock 证据：请求含注入区，delta 读取只看本轮）
 *   V5 E2E_WF_GEN 标记 → workflow_create 工具调用 → 草稿实时出现
 *   V6 画布草稿预览（只读 + diff 统计 + palette 隐藏 + 退出按钮）
 *   V7 应用草稿 → 工作流列表出现 + 草稿被消费
 *   V8 编辑已有工作流：注入区含当前 YAML（mock 证据 yaml_like + last-request）
 *   V9 放弃手工草稿 → 面板移除
 */
import fs from 'node:fs'
import path from 'node:path'
import { expect, test } from '@playwright/test'
import { BASE_URL, DRAFTS_DIR, EVIDENCE_LAST_REQUEST } from './env'
import { evidenceCount, evidenceSince, newAuthedPage, wsapi } from './helpers'

const MANUAL_DRAFT = [
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
].join('\n')

test('对话生成：布局/注入/工具调用草稿/画布预览/应用/编辑注入/放弃', async ({ browser }) => {
  test.setTimeout(240_000)
  // V9 前置：手工草稿（幂等——fresh home 无残留，写一次即可）
  fs.mkdirSync(DRAFTS_DIR, { recursive: true })
  fs.writeFileSync(path.join(DRAFTS_DIR, 'manual-draft-demo.yaml'), MANUAL_DRAFT)

  const page = await newAuthedPage(browser)
  page.on('dialog', (d) => void d.accept()) // 应用同名替换确认 / 放弃确认

  // V1: 对话生成 TAB
  await page.goto(`${BASE_URL}/#/workflows`)
  const genTab = page.locator('button.tab', { hasText: '对话生成' })
  await genTab.waitFor({ state: 'visible', timeout: 20_000 })
  await genTab.click()

  // V2: agentGen 布局（目标下拉 + 草稿面板 + 聊天 + manual 草稿）
  await expect(page.locator('#wf-gen-target')).toBeVisible({ timeout: 20_000 })
  await expect(page.locator('.draft-panel')).toBeVisible()
  await expect(page.locator('.gen-chat textarea').first()).toBeVisible()
  await expect(page.locator('.draft-item', { hasText: 'manual-draft-demo' }).first()).toBeVisible()

  // V3-V5: 发送（带 E2E_WF_GEN 路由标记）→ 会话登记 + 注入证据 + 草稿出现
  const evidenceBefore = evidenceCount()
  const sendBox = page.locator('.gen-chat textarea').first()
  await sendBox.click()
  await sendBox.fill('E2E_WF_GEN 帮我做一个每日新闻摘要工作流')
  await page.locator('.gen-chat button', { hasText: '发送' }).first().click()

  const mockDraft = page.locator('.draft-item', { hasText: 'mock-news-summary' }).first()
  await mockDraft.waitFor({ state: 'visible', timeout: 90_000 })

  // V3: 目标会话服务端绑定（真相源 session_bindings 注册表；旧 localStorage
  // 映射 nemesisbot_wf_agentGen_sids 已是只读迁移源，UI 不再写入）+ 编辑会话登记
  const lst = await wsapi(page, 'sessions', 'list')
  expect(lst.ok, `sessions.list 失败: ${lst.error}`).toBe(true)
  const bindings = (lst.data as { bindings: Record<string, string> }).bindings ?? {}
  expect(Object.keys(bindings)).toContain('wf_agentgen:__new__')
  const registered = await page.evaluate(() => localStorage.getItem('nemesisbot_wf_edit_sessions'))
  expect((registered ?? '').length).toBeGreaterThan(2)

  // V4: workflow_edit 注入链（增量读取——只看本轮产生的证据行）
  const newEvidence = evidenceSince(evidenceBefore)
  expect(
    newEvidence.some((l) => l.includes('wf_edit_section=true')),
    `本轮 mock 证据无注入区标记:\n${newEvidence.join('\n') || '<空>'}`,
  ).toBe(true)

  // V5: 草稿面板出现（已在上方 waitFor 断言，这里补显式 expect 供报告可读）
  await expect(mockDraft).toBeVisible()

  // V6: 画布草稿预览（只读态：palette 隐藏 + diff 统计 + 退出按钮）
  await mockDraft.locator('button', { hasText: '画布预览' }).click()
  await expect(page.locator('h3', { hasText: '草稿预览：mock-news-summary' })).toBeVisible({ timeout: 20_000 })
  await page.waitForTimeout(1500) // vue-flow 布局稳定
  expect(await page.locator('.palette').isVisible().catch(() => false)).toBe(false)
  await expect(page.locator('button', { hasText: '退出预览' })).toBeVisible()
  await expect(page.locator('.diff-stat.added')).toContainText('新增')

  // V7: 应用草稿 → 注册成功 + 草稿被消费
  await page.locator('button', { hasText: '应用草稿' }).first().click()
  await page.waitForTimeout(1500)
  await page.goto(`${BASE_URL}/#/workflows`)
  const listTab = page.locator('button.tab', { hasText: '工作流列表' })
  await listTab.waitFor({ state: 'visible', timeout: 20_000 })
  await listTab.click()
  await expect(page.getByText('mock-news-summary').first()).toBeVisible({ timeout: 20_000 })
  await page.locator('button.tab', { hasText: '对话生成' }).click()
  await expect(page.locator('#wf-gen-target')).toBeVisible({ timeout: 20_000 })
  await expect(mockDraft).toBeHidden({ timeout: 20_000 })

  // V8: 编辑已有工作流 → 注入区含当前 YAML（evidence: yaml_like + last-request 核对）
  await page.locator('#wf-gen-target').selectOption('mock-news-summary')
  await page.waitForTimeout(1000)
  const box2 = page.locator('.gen-chat textarea').first()
  await box2.click()
  await box2.fill('把触发时间改到晚上十点')
  const evidenceBeforeV8 = evidenceCount()
  await page.locator('.gen-chat button', { hasText: '发送' }).first().click()
  // 本轮 mock 证据行出现 yaml_like=true（编辑目标注入了已注册定义的 trigger_type）
  await expect
    .poll(() => evidenceSince(evidenceBeforeV8).some((l) => l.includes('yaml_like=true')), {
      timeout: 30_000,
      message: '本轮 mock 证据未出现 yaml_like=true（编辑目标未注入当前 YAML）',
    })
    .toBe(true)
  const lastReq = JSON.parse(fs.readFileSync(EVIDENCE_LAST_REQUEST, 'utf-8'))
  const reqText = JSON.stringify(lastReq.messages ?? [])
  expect(reqText).toContain('Workflow Editor Session')
  expect(reqText).toContain('mock-news-summary')
  expect(reqText).toContain('trigger_type')

  // V9: 放弃手工草稿 → 面板移除（confirm 已全局接受）
  const manualDraft = page.locator('.draft-item', { hasText: 'manual-draft-demo' }).first()
  await manualDraft.locator('button', { hasText: '放弃' }).click()
  await expect(manualDraft).toBeHidden({ timeout: 20_000 })

  await page.context().close()
})
