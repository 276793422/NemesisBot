/**
 * e2e/open-files.spec.ts — I5 上下文文件前端接线真机链路（2026-10-17）。
 *
 * 真实用户路径全链验证：侧栏「工作区」按钮展开文件树 → 文件行 📌 设为
 * 上下文 → 输入框发消息 → chat.send data.open_files 随载荷上行 → 后端
 * per-turn 注入 merged 快照节 `# Open Files (client-reported)` → provider
 * 请求真实到达 mock LLM → evidence 证据行 open_files=true +
 * mock-evidence.last-request.json 留全量请求体供人工核对。
 *
 * 负例同链路覆盖：未设定 / 清除后 → 载荷不带字段 → 注入节缺席
 * （evidence open_files=false，老协议零影响）。
 *
 * 判定锚点是 mock LLM 收到的请求体（不是前端状态、不是后端 metadata）——
 * 注入节出现在 LLM 请求 messages 里才是「到达模型」的直接证据。
 */
import fs from 'node:fs'
import { expect, test } from '@playwright/test'
import { EVIDENCE_LAST_REQUEST } from './env'
import { evidenceCount, evidenceSince, newAuthedPage, sendChat, waitAssistantReply } from './helpers'

/** 等 mock 证据增量区出现指定 open_files 形态（回复渲染先于断言到达，但保
 *  expect.poll 容忍证据文件 flush 时序）。 */
async function expectEvidenceOpenFiles(before: number, value: 'true' | 'false') {
  const needle = `open_files=${value}`
  await expect
    .poll(() => evidenceSince(before).some((l) => l.includes(needle)), { timeout: 15_000 })
    .toBe(true)
}

test('📌 上下文文件 → 注入节真实到达 LLM 请求；清除后不再注入', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  // 1. 打开会话侧栏（ChatPanel 工具条开关，侧栏默认收起）→ 点「工作区」
  //    按钮展开文件树（真实用户入口，非共享单例直调）
  await page.locator('.toolbar-toggle[title="显示会话列表"]').click()
  await page.locator('.fn-btn[title="工作区文件树"]').click()
  const panel = page.locator('.filetree-panel')
  await expect(panel).toBeVisible()
  const idRow = page.locator('.filetree-row', { hasText: 'IDENTITY.md' }).first()
  await expect(idRow).toBeVisible({ timeout: 15_000 })

  // 2. 文件行 📌 设为上下文 → 头部 chip 可视
  await idRow.locator('.filetree-pin').click()
  const chip = page.locator('.filetree-context')
  await expect(chip).toBeVisible()
  await expect(chip).toContainText('IDENTITY.md')

  // 3. 发消息（真实输入框路径）→ mock 确定性回复 → 注入节到达 LLM 请求
  const beforePinned = evidenceCount()
  await sendChat(page, '请描述当前上下文文件里写了什么')
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY', 45_000)
  await expectEvidenceOpenFiles(beforePinned, 'true')

  // 4. 全量请求体核对：注入节标题 + 上报路径都在 messages 里
  const lastReq = JSON.parse(fs.readFileSync(EVIDENCE_LAST_REQUEST, 'utf-8')) as {
    messages: { role: string; content: unknown }[]
  }
  const reqText = JSON.stringify(lastReq.messages)
  expect(reqText).toContain('# Open Files (client-reported)')
  expect(reqText).toContain('IDENTITY.md')

  // 5. chip × 清除 → 下一条消息不再携带（后端无 metadata → 无注入节）
  await chip.locator('button[title="清除上下文文件（后续消息不再携带）"]').click()
  await expect(chip).toBeHidden()

  const beforeCleared = evidenceCount()
  await sendChat(page, '上下文已清除，这条不该带文件')
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY', 45_000)
  await expectEvidenceOpenFiles(beforeCleared, 'false')

  await page.context().close()
})

test('未设定 📌 → 请求不含注入节（老协议零影响）', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  // 全新页面（每测试独立 JS 环境）直接发送——不展开文件树、不设上下文。
  const before = evidenceCount()
  await sendChat(page, '普通消息，无上下文文件')
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY', 45_000)
  await expectEvidenceOpenFiles(before, 'false')

  await page.context().close()
})
