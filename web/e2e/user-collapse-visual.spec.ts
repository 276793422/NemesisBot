/**
 * 长用户消息折叠 — 聊天气泡折叠/展开视觉自测（2026-11-20，UI 视觉自测纪律）。
 *
 * 截图人工看图（test-results/user-collapse-visual/shots/）：
 *   1. 折叠态（长消息 max-height 截断 + 底部渐隐 + 「展开全文」按钮）；
 *   2. 展开态（全文 + 「收起」按钮）；
 *   3. 短消息对照（无按钮无截断，渲染零变化）。
 *
 * 真链路：新建会话 → UI 发送超长文本（mock LLM 正常回一趟）→ 气泡即折叠。
 */
import { test, expect } from './fixtures'
import path from 'node:path'
import { newAuthedPage, openSessionSidebar, sendChat, waitAssistantReply } from './helpers'

const SHOT_DIR = path.join('test-results', 'user-collapse-visual', 'shots')

/** 生成超阈值长文本（>600 字符且 >18 行，双条件都过）。 */
function longText(): string {
  return Array.from(
    { length: 30 },
    (_, i) => `第 ${i + 1} 行：这是一段用于验证长用户消息折叠功能的测试内容，包含足够的字符数以超过折叠阈值。`,
  ).join('\n')
}

test.describe('长用户消息折叠视觉', () => {
  test('折叠态 + 展开态 + 短消息对照', async ({ browser }) => {
    const page = await newAuthedPage(browser)
    await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })
    await openSessionSidebar(page)
    await page.locator('.fn-new-chat').click()
    await page.waitForTimeout(1_500)

    // 长消息：发送 → 折叠态即刻生效（本地回显与历史行同模板）。
    await sendChat(page, longText())
    const bubble = page.locator('.message.user .message-bubble').last()
    await expect(bubble.locator('.message-text.user-collapsed')).toBeVisible()
    await expect(bubble.locator('.msg-collapse-toggle')).toHaveText('展开全文')
    await page.screenshot({ path: path.join(SHOT_DIR, '1-collapsed.png'), fullPage: true })

    // 等 mock 回复落账（避免收尾竞速），再展开。
    await waitAssistantReply(page, '', 30_000).catch(() => {})
    await bubble.locator('.msg-collapse-toggle').click()
    await expect(bubble.locator('.message-text.user-collapsed')).toHaveCount(0)
    await expect(bubble.locator('.msg-collapse-toggle')).toHaveText('收起')
    await page.screenshot({ path: path.join(SHOT_DIR, '2-expanded.png'), fullPage: true })

    // 短消息对照：无按钮、无截断类。
    await sendChat(page, '短消息对照行')
    const shortBubble = page.locator('.message.user .message-bubble').last()
    await expect(shortBubble.locator('.message-text')).toBeVisible()
    await expect(shortBubble.locator('.message-text')).not.toHaveClass(/user-collapsed/)
    await expect(shortBubble.locator('.msg-collapse-toggle')).toHaveCount(0)
    await page.screenshot({ path: path.join(SHOT_DIR, '3-short-control.png'), fullPage: true })
  })
})
