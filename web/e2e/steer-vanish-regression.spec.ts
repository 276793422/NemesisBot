/**
 * steer-vanish-regression.spec.ts — 插队时间线消失四缺陷修复回归（2026-11-23）。
 *
 * 旧 BUG（docs/BUG/2026-11-23_插队后watchdog误重灌冲掉live时间线.md，已修复归
 * REPORT）：busy 中插队 → live 时间线瞬间消失（缺陷 B：回声 dedupe 臂无条件
 * flush 排空活缓冲）→ ⚡ 注记落进全新空缓冲（「从不完整的新地方回来」，用时
 * 从插队时刻重算）→ 切页才补全（work_current 接管）。
 *
 * 本 spec 断言反转（真实常数、零源码补丁、纯观察者）：
 *   1. sleep 工具卡在 live 时间线可见（活缓冲建立）；
 *   2. 插队回执到场后 sleep 卡**持续在场**（回声帧已处理仍不排空——修复 B），
 *      busy 保持；
 *   3. 轮边界 claim 的 ⚡ 注记落进**同一缓冲**——sleep 卡同场可见 + 用时连续
 *      （≥20s；旧 BUG 新缓冲 0-4s）；
 *   4. 终局回复到场后，落地时间线含全部轮次（sleep 卡 + 注记）——**免切页**
 *      （旧 BUG 必须切页才回来）。
 *
 * ⚠️ mock 旋钮（跑本 spec 必须带，env 经 global-setup → mock 继承；缺省
 * 8000/3 会让 sleep 窗口缩短、场景静默变形）：
 *   SLOW_TOOL_MS=2000 SLOW_TOOL_SLEEP_SECS=30
 *
 * 时序（旋钮生效时）：t=0 发任务 → LLM 轮 1（2s）→ sleep 下发（卡进场
 * ~2.5s）→ t≈4s 插队（回执）→ sleep 跑 30s → ~t+33s 轮边界 claim（注记帧）
 * → 专轮 steer_reply 5s → E2E_STEER_ACK → resume 4s → E2E_STEER_MAIN_DONE
 * 终局（~t+43s，45s watchdog 首 tick 之前收尾；即便越线，修复 B 保证
 * pendingWork 存活 → 续期臂正常延期，不会误重灌）。
 */
import path from 'node:path'
import { test, expect } from './fixtures'
import { newAuthedPage, openSessionSidebar, sendChat } from './helpers'

const SHOT_DIR = path.join('test-results', 'steer-vanish-regression', 'shots')

test.describe('插队时间线存活回归', () => {
  test('busy 插队不排空活缓冲：sleep 卡保持 → 注记落同一缓冲 → 终局免切页', async ({ browser }) => {
    test.setTimeout(150_000)
    const page = await newAuthedPage(browser)
    await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })
    await openSessionSidebar(page)
    await page.locator('.fn-new-chat').click()
    await page.waitForTimeout(1_500)

    await sendChat(page, 'E2E_SLOW_TOOL 长任务：请分步处理')
    await expect(page.locator('button.btn-stop')).toBeVisible({ timeout: 10_000 })

    // sleep 工具卡进场（LLM 2s 后下发、sleep 执行 30s）——活缓冲建立。
    const sleepCard = page.locator('.wt-block.live .wt-tool').filter({ hasText: 'sleep' })
    await expect(sleepCard).toBeVisible({ timeout: 20_000 })
    await page.screenshot({ path: path.join(SHOT_DIR, '1-sleep-card-alive.png'), fullPage: true })

    // 插队（! 前缀走 steer 通道）→ 回执到场。回执帧晚于回声帧——此刻
    // dedupe 臂已经跑过：修复 B 前此处活缓冲已被排空。
    await sendChat(page, '! 紧急插话：注意边界情况的处理')
    const receipt = page.locator('.busy-receipt-note')
    await expect(receipt).toBeVisible({ timeout: 10_000 })
    await expect(receipt).toContainText('已接收为紧急插话')
    await expect(page.locator('button.btn-stop')).toBeVisible()

    // ★ 修复 B 主断言：回声帧处理完 + 3s 观察窗后 sleep 卡仍在（旧 BUG
    //   亚秒级消失且持续不在），busy 保持。
    await page.waitForTimeout(3_000)
    await expect(sleepCard).toBeVisible()
    await expect(page.locator('button.btn-stop')).toBeVisible()
    await page.screenshot({ path: path.join(SHOT_DIR, '2-echo-buffer-survives.png'), fullPage: true })

    // 轮边界 claim（~t+33s）：⚡ 注记落进 ★ 同一缓冲——sleep 卡同场可见
    //（旧 BUG 注记进全新空缓冲）+ 用时连续（≥20s；旧 BUG 新缓冲 0-4s）。
    const liveNote = page.locator('.wt-block.live .wt-steer-note')
    await expect(liveNote).toBeVisible({ timeout: 45_000 })
    await expect(liveNote).toContainText('紧急插话：注意边界情况的处理')
    await expect(page.locator('.wt-block.live .wt-header .wt-steer')).toContainText('⚡')
    await expect(sleepCard).toBeVisible()
    const title = (await page.locator('.wt-block.live .wt-header .wt-title').textContent()) ?? ''
    const secs = Number(title.match(/用时 (\d+)秒/)?.[1] ?? '0')
    expect(secs).toBeGreaterThanOrEqual(20)
    await page.screenshot({ path: path.join(SHOT_DIR, '3-note-same-buffer.png'), fullPage: true })

    // 专轮收口回应 + 终局回复（steer_reply 5s → ACK；resume 4s → MAIN_DONE）。
    await expect(page.locator('.wt-block.live .wt-steer-response')).toContainText(
      'E2E_STEER_ACK',
      { timeout: 20_000 },
    )
    await expect(page.locator('.message.assistant .message-bubble').last()).toContainText(
      'E2E_STEER_MAIN_DONE',
      { timeout: 40_000 },
    )

    // ★ 免切页断言：终局挂载后落地时间线含全部轮次（sleep 卡 + ⚡ 徽标 +
    //   注记行）——旧 BUG 此刻 live 缓冲早已丢失、必须切页才回来。
    //   落地块默认折叠（open=live 才默认开），先展开再断言内容。
    const landedHeader = page.locator('button.wt-header').last()
    await expect(landedHeader.locator('.wt-steer')).toContainText('⚡')
    await landedHeader.click()
    await expect(page.locator('.wt-tool').filter({ hasText: 'sleep' })).toHaveCount(1)
    await expect(page.locator('.wt-block:not(.live) .wt-steer-note').last()).toContainText(
      '紧急插话：注意边界情况的处理',
    )
    await page.screenshot({ path: path.join(SHOT_DIR, '4-final-no-page-switch.png'), fullPage: true })
  })
})
