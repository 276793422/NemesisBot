/**
 * e2e/disconnect-recovery.spec.ts — 断线恢复（L2 韧性面，整改复核 §四.3）
 *
 * resync.spec 只测「刷新重载→历史重拉」；本文件补两类真正的断线形态：
 *
 * A. WS 断线、进程存活（context.setOffline 模拟网络断）：断线期间其他端
 *    （第二个 context）继续产生事件 → 前端自动重连 → ChatPanel
 *    syncMissedChat 经 chat.sync 按 seq 增量补拉，DOM 无重复渲染 + 断线后
 *    续流正常。
 * B. gateway 进程重启（restartGateway）：per-session 事件环随进程消失 →
 *    持旧 seq 的 chat.sync 必须 gap=true 诚实上报缺口（不假装连续）→
 *    历史（磁盘持久层）恰好一次 → 重启后续流正常。
 *
 * chat.sync 返回契约（handlers/chat.rs）：{session_id, session_key,
 * after_seq, gap, events:[{seq, role, content, ...}]}——events 为
 * seq > after_seq 的帧，gap=true 表示缺口滑出重放窗口/会话环不存在。
 *
 * 种子纪律（实录教训）：会话必须经页面 UI 创建并选中——页面外 wsapi
 * sessions.create 不进本页面 store，侧栏也无行可选，`.message.user` 断言
 * 必然扑空（首版 3 失败的根因）。sid 一律取自侧栏选中行
 * `.session-item.active` 的 data-session-id（与 locator 同源，零失配）。
 */
import { expect, test } from '@playwright/test'
import {
  newAuthedPage,
  restartGateway,
  sendChat,
  waitAssistantReply,
  wsapi,
  wsChatSend,
  type WsapiResult,
} from './helpers'

interface SyncEvent {
  seq: number
  role: string
  content: string
}

function syncEvents(r: WsapiResult): SyncEvent[] {
  return ((r.data as { events?: SyncEvent[] })?.events ?? []) as SyncEvent[]
}

function syncGap(r: WsapiResult): boolean {
  return Boolean((r.data as { gap?: boolean })?.gap)
}

/** 当前会话环内最大 seq（sync 全量重放取末帧；空环 = 0）。 */
function maxSeq(events: SyncEvent[]): number {
  return events.length > 0 ? events[events.length - 1]!.seq : 0
}

/** UI 驱动种子：展开侧栏 → 新建对话 → 发送并等回复 → 返回会话 sid。 */
async function seedViaUi(page: Awaited<ReturnType<typeof newAuthedPage>>, marker: string) {
  // 展开侧栏（若收起）——「显示会话列表」toggle，resync.spec 同款。
  const toggle = page.locator('.toolbar-toggle').first()
  if (await toggle.isVisible().catch(() => false)) {
    await toggle.click()
  }
  // 显式新建对话：新页面挂载 auto-select sessions[0]（最近活跃），可能落
  // 在前面 spec 留下的会话上；点「＋ 新建对话」= create() 内 switchTo，
  // currentId 必然落干净会话（resync.spec 实录纪律）。
  await page.locator('.fn-new-chat').click()
  await expect(page.locator('.fn-new-chat')).toBeVisible()

  await sendChat(page, marker)
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY', 45_000)

  // sid = 选中行的 data-session-id（发送后该行必然 active）
  const sid = await page.locator('.session-item.active').getAttribute('data-session-id')
  expect(sid, '侧栏选中行应有 data-session-id').toBeTruthy()
  return sid!
}

