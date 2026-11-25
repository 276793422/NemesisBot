/**
 * 浏览器接管 UI 视觉自测（2026-11-15 计划 D10/D15，UI 视觉自测纪律）。
 *
 * 走真实 gateway + 真实浏览器 spawn（config.browser.kind 钉 edge——auto 解析序
 * Chrome 优先，测试要确定性；专用 profile 落 e2e home），六景截图人审：
 *   1. 执行单元【浏览器】TAB 空态（列表子页签 + 打开按钮）
 *   2. spawn 后实例卡（Edge·1 + 状态点 + 端口 + profile）
 *   3. 展开卡标签页表 + 点名（工作 tab 圆点实心 + 行高亮）
 *   4. 设置子页签（detect 枚举卡片：可用/二期徽标 + 自定义路径卡）
 *   5. 日志子页签（spawn/attach 编排留痕时间线）
 *   6. 聊天侧 BrowserPickPanel（侧栏 🌐 按钮开面板 + 点名行）
 *   7. 死实例（外杀进程树）→ 死卡「↻ 重新打开」+ footer 常驻「打开浏览器」
 *      → 点重开 → 同 kind 新实例（2026-11-25 批 A+B；两张 07/08）
 *
 * Edge 缺席 = 诚实 SKIP 场景 2/3/6/7 实例部分（空态/设置照截）。
 * 收尾 browser.close 优雅关闭（Edge 进程树不残留）。
 */
import { test, expect, type Page } from './fixtures'
import fs from 'node:fs'
import path from 'node:path'
import { execSync } from 'node:child_process'
import { newAuthedPage, gotoRoute, wsapi } from './helpers'
const SHOT_DIR = path.join('test-results', 'browser-visual', 'shots')

function shot(page: Page, name: string): Promise<string> {
  const file = path.join(SHOT_DIR, name)
  return page.screenshot({ path: file, fullPage: false, animations: 'disabled' }).then(() => file)
}

