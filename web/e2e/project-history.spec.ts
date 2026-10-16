/**
 * e2e/project-history.spec.ts — 项目会话历史修复回归（verify-ui-history-fix U1-U5 迁移）。
 *
 * 种子（公开 API，全部真实路径）：建目录 → projects.create →
 * sessions.create（project_id）→ WS 直发 5 轮对话 → 等落账 → 删目录 →
 * **重启 gateway**（「场景 A」机制：目录缺失必须在 gateway 启动前成立——
 * 注册表条目在、目录不在 → 项目 loop 不 spawn → running=false → 侧栏组头
 * 置灰。运行中删目录是 lenient 设计，loop 继续跑、running 保持 true，
 * 复现不了置灰态）。磁盘态（注册表/会话/聊天账本）重启后全部存活。
 *
 * 断言（BUG 2026-09-23 修复面）：
 *   U1 侧栏项目组可见（loop 未 spawn → unavailable ⚠ 态，组不消失）
 *   U2 点项目会话 → 历史消息行渲染（≥10 行 = 5 user + 5 assistant）
 *   U3 无「当前不可用」伪 assistant 消息
 *   U4 无「历史消息加载失败」横幅
 *   U5 切走（新建对话）再切回 → 历史仍在
 */
import fs from 'node:fs'
import path from 'node:path'
import { expect, test } from '@playwright/test'
import { E2E } from './env'
import { newAuthedPage, restartGateway, wsapi, wsChatSend } from './helpers'

const PROJ_NAME = '专利-e2e'
const PROJ_DIR = path.join(E2E.home, 'e2e-history-project')
const ROUNDS = 5

test('项目会话历史：目录删除后组不可用但历史完整可读', async ({ browser }) => {
  test.setTimeout(180_000)

  // ---- 种子 ----
  fs.mkdirSync(PROJ_DIR, { recursive: true })
  const page = await newAuthedPage(browser)
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  const pc = await wsapi(page, 'projects', 'create', { name: PROJ_NAME, path: PROJ_DIR })
  expect(pc.ok, `projects.create 失败: ${pc.error}`).toBe(true)
  const projectId = (pc.data as { project: { id: string } }).project.id

  const sc = await wsapi(page, 'sessions', 'create', { title: 'e2e-project-history', project_id: projectId })
  expect(sc.ok, `sessions.create 失败: ${sc.error}`).toBe(true)
  const sid = (sc.data as { session_id: string }).session_id

  for (let i = 1; i <= ROUNDS; i++) {
    const reply = await wsChatSend(page, sid, `项目历史种子第 ${i} 轮`)
    expect(reply).toContain('E2E_MOCK_TEXT_REPLY')
  }

  // 删目录 → 重启 gateway（场景 A：注册表在、目录不在 → loop 不 spawn）；
  // reload 拿全新侧栏状态（磁盘态重启后存活）
  fs.rmSync(PROJ_DIR, { recursive: true, force: true })
  await restartGateway()
  await page.reload()
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  const toggle = page.locator('.toolbar-toggle').first()
  if (await toggle.isVisible().catch(() => false)) {
    await toggle.click()
  }
  await expect(page.locator('.group-header').first()).toBeVisible({ timeout: 20_000 })

  // U1: 项目组可见 + unavailable 态
  const projGroup = page.locator('.group-header', { hasText: PROJ_NAME }).first()
  await expect(projGroup).toBeVisible({ timeout: 20_000 })
  expect(await projGroup.evaluate((el) => el.classList.contains('unavailable'))).toBe(true)

  // U2: 项目会话历史渲染（组头相邻的会话行）
  const sessionRow = projGroup.locator('xpath=following-sibling::div[contains(@class,"session-item")][1]')
  await expect(sessionRow).toBeVisible({ timeout: 20_000 })
  await sessionRow.click()
  await expect(page.locator('.message').last()).toBeVisible({ timeout: 30_000 })
  const msgCount = await page.locator('.message').count()
  expect(msgCount).toBeGreaterThanOrEqual(ROUNDS * 2)

  // U3/U4: 无伪消息、无失败横幅
  await expect(page.locator('.message', { hasText: '当前不可用' }).first()).toBeHidden()
  await expect(page.locator('.message', { hasText: '历史消息加载失败' }).first()).toBeHidden()

  // U5: 切走（新建对话）再切回 → 历史仍在
  const newChat = page.locator('.fn-new-chat')
  if (await newChat.isVisible().catch(() => false)) {
    await newChat.click()
    await expect(page.locator('.session-item').first()).toBeVisible({ timeout: 20_000 })
  }
  await sessionRow.click()
  await expect(page.locator('.message').last()).toBeVisible({ timeout: 30_000 })
  const msgCountBack = await page.locator('.message').count()
  expect(msgCountBack).toBeGreaterThanOrEqual(ROUNDS * 2)
  await expect(page.locator('.message', { hasText: '历史消息加载失败' }).first()).toBeHidden()

  await page.context().close()
})