test('WS 断线重连：断线期间事件经 chat.sync 补拉，DOM 无重复，续流正常', async ({ browser }) => {
  test.setTimeout(180_000)
  const page = await newAuthedPage(browser)
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  // ---- 种子：UI 建会话 + 断线前消息 A ----
  const sid = await seedViaUi(page, `断线前消息A-${Date.now()}`)

  // 基线：断线前事件环快照（maxSeq 之后的新帧 = 补拉断言的增量窗口）
  const base = await wsapi(page, 'chat', 'sync', { session_id: sid, after_seq: 0 })
  expect(base.ok, `chat.sync 失败: ${base.error}`).toBe(true)
  const seqBefore = maxSeq(syncEvents(base))
  expect(seqBefore).toBeGreaterThan(0)

  // ---- 断线窗口：主页面断网；第二 context（正常网络）继续产生事件 ----
  await page.context().setOffline(true)
  await page.waitForTimeout(1_600) // 前端重连退避 1s 起步——第一轮重连必须撞在断网窗口内

  const seedMarker = `断线期间消息B-${Date.now()}`
  const page2 = await newAuthedPage(browser)
  const reply2 = await wsChatSend(page2, sid, seedMarker)
  expect(reply2).toContain('E2E_MOCK_TEXT_REPLY')
  await page2.context().close()

  // ---- 恢复：前端自动重连 → syncMissedChat 增量补拉 → B 渲染 ----
  await page.context().setOffline(false)
  const bubbleB = page.locator('.message.user', { hasText: seedMarker })
  await expect(bubbleB.first()).toBeVisible({ timeout: 45_000 })

  // 去重：重连恢复（补拉或全量刷新任一路径）不得重复渲染同一消息
  await page.waitForTimeout(2_000) // 给潜在的重复渲染窗口留时间
  expect(await bubbleB.count()).toBe(1)

  // 服务端补拉契约：after_seq=基线 的增量恰含断线期间事件（B 的 user+assistant 帧）
  const incr = await wsapi(page, 'chat', 'sync', { session_id: sid, after_seq: seqBefore })
  expect(incr.ok, `chat.sync 增量失败: ${incr.error}`).toBe(true)
  const events = syncEvents(incr)
  expect(events.length, '断线期间应有新事件入环').toBeGreaterThan(0)
  expect(
    events.some((e) => e.content.includes(seedMarker)),
    `增量补拉应含断线期间消息，实际 ${events.length} 帧无命中`,
  ).toBe(true)
  for (let i = 1; i < events.length; i++) {
    expect(events[i]!.seq).toBeGreaterThan(events[i - 1]!.seq)
  }

  // ---- 断线后续流：恢复的连接上正常收发 + 页面实时渲染 ----
  const markerC = `断线后消息C-${Date.now()}`
  const replyC = await wsChatSend(page, sid, markerC)
  expect(replyC).toContain('E2E_MOCK_TEXT_REPLY')
  await expect(page.locator('.message.user', { hasText: markerC }).first()).toBeVisible({
    timeout: 45_000,
  })

  await page.context().close()
})

test('gateway 重启：环形缓冲随进程消失 → chat.sync 诚实上报 gap，历史恰好一次，续流正常', async ({
  browser,
}) => {
  test.setTimeout(240_000)
  const page = await newAuthedPage(browser)
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  // ---- 种子（UI 驱动）+ 重启前事件环基线 ----
  const markerA = `重启前消息A-${Date.now()}`
  const sid = await seedViaUi(page, markerA)
  const base = await wsapi(page, 'chat', 'sync', { session_id: sid, after_seq: 0 })
  expect(base.ok).toBe(true)
  const seqBefore = maxSeq(syncEvents(base))
  expect(seqBefore).toBeGreaterThan(0)

  // ---- gateway 重启（进程死：WS 全断 + 内存事件环清空；磁盘态存活）----
  await restartGateway()
  await page.reload()
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  // 重新选中会话（reload 后 currentId 不持久化），历史从磁盘重拉
  const toggle = page.locator('.toolbar-toggle').first()
  if (await toggle.isVisible().catch(() => false)) {
    await toggle.click()
  }
  const target = page.locator(`.session-item[data-session-id="${sid}"]`)
  await expect(target).toBeVisible({ timeout: 20_000 })
  await target.click()

  // 去重：重启 + 重拉后该消息恰好一次（持久层唯一真相，不与残帧叠加）
  const bubbleA = page.locator('.message.user', { hasText: markerA })
  await expect(bubbleA.first()).toBeVisible({ timeout: 30_000 })
  await page.waitForTimeout(2_000)
  expect(await bubbleA.count()).toBe(1)

  // 诚实缺口上报：持旧 seq 的补拉必须 gap=true（环形缓冲随进程消失，
  // 不假装连续——前端据此走全量刷新兜底，handlers/chat.rs 契约）
  const after = await wsapi(page, 'chat', 'sync', { session_id: sid, after_seq: seqBefore })
  expect(after.ok, `chat.sync 失败: ${after.error}`).toBe(true)
  expect(syncGap(after)).toBe(true)
  expect(syncEvents(after)).toHaveLength(0)

  // ---- 重启后续流：新环从 1 重新起 seq，页面实时渲染 ----
  const markerC = `重启后消息C-${Date.now()}`
  const replyC = await wsChatSend(page, sid, markerC)
  expect(replyC).toContain('E2E_MOCK_TEXT_REPLY')
  await expect(page.locator('.message.user', { hasText: markerC }).first()).toBeVisible({
    timeout: 45_000,
  })

  await page.context().close()
})