test('浏览器接管六景截图：空态/实例卡/点名tab/设置/日志/点名面板', async ({ browser }) => {
  test.setTimeout(300_000)
  fs.mkdirSync(SHOT_DIR, { recursive: true })
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1440, height: 900 })
  // 本 spec 兼任资源窗首屏判宽修复（2026-11-25）的真浏览器回归锁：景 6 的
  // gotoRoute 整页加载 = 1440 视口首屏（可用宽 1200 落滞后带），修复前被
  // 误判宽屏走并排——资源窗并排（ChatPanel 944 + min 300）+ 会话侧栏 260
  // 超出可用宽，把无缩窄保护的点名面板列挤成 0 宽（景 6 toBeVisible 失败
  // 实录）；修复后首屏硬判窄 → 资源窗静默收起走 overlay，点名面板完整
  // 240px。若回归，景 6 面板不可见即红。
  await page.evaluate(() => localStorage.setItem('nb_resources_panel', '0'))
  // closeBrowser 的 window.confirm 原生守卫——Playwright 默认 dismiss（=取消）。
  page.on('dialog', (d) => d.accept())

  // —— 景 1：执行单元【浏览器】TAB 空态 ——
  await gotoRoute(page, '/#/executors?tab=browser')
  await expect(page.locator('[data-test="browser-list"]')).toBeVisible()
  await expect(page.locator('[data-test="browser-empty"]')).toBeVisible()
  await shot(page, '01-browser-list-empty.png')

  // —— 景 2：spawn 真 Edge（UI 按钮真实点击）——
  // 先把 browser.kind 钉成 edge（与设置页同款 config.set_field）：auto 解析序
  // 是「Chrome（默认浏览器偏好）> 平台保底」，本机装 Chrome 时 auto 会起
  // Chrome——测试要确定性（也符合验收口径「控制 Edge」），显式钉 kind。
  // ok 断言必须显式：wsapi 对 ok:false 不抛错，静默吞错会退化成 auto spawn。
  const cfgSet = await wsapi(page, 'config', 'set_field', { path: 'browser.kind', value: 'edge' })
  if (!cfgSet.ok) throw new Error(`钉 browser.kind=edge 失败: ${cfgSet.error}`)
  await page.locator('[data-test="browser-open"]').first().click()
  let hasSession = true
  try {
    await expect(page.locator('[data-test="browser-session-edge"]')).toBeVisible({ timeout: 60_000 })
  } catch {
    // Edge spawn 失败（SAC 拦截/无 Edge 等）= 诚实降级：只截设置/日志空态。
    hasSession = false
    await test.info().attach('edge-spawn-failed', {
      body: await page.locator('[data-test="browser-list"]').textContent() ?? 'spawn 超时',
      contentType: 'text/plain',
    })
  }

  if (hasSession) {
    // —— 景 3：展开卡 + 标签页表 + 点名 ——
    await expect(page.locator('[data-test="browser-tabs-edge"]')).toBeVisible()
    await expect(page.locator('[data-test="browser-tab-row"]').first()).toBeVisible({ timeout: 15_000 })
    await shot(page, '02-browser-session-tabs.png')
    await page.locator('[data-test="browser-tab-row"]').first().click()
    await expect(page.locator('[data-test="browser-tab-row"].tab-row-working').first()).toBeVisible({ timeout: 15_000 })
    await shot(page, '03-browser-working-attached.png')
  }

  // —— 景 4：设置子页签（detect 枚举卡片）——
  await page.locator('[data-test="browser-tab-settings"]').click()
  await expect(page.locator('[data-test="browser-settings"]')).toBeVisible()
  await expect(page.locator('[data-test="browser-kind-custom"]')).toBeVisible({ timeout: 15_000 })
  await shot(page, '04-browser-settings.png')

  // —— 景 5：日志子页签（编排留痕时间线）——
  await page.locator('[data-test="browser-tab-logs"]').click()
  await expect(page.locator('[data-test="browser-logs"]')).toBeVisible()
  await shot(page, '05-browser-logs.png')

  // —— 景 6：聊天侧 BrowserPickPanel（侧栏 🌐 按钮开面板）——
  // ChatView 挂在根路由 `/`（无 /chat 路径）；会话侧栏默认收起
  // （sessionStore.showSidebar=false），先点「显示会话列表」挂出侧栏。
  await gotoRoute(page, '/#/')
  await page.locator('button.toolbar-toggle[title="显示会话列表"]').click()
  await expect(page.locator('[data-test="browser-panel-toggle"]')).toBeVisible()
  await page.locator('[data-test="browser-panel-toggle"]').click()
  await expect(page.locator('[data-test="browser-pick-panel"]')).toBeVisible()
  if (hasSession) {
    await expect(page.locator('.browserpick-session').first()).toBeVisible({ timeout: 15_000 })
  }
  await shot(page, '06-browser-pick-panel.png')

  // —— 景 7：死实例「↻ 重新打开」+ footer 常驻「打开浏览器」（2026-11-25
  // 批 A+B 视觉自测）——taskkill 外杀 Edge 进程树（= 用户手关窗口）→ reaper
  // 置 alive=false → 点名面板死卡渲染「已退出」+ 重开钮（批 B）+ footer
  // 常驻启动钮（批 A——旧实现按钮只在空态，死会话保留注册表 = 永无入口）。
  // 点重开 → 同 kind 新实例起，死卡保留（注册表只认显式 close）。
  if (hasSession) {
    const sr = await wsapi(page, 'browser', 'sessions', {})
    const first = ((sr.data as { sessions?: { id: string; pid?: number; alive?: boolean }[] })?.sessions ?? [])[0]
    if (first?.pid) {
      // 外杀进程树；进程已先消失（异常路径）不硬测本景。
      try {
        execSync(`taskkill /PID ${first.pid} /T /F`, { stdio: 'ignore' })
      } catch {
        /* already gone */
      }
      await expect
        .poll(
          async () => {
            const r = await wsapi(page, 'browser', 'sessions', {})
            const list = (r.data as { sessions?: { alive?: boolean }[] })?.sessions ?? []
            return list.length > 0 && list.every((s) => s.alive === false)
          },
          { timeout: 30_000, intervals: [1_000, 2_000, 5_000] },
        )
        .toBe(true)
      // 面板 3s 轮询把死态带进 UI：死卡重开钮 + footer 常驻钮齐备。
      await expect(page.locator('[data-test="browser-pick-reopen"]')).toBeVisible({ timeout: 15_000 })
      await expect(page.locator('[data-test="browser-pick-open"]')).toBeVisible()
      await shot(page, '07-browser-pick-dead-reopen.png')
      // 批 B 主路径：点「↻ 重新打开」→ 同 kind 新实例（spawn 真浏览器 ~60s 级）。
      await page.locator('[data-test="browser-pick-reopen"]').click()
      await expect(page.locator('.browserpick-session:not(.dead)').first()).toBeVisible({ timeout: 90_000 })
      // 死卡保留（注册表语义：死实例不自动清册）+ 新实例并存。
      await expect(page.locator('[data-test="browser-pick-reopen"]')).toBeVisible()
      await shot(page, '08-browser-pick-reopened.png')
    }
  }

  // —— 收尾：优雅关闭全部会话（浏览器进程树不留残；spawn 失败路径也照跑——
  // 防上轮半开实例残留孤儿进程）——
  const r = await wsapi(page, 'browser', 'sessions', {})
  const sessions = ((r.data as { sessions?: { id: string }[] })?.sessions ?? [])
  for (const s of sessions) {
    await wsapi(page, 'browser', 'close', { session_id: s.id })
  }
})
