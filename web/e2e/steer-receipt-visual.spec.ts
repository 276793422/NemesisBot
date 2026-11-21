/**
 * steer-receipt-visual.spec.ts — 插话专轮 GUI 自测（2026-11-20 回执徽标批；
 * 2026-11-21 插话专轮批三次改版：让路机制全移除 → 插话专轮模型）。
 *
 * 截图人工看图（test-results/steer-receipt-visual/shots/）：
 *   1. busy-baseline：turn 进行中基线（停止按钮亮）；
 *   2. steer-receipt：⚡ 插话回执徽标到场——灰徽标、无完成语义触发
 *      （busy 保持、停止按钮存活）；
 *   3. steer-note-live：轮边界 claim → 注记落进 live 时间线过程流（琥珀
 *      ⚡ N 徽标 + 注记行）——**sleep 工具行同场可见**（已下发即执行：
 *      让路机制已移除，sleep 真执行 3s 后轮边界 claim，工具行 ✓ 收尾）；
 *   4. steer-response-live：插话专轮收口回应（⚡ 回应行）渲染在注记行
 *      正下方——「我的插话 → 中间回应」成对可见（live 成对窗口 = RESUME
 *      续跑轮的 mock 延迟 STEER_RESUME_MS 默认 4s——steer_reply 5s 延迟
 *      只是注记单行窗口，回应与终局间隔由续跑延迟决定）；
 *   5. final-digested：终局回复（RESUME 续跑轮的主线文本）到场——回执
 *      徽标扫除、落地时间线 ⚡ 徽标，展开即见注记行 + 回应行；
 *   6. history-reload：刷新后——work 键重放注记行 + 回应行（落盘语义）、
 *      回执徽标瞬态不落盘不再现。
 *
 * 真链路（2026-11-21 插话专轮模型）：E2E_SLOW_TOOL（mock 8s 延迟 + sleep
 * 工具调用）→ 8s 窗口内 UI 发 `! 插话`（busy_receipt 回执帧）→ 轮 1 返回
 * 后 sleep 真执行 3s（工具行出现）→ 轮边界 claim（steer_inject 注记帧 +
 * jsonl 落行 + work 注记轮）→ 专轮请求含 [E2E_STEER_CONTRACT] → mock
 * steer_reply 延迟 5s 纯文本 = 阶段收口（steer_response 瞬态帧，回应行
 * 成对渲染，live 窗口 = 续跑轮 steer_resume 的 4s 延迟）→ 队列空 → 注入
 * [E2E_STEER_RESUME] → 续跑轮 4s 后终答（E2E_STEER_MAIN_DONE）。
 */
import { test, expect } from './fixtures'
import path from 'node:path'
import { newAuthedPage, openSessionSidebar, sendChat } from './helpers'

const SHOT_DIR = path.join('test-results', 'steer-receipt-visual', 'shots')

