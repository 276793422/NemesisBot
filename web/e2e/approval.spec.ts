/**
 * e2e/approval.spec.ts — 审批卡全链路（真实 UI 路径）。
 *
 * mock LLM 按标记路由出 exec 工具调用（命令含 E2E_APPROVAL_MARKER）→
 * 安全管线 process_rules.exec 的 ask 规则拦停 → WebApprovalManager →
 * SSE approval-requested → Dashboard 审批卡 → 用户点批准 → 工具真实执行
 * → agent 续行 → mock 收尾轮确定性回复。
 * （e2e workspace 的安全配置由 global-setup 预置该 ask 规则。）
 */
import { expect, test } from '@playwright/test'
import { newAuthedPage, sendChat, waitAssistantReply } from './helpers'

test('审批卡：ask 规则拦截 exec → 批准 → 工具执行 → 续行回复', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  await sendChat(page, 'E2E_TOOL_EXEC 请执行带标记的命令')

  // 审批卡弹出，且展示的命令含标记子串（glob_contains 子串匹配的语义对齐）
  const modal = page.locator('.approval-modal')
  await expect(modal).toBeVisible({ timeout: 45_000 })
  await expect(modal).toContainText('E2E_APPROVAL_MARKER', { timeout: 10_000 })

  // 批准 → 工具真实执行 → agent 续行 → 确定性收尾回复
  await modal.locator('.btn-success').click()
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY', 45_000)

  // 审批卡收走
  await expect(modal).toBeHidden({ timeout: 15_000 })

  await page.context().close()
})
