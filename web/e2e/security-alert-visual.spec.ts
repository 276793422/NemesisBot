/**
 * #53 安全事件流 — AppLayout 安全告警条视觉自测（2026-11-20，UI 视觉自测纪律）。
 *
 * 截图人工看图（test-results/security-alert-visual/shots/）：
 *   1. critical 告警卡（🛑 图标 + 红色边框/底 + estop 摘要文本）——真链路：
 *      WSAPI estop.trigger → alert hub（critical 命中内置规则）→ SSE
 *      security_alert → AppLayout 告警条；
 *   2. high 变体（is-high 琥珀红边框/底）——合成注 class 验证 CSS 组合
 *      （真实 High 事件需要审批拒绝全链路，脆弱；class 换装即可验证样式面，
 *      逻辑面由 useSecurityAlerts.spec.ts 覆盖）；
 *   3. 关闭后告警条消失。
 *
 * 测后释放急停（estop 状态进程级，不清理会冻结后续 spec 的 agent 活动）。
 */
import { test, expect } from './fixtures'
import path from 'node:path'
import { newAuthedPage, gotoRoute, wsapi } from './helpers'

const SHOT_DIR = path.join('test-results', 'security-alert-visual', 'shots')

test.describe('安全告警事件卡视觉', () => {
  test('estop critical 告警卡真链路展示 + high 变体 + 关闭', async ({ browser }) => {
    const page = await newAuthedPage(browser)
    await gotoRoute(page, '/')

    // 初始：无安全告警条。
    await expect(page.locator('.security-alert-bar')).toHaveCount(0)

    // 真链路触发：estop.trigger → EstopTriggered Critical → 内置规则路由
    // → SSE → 前端告警条出现。
    await wsapi(page, 'estop', 'trigger')
    const item = page.locator('.security-alert-bar .system-alert-item.is-critical')
    await expect(item).toBeVisible()
    await expect(page.locator('.security-alert-bar .system-alert-text')).toContainText(
      'estop',
    )
    await page.screenshot({
      path: path.join(SHOT_DIR, '1-critical-alert.png'),
      fullPage: true,
    })

    // high 变体：合成换 class 验证 is-high 的 CSS 组合（边框/底色琥珀红）。
    await page.evaluate(() => {
      const el = document.querySelector(
        '.security-alert-bar .system-alert-item.is-critical',
      )
      if (el) {
        el.classList.remove('is-critical')
        el.classList.add('is-high')
      }
    })
    await expect(
      page.locator('.security-alert-bar .system-alert-item.is-high'),
    ).toBeVisible()
    await page.screenshot({
      path: path.join(SHOT_DIR, '2-high-variant.png'),
      fullPage: true,
    })

    // 关闭：条整条消失（唯一卡片被摘）。
    await page.locator('.security-alert-bar .system-alert-close').first().click()
    await expect(page.locator('.security-alert-bar')).toHaveCount(0)
    await page.screenshot({
      path: path.join(SHOT_DIR, '3-after-dismiss.png'),
      fullPage: true,
    })

    // 清理：释放急停。
    await wsapi(page, 'estop', 'release')
  })
})