test.describe('插话专轮视觉', () => {
  test('⚡ 回执 → sleep 真执行 + 注记行 → 回应行 → 终局 → 刷新保留', async ({ browser }) => {
    test.setTimeout(180_000)
    const page = await newAuthedPage(browser)
    await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })
    await openSessionSidebar(page)
    await page.locator('.fn-new-chat').click()
    await page.waitForTimeout(1_500)

    // 多轮 turn：轮 1 慢速 LLM（8s 延迟窗口给插话留时间）→ sleep 工具
    // 调用（已下发即执行——真跑 3s）→ 轮边界 claim 插话 → 专轮 → 续跑。
    await sendChat(page, 'E2E_SLOW_TOOL 长任务：请分步处理')
    // busy 态确认（停止按钮亮）。
    await expect(page.locator('button.btn-stop')).toBeVisible({ timeout: 10_000 })
    await page.screenshot({ path: path.join(SHOT_DIR, '1-busy-baseline.png'), fullPage: true })
    // 等 turn 进入轮 1 LLM 调用期（claim 在轮 1 顶已过——插话须晚于首轮
    // claim 才会排队走专轮路径；800ms > 装配开销、≪ 8s 窗口）。
    await page.waitForTimeout(800)

    // 插话：! 前缀走 steer 通道 → ⚡ busy_receipt 回执帧。
    await sendChat(page, '! 紧急插话：注意边界情况的处理')
    // 回执徽标到场（灰徽标形态，非 assistant 气泡）。
    const receipt = page.locator('.busy-receipt-note')
    await expect(receipt).toBeVisible({ timeout: 10_000 })
    await expect(receipt).toContainText('已接收为紧急插话')
    // 完成语义未触发：busy 保持（停止按钮不消失）。
    await expect(page.locator('button.btn-stop')).toBeVisible()
    // 回执不是 assistant 气泡（打断感的直接反证）。
    await expect(page.locator('.message.assistant').filter({ hasText: '已接收为紧急插话' })).toHaveCount(0)
    await page.screenshot({ path: path.join(SHOT_DIR, '2-steer-receipt.png'), fullPage: true })

    // 轮 1 返回（t≈8s）→ sleep 真执行 3s（工具行出现——已下发即执行）
    // → 轮边界 claim（注记帧 + work 注记轮）→ 专轮请求（contract marker
    // 路由）→ mock steer_reply 延迟 5s。注记行在 live 时间线可见数秒，
    // 截图窗口确定性。
    const liveNote = page.locator('.wt-block.live .wt-steer-note')
    await expect(liveNote).toBeVisible({ timeout: 45_000 })
    await expect(liveNote).toContainText('插话已注入本轮思考')
    // 注记自带插话内容摘要（后端 strip_steer_marker 后原文，前缀 ！ 不进摘要）。
    await expect(liveNote).toContainText('紧急插话：注意边界情况的处理')
    // live 头部 ⚡ 计数徽标（折叠态可发现插话落点的入口）。
    await expect(page.locator('.wt-block.live .wt-header .wt-steer')).toContainText('⚡')
    // sleep 工具行已执行（让路机制移除的直接证据：已下发的批次恒执行，
    // 工具行 ✓ 收尾在注记轮之前——sleep 无标签映射，行内显示原名 sleep）。
    await expect(page.locator('.wt-block.live .wt-tool').filter({ hasText: 'sleep' })).toHaveCount(1)
    await page.screenshot({ path: path.join(SHOT_DIR, '3-steer-note-live.png'), fullPage: true })

    // 专轮收口回应：steer_response 瞬态帧 → 回应行渲染在注记行正下方
    // （成对可读——「我的插话 → 中间回应」）；回应是主文本色非琥珀注记体。
    const liveResponse = page.locator('.wt-block.live .wt-steer-response')
    await expect(liveResponse).toBeVisible({ timeout: 15_000 })
    await expect(liveResponse).toContainText('E2E_STEER_ACK')
    // 回应行紧跟注记行（同一注记轮内的成对渲染）。
    await expect(liveResponse).toContainText('⚡ 回应')
    await page.screenshot({ path: path.join(SHOT_DIR, '4-steer-response-live.png'), fullPage: true })

    // 终局回复（RESUME 续跑轮的主线文本）到场：回执徽标扫除、独立注记行
    // 不再露出（隐藏占行）；落地时间线折叠头部带 ⚡ 徽标，展开即见注记行
    // + 回应行（回应随 work 键挂载——落盘语义，回看面同一落点）。
    await expect(page.locator('.message.assistant .message-bubble').last()).toContainText(
      'E2E_STEER_MAIN_DONE',
      { timeout: 30_000 },
    )
    await expect(page.locator('.busy-receipt-note')).toHaveCount(0)
    await expect(page.locator('.steer-inject-note')).toHaveCount(0)
    const landedHeader = page.locator('button.wt-header').last()
    await expect(landedHeader.locator('.wt-steer')).toContainText('⚡')
    await landedHeader.click()
    const landedNote = page.locator('.wt-block:not(.live) .wt-steer-note')
    await expect(landedNote).toBeVisible()
    await expect(landedNote).toContainText('紧急插话：注意边界情况的处理')
    const landedResponse = page.locator('.wt-block:not(.live) .wt-steer-response')
    await expect(landedResponse).toBeVisible()
    await expect(landedResponse).toContainText('E2E_STEER_ACK')
    await page.screenshot({ path: path.join(SHOT_DIR, '5-final-digested.png'), fullPage: true })

    // 刷新恢复：历史行重放——注记 jsonl 行隐藏（占行锚不动）、work 键
    // 注记轮 + 回应段随 assistant 行到场（展开时间线可见）；回执徽标
    // 瞬态不落盘。
    await page.reload()
    await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })
    await expect(page.locator('.steer-inject-note')).toHaveCount(0)
    await expect(page.locator('.busy-receipt-note')).toHaveCount(0)
    await expect(page.locator('button.wt-header').last().locator('.wt-steer')).toContainText('⚡')
    await page.locator('button.wt-header').last().click()
    await expect(page.locator('.wt-block:not(.live) .wt-steer-note').last()).toContainText(
      '紧急插话：注意边界情况的处理',
    )
    await expect(page.locator('.wt-block:not(.live) .wt-steer-response').last()).toContainText(
      'E2E_STEER_ACK',
    )
    await page.screenshot({ path: path.join(SHOT_DIR, '6-history-reload.png'), fullPage: true })
  })
})
