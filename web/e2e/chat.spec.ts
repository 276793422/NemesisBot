/**
 * e2e/chat.spec.ts — 聊天主链路（UI 真实路径）。
 *
 * stream:'yes' 配置 → provider 走 SSE 流式 → gateway 聚合
 * （stream_util::collect_stream_response）→ 前端回复渲染。
 * mock LLM 确定性直答（E2E_MOCK_TEXT_REPLY 前缀），断言零采样随机性。
 */
import { expect, test } from '@playwright/test'
import { newAuthedPage, sendChat, waitAssistantReply } from './helpers'

test('聊天往返：UI 发送 → mock SSE 流式 → assistant 确定性回复', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  await sendChat(page, '你好，请做个自我介绍')
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY', 45_000)

  // 用户消息回显也在（顺序：用户在前）
  await expect(page.locator('.message.user').last()).toContainText('自我介绍')

  await page.context().close()
})
