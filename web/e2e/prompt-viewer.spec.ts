/**
 * e2e/prompt-viewer.spec.ts — 提示词查看器独立页全链路。
 *
 * 回归背景 1（2026-11-13 凭据）：查看器的凭据源是 localStorage
 * （nemesisbot_auth_token），而注入链认证（URL fragment __dashboard_token= /
 * plugin-ui __DASHBOARD_TOKEN__ / 桌面 runtime 事件 → auth.autoLogin）曾只
 * 写内存——同源独立页读到空值，恒报「未检测到登录凭据」。既有 e2e 全部经
 * newAuthedPage 预置 localStorage，恰好绕开 autoLogin 链路，所以一直没红
 * （漏网根因；修复 = autoLogin 成功落盘，见 stores/auth.ts）。
 *
 * 回归背景 2（2026-11-13 一次性页面 UX）：链接打开 10 秒后不可访问、已打开
 * 页面持续可看——服务端票据裁决（chat.prompt_ticket 发票 → prompt_segment
 * 单次消费），纯前端倒计时防不了刷新重放。
 *
 * 本 spec 五段：
 * 1. 负面对照：零存储 context 直开查看器 → 凭据缺失错误卡；
 * 2. 修复链路：URL fragment token 进 Dashboard（autoLogin 真实路径）→
 *    断言 localStorage 已落盘 → 干净导航进聊天视图（回读分支）；
 * 3. 造真实会话（UI 发送）→ 差分取 sid；
 * 4. 票据流：chat.prompt_ticket 取票 → 带 &ticket= 打开查看器 → 内容渲染
 *    非空 + 倒计时横幅（10 秒后链接不可访问）→ 等归零转「已失效」；
 * 5. 一次性裁决：刷新 = 票据已消费 → 错误卡（链接已失效）；有凭据但缺票
 *    → 错误卡（缺少一次性访问票据）。
 */
import { expect, test } from './fixtures'
import { BASE_URL, E2E } from './env'
import { AUTH_KEY, gotoRoute, sendChat, waitAssistantReply, wsapi, type WsapiResult } from './helpers'

/** sessions.list 响应里的裸 sid 集合（data = { sessions: [{id,...}], ... }） */
function sessionIds(r: WsapiResult): string[] {
  const arr = (r.data as { sessions?: { id?: string }[] })?.sessions ?? []
  return arr.map((s) => s.id ?? '')
}

test('提示词查看器：autoLogin 落盘 → 票据流取段 → 一次性链接裁决', async ({ browser }) => {
  // ── 1. 负面对照：无凭据 → 错误卡（凭据闸先于票据闸）──
  const negCtx = await browser.newContext()
  const negPage = await negCtx.newPage()
  await negPage.goto(`${BASE_URL}/prompt-viewer/index.html?session=nonexistent&segment=tools`)
  await expect(negPage.locator('.pv-error-msg')).toContainText('未检测到登录凭据', { timeout: 20_000 })
  await negCtx.close()

  // ── 2. 修复链路：URL fragment token → autoLogin → localStorage 落盘 ──
  // App.vue 的 URL 入口契约是 hash 形态 `#__dashboard_token=`（读后
  // history.replaceState 剥离），不是 ?token= 查询参数。
  const context = await browser.newContext()
  const page = await context.newPage()
  await page.goto(`${BASE_URL}/#__dashboard_token=${encodeURIComponent(E2E.token)}`)
  // autoLogin 成功 = 遮罩消失（此 hash 会被 Vue Router 当未知路由主区空载，
  // 桌面真实入口走 init script 无脏 hash——断言凭据面即可）
  await expect(page.locator('.auth-overlay')).toBeHidden({ timeout: 20_000 })
  // 修复本体断言：autoLogin（非 localStorage 预置）成功后凭据已落盘
  const stored = await page.evaluate((k) => localStorage.getItem(k), AUTH_KEY)
  expect(stored, 'autoLogin 成功后 localStorage 必须有凭据（prompt-viewer 依赖）').toBe(E2E.token)

  // hash 入口主区空载——凭据已在 localStorage，干净加载落聊天视图（第 4 分支回读验证）
  await gotoRoute(page, '/')
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  // ── 3. 造真实会话（UI 发送 → mock 确定性回复落账）→ 差分取 sid ──
  const before = new Set(sessionIds(await wsapi(page, 'sessions', 'list')))
  await sendChat(page, '你好，请做个自我介绍')
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY')
  let sid = ''
  for (let i = 0; i < 20 && !sid; i++) {
    const added = sessionIds(await wsapi(page, 'sessions', 'list')).filter((id) => !before.has(id))
    if (added.length > 0) sid = added[0]
    else await page.waitForTimeout(500)
  }
  expect(sid, '发送后 sessions.list 应差分出新会话 id').not.toBe('')

  // ── 4. 票据流：取票 → 带票打开 → 内容渲染 + 倒计时横幅 ──
  const tk = await wsapi(page, 'chat', 'prompt_ticket', { session_id: sid, segment: 'tools' })
  expect(tk.data?.ticket, 'chat.prompt_ticket 应返回票据').toBeTruthy()
  const viewer = await context.newPage()
  await viewer.goto(
    `${BASE_URL}/prompt-viewer/index.html?session=${encodeURIComponent(sid)}&segment=tools&ticket=${encodeURIComponent(String(tk.data.ticket))}`,
  )
  await expect(viewer.locator('.pv-error-card')).toHaveCount(0)
  await expect(viewer.locator('.pv-body')).toBeVisible({ timeout: 20_000 })
  const parts = viewer.locator('.pv-part')
  await expect(parts.first()).toBeVisible()
  expect(await parts.count(), 'tools 段应渲染出多个 part').toBeGreaterThanOrEqual(1)
  const bodyText = (await viewer.locator('.pv-body').innerText()).trim()
  expect(bodyText.length, '段内容必须非空（能看到内容）').toBeGreaterThan(50)
  expect(bodyText).toContain('tok')
  // 滚动修复（2026-11-13）：theme.css 的 body{overflow:hidden} 是 dashboard
  // 自管滚动语义，查看器独立页被连带吞掉文档滚动 → 恢复 overflow:auto。
  // 断言修复本体（computed overflow）而非「此刻恰好溢出」——内容不足一屏
  // 时 scrollHeight==clientHeight 是正常形态。
  const overflow = await viewer.evaluate(
    () => getComputedStyle(document.body).overflow,
  )
  expect(overflow, 'body overflow 必须是 auto（theme 的 hidden 已被查看器覆盖）').toBe('auto')
  // 倒计时横幅：初始 10 秒口径 + 归零转已失效
  await expect(viewer.locator('.pv-timer-bar')).toContainText('秒后，链接不可访问')
  await expect(viewer.locator('.pv-timer-bar')).toContainText('页面不刷新、关闭则可持续查看')
  await expect(viewer.locator('.pv-timer-bar')).toContainText('链接已失效', { timeout: 15_000 })
  await viewer.screenshot({ path: '../target/prompt-viewer-e2e.png', fullPage: false })

  // ── 5. 一次性裁决：刷新（票据已消费）→ 错误卡；缺票 → 错误卡 ──
  await viewer.reload()
  await expect(viewer.locator('.pv-error-msg')).toContainText('链接已失效', { timeout: 20_000 })
  const noTicket = await context.newPage()
  await noTicket.goto(
    `${BASE_URL}/prompt-viewer/index.html?session=${encodeURIComponent(sid)}&segment=tools`,
  )
  await expect(noTicket.locator('.pv-error-msg')).toContainText('缺少一次性访问票据', { timeout: 20_000 })

  await context.close()
})
