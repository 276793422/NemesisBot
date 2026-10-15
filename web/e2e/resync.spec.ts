/**
 * e2e/resync.spec.ts — 刷新后的历史重载（L6++ ChatPanel 重挂载补偿的用户路径）。
 *
 * 显式新建会话 → UI 发送 → 回复落账 → page.reload() → 按 data-session-id
 * 重新选中该会话 → 历史全量重拉，用户消息与 assistant 回复都在
 * （session_log 持久化 + 前端补拉链路）。
 *
 * 会话 id 获取方式 = sessions.list 前后差分（发送前快照、发送后差集——
 * create 是乐观插入，session_logs 文件首条消息才物化，差分必须放发送后）。
 * 不读 Pinia：生产 bundle 的 __vue_app__.config.globalProperties 链不可用
 * （page.evaluate 诊断实录 {"hasEl":true,"hasApp":true,"gpDollarKeys":null,
 * "hasPinia":false}，vitest 源码形态可用但产物形态不同）；DOM 文本匹配也
 * 不行——多 spec 残留「新对话」重名。
 */
import { expect, test } from '@playwright/test'
import { newAuthedPage, sendChat, waitAssistantReply, wsapi, type WsapiResult } from './helpers'

/** sessions.list 响应里的裸 sid 集合（data = { sessions: [{id,...}], bindings }） */
function sessionIds(r: WsapiResult): string[] {
  const arr = (r.data as { sessions?: { id?: string }[] })?.sessions ?? []
  return arr.map((s) => s.id ?? '')
}

test('刷新重载：发送/回复落账 → reload → 历史重拉可见', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  // 显式新建会话再发送——新页面挂载会 auto-select sessions[0]（ChatView
  // onMounted），那是最近活跃会话；前面的 spec 恰好留下断目录项目会话时
  // （场景 A 态），发送命中「项目当前不可用」诚实报错（实录）。点
  // 「＋ 新建对话」= create() 内 switchTo，currentId 必然落干净对话组会话。
  const toggle = page.locator('.toolbar-toggle').first()
  if (await toggle.isVisible().catch(() => false)) {
    await toggle.click()
  }

  // 差分锚定新会话 id——但必须放在发送之后：sessions.create 是乐观插入
  // （store 注释原文「the server-side file materializes on the first
  // message」），sessions.list 扫的是 session_logs 文件，首条消息落盘前
  // 差分恒空（实录：点击后轮询 15s added=[] 而侧栏已有「新对话」行）。
  const before = new Set(sessionIds(await wsapi(page, 'sessions', 'list')))
  await page.locator('.fn-new-chat').click()
  await expect(page.locator('.fn-new-chat')).toBeVisible()

  const marker = `resync 往返验证 ${Date.now()}`
  await sendChat(page, marker)
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY', 45_000)

  // 回复已落账 → 文件已物化 → 差分恰好多一条 = 新会话 sid
  let sid = ''
  await expect(async () => {
    const added = sessionIds(await wsapi(page, 'sessions', 'list')).filter((id) => !before.has(id))
    expect(added, `新建会话差分: before=${before.size}条 added=${JSON.stringify(added)}`).toHaveLength(1)
    sid = added[0]
  }).toPass({ timeout: 15_000 })

  // 刷新（currentId 不持久化 → 重载后需重新选中会话）
  await page.reload()
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  // 展开侧栏（若收起），按会话 id 精确点行——「first() = 最近活跃」的旧
  // 假设在 L6++ 侧栏布局下不成立：项目组恒在顶部，DOM 序 ≠ 活跃序（实录）。
  const toggleAfter = page.locator('.toolbar-toggle').first()
  if (await toggleAfter.isVisible().catch(() => false)) {
    await toggleAfter.click()
  }
  const targetSession = page.locator(`.session-item[data-session-id="${sid}"]`)
  await expect(targetSession).toBeVisible({ timeout: 20_000 })
  await targetSession.click()

  // 历史全量重拉：两条消息都在
  await expect(page.locator('.message.user').last()).toContainText(marker, { timeout: 30_000 })
  await expect(page.locator('.message.assistant').last()).toContainText('E2E_MOCK_TEXT_REPLY', {
    timeout: 30_000,
  })

  await page.context().close()
})
