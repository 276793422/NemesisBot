/**
 * e2e/scroll-follow.spec.ts — ChatPanel 滚动跟随「轮末收缩杀 follow」真实链路验收
 * （2026-11-26 修复的 app 级回归锁；机制级验证见 useStickToBottom.spec.ts 与
 * scroll-shrink-live-check.mjs）。
 *
 * 根因回顾：钉底时 scrollTop = scrollHeight - clientHeight 是恒等式；轮末
 * live 块折叠换装是净收缩，旧方向守卫把「几何性 scrollTop 下移」误判成用户
 * 上滚 → follow 永久解除 → 下一轮内容全部堆积视口下方（HOOK 场景必现）。
 *
 * 链路（真实 gateway AgentLoop + mock LLM 多轮）：
 *  1. UI 建会话 → 种子消息垫高对话到溢出视口（长对话场景；内容不满一屏时
 *     scrollHeight 恒 = clientHeight，收缩对 sh 不可见 = 测试空洞——实测
 *     两轮全在视口内的教训）→ sendChat(E2E_TOOL_MULTI)：mock 连续 12 轮
 *     sleep 工具调用再终答（live 时间线多轮真实展开、轮末真实折叠成单行
 *     = 大幅净收缩；单工具轮净收缩≈0，旧守卫吃不到 killing sample）
 *  2. 全程 250ms 采样滚动指标（dist / live 块在场 / 回底按钮 / scrollHeight）
 *  3. 断言：溢出前提成立；live 期 dist ≤ 10（贴底跟得住）；轮末净收缩
 *     ≥100px（场景有效性——无收缩则对旧守卫空洞）；终答后 dist ≤ 2 且回底
 *     按钮不出现（follow 未被折叠收缩误杀）；turn 2 全程同断言（折叠后
 *     下一轮照常贴底 = 用户症状面）；真实上滚仍出现回底按钮、点按钮回底
 *     （意图面零回退）
 *
 * ⚠️ static 是编译期嵌入：改 web/ 源码后必须 npm run build + 重编 nemesisbot，
 * 否则测的是上次构建进去的 UI（playwright.config.ts 头注释同款纪律）。
 */
import { expect, test } from './fixtures'
import { newAuthedPage, openSessionSidebar, sendChat, waitAssistantReply, wsChatSend } from './helpers'

interface ScrollSample {
  t: number
  dist: number
  live: boolean
  btn: boolean
  sh: number
  st: number
}

interface WrapEvent {
  t: number
  kind: 'mount' | 'unmount'
}

interface ScrollEvt {
  t: number
  st: number
  sh: number
  ch: number
}

/** 页内探针：100ms 采样滚动指标 + 回底按钮 wrap 挂载/卸载逐拍记录 + 原生
 * scroll 事件捕获日志（wrap 出现 = follow 被杀、消失 = follow 重臂——帧级
 * 区分「守卫开火后自愈」与「守卫根本没开火」；scroll 事件序列用于重构
 * 旧守卫 lastScrollTop 的真实裁决序列——折叠钳位到底发没发事件、值是多少）。 */
async function startSampler(page: import('@playwright/test').Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as any
    w.__scrollTrace = []
    if (!w.__wrapLog) {
      w.__wrapLog = []
      const parent = document.querySelector('.chat-messages-wrap')
      if (parent) {
        new MutationObserver(() => {
          const present = !!document.querySelector('.back-to-bottom-wrap')
          const last = (w.__wrapLog as WrapEvent[])[(w.__wrapLog as WrapEvent[]).length - 1]
          if (!last || (last.kind === 'mount') !== present) {
            w.__wrapLog.push({ t: performance.now() | 0, kind: present ? 'mount' : 'unmount' })
          }
        }).observe(parent, { childList: true, subtree: true })
      }
    }
    if (!w.__scrollEvtLog) {
      w.__scrollEvtLog = []
      const el = document.querySelector('.chat-messages')
      if (el) {
        el.addEventListener(
          'scroll',
          () => {
            ;(w.__scrollEvtLog as ScrollEvt[]).push({
              t: Date.now(),
              st: Math.round(el.scrollTop),
              sh: el.scrollHeight,
              ch: el.clientHeight,
            })
          },
          { passive: true, capture: true },
        )
      }
    }
    w.__sampler = setInterval(() => {
      const el = document.querySelector('.chat-messages')
      const live = !!document.querySelector('.message.assistant.process-live')
      const btn = !!document.querySelector('.back-to-bottom-wrap')
      const dist = el ? el.scrollHeight - el.scrollTop - el.clientHeight : -1
      w.__scrollTrace.push({
        t: Date.now(),
        dist,
        live,
        btn,
        sh: el?.scrollHeight ?? 0,
        st: Math.round(el?.scrollTop ?? 0),
      })
    }, 100)
  })
}

async function stopSampler(page: import('@playwright/test').Page): Promise<{
  trace: ScrollSample[]
  wrapLog: WrapEvent[]
  evtLog: ScrollEvt[]
}> {
  return page.evaluate(() => {
    const w = window as any
    clearInterval(w.__sampler)
    return {
      trace: w.__scrollTrace as ScrollSample[],
      wrapLog: w.__wrapLog as WrapEvent[],
      evtLog: w.__scrollEvtLog as ScrollEvt[],
    } as any
  })
}

/** 压缩 trace 打印：只保留 sh/st/dist 相邻变化点（全量太长淹没关键帧）。 */
function compressTrace(trace: ScrollSample[]): string {
  const out: string[] = []
  let prev: ScrollSample | null = null
  for (const s of trace) {
    if (
      !prev ||
      s.sh !== prev.sh ||
      s.st !== prev.st ||
      s.dist !== prev.dist ||
      s.live !== prev.live ||
      s.btn !== prev.btn
    ) {
      out.push(`${s.live ? 'L' : s.btn ? 'B' : '.'} sh=${s.sh} st=${s.st} dist=${s.dist}`)
    }
    prev = s
  }
  return out.join(' | ')
}

/** 压缩 scroll 事件序列：只保留 st 或 sh 相邻变化点（重构守卫裁决用——
 * 每个事件按 HEAD 逻辑 st 减小即杀，减小的那些事件用 ↓ 标出）。 */
function compressEvtLog(log: ScrollEvt[]): string {
  const out: string[] = []
  let prev: ScrollEvt | null = null
  for (const e of log) {
    if (!prev || e.st !== prev.st || e.sh !== prev.sh) {
      const mark = prev && e.st < prev.st - 1 ? '↓' : '='
      out.push(`${mark}${e.st}/${e.sh}`)
    }
    prev = e
  }
  return out.join(' ')
}

async function scrollMetrics(page: import('@playwright/test').Page) {
  return page.evaluate(() => {
    const el = document.querySelector('.chat-messages') as HTMLElement | null
    if (!el) return null
    return {
      dist: el.scrollHeight - el.scrollTop - el.clientHeight,
      overflow: el.scrollHeight - el.clientHeight > 50,
      btnVisible: !!document.querySelector('.back-to-bottom-wrap'),
    }
  })
}

test('轮末折叠不杀滚动跟随：live 贴底 → 折叠后 follow 存活 → 下一轮照常贴底；真实上滚仍放手', async ({ browser }) => {
  test.setTimeout(300_000)
  // 矮视口强制对话溢出滚动容器（默认 720 高下两轮消息全在视口内，滚动
  // 事件零发生 = 测试空洞；用户真实场景是对话超出一屏）。
  const page = await newAuthedPage(browser, { viewport: { width: 1280, height: 420 } })
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  // ---- UI 建会话（种子纪律：必须经页面 UI 创建）----
  await openSessionSidebar(page)
  const prevActive = await page
    .locator('.session-item.active')
    .getAttribute('data-session-id')
    .catch(() => null)
  await page.locator('.fn-new-chat').click()
  await expect(async () => {
    const row = page.locator('.session-item.active')
    await expect(row.first()).toBeVisible({ timeout: 2_000 })
    const id = await row.first().getAttribute('data-session-id', { timeout: 1_000 })
    expect(id, `active id 未变：${prevActive}`).not.toBe(prevActive)
  }).toPass({ timeout: 15_000 })

  // 初始态：贴底（dist ≈ 0）、无回底按钮
  {
    const m = await scrollMetrics(page)
    expect(m, '.chat-messages 容器应在场').toBeTruthy()
    expect(m!.dist, '新会话初始应贴底').toBeLessThanOrEqual(2)
    expect(m!.btnVisible, '初始不应出现回底按钮').toBe(false)
  }

  // ---- 种子预垫：把对话垫高到溢出（用户真实场景 = 长对话中的 HOOK 轮）。
  // 内容不满一屏时 scrollHeight 恒 = clientHeight，轮末收缩对 sh 完全不可
  // 见、守卫吃不到任何采样 = 测试空洞（矮视口直跑两轮全在视口内的实测
  // 教训）。4 条种子（mock 秒回直答）≈ 800px，视口 420 下必然溢出。----
  const sessionId = await page.locator('.session-item.active').getAttribute('data-session-id')
  expect(sessionId, '新会话应有 data-session-id').toBeTruthy()
  for (let i = 1; i <= 4; i++) {
    await wsChatSend(page, sessionId!, `种子垫高消息 ${i}（滚动跟随场景前置）`)
  }
  // 溢出前置断言：内容确实超出视口（否则后续 sh 断言全部失真）
  await expect(async () => {
    const m = await scrollMetrics(page)
    expect(m!.dist, '种子垫高后应贴底（增长不脱队）').toBeLessThanOrEqual(2)
    expect(m!.overflow, '种子垫高后对话应溢出视口（滚动场景成立的前提）').toBe(true)
  }).toPass({ timeout: 10_000 })

  // ---- turn 1：多工具轮臂（E2E_TOOL_MULTI：连续 sleep 工具轮 → live
  // 时间线多轮展开 → 轮末折叠大幅净收缩，忠实复现 HOOK 场景收缩幅度。
  // 单工具轮 live 与折叠形态高度相当净收缩≈0，旧守卫吃不到 killing
  // sample，测不到目标场景——已用旧前端实测证实）----
  await startSampler(page)
  await sendChat(page, `E2E_TOOL_MULTI 滚动跟随验收 turn1 ${Date.now()}`)
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY', 60_000)
  // 终答气泡出现与折叠同 tick，但采样器 250ms 一拍——多等两拍确保 trace
  // 收到折叠后的 sh（否则净收缩断言读到折叠前样本 = 假 0）。
  await page.waitForTimeout(600)
  const { trace: trace1, wrapLog: wrapLog1, evtLog: evtLog1 } = await stopSampler(page)
  console.log('[trace1]', compressTrace(trace1))
  console.log('[wrapLog1]', JSON.stringify(wrapLog1))
  console.log('[evtLog1]', compressEvtLog(evtLog1))

  // 测试有效性：live 时间线必须真实出现过（否则本 spec 没测到目标场景）
  expect(
    trace1.some((s) => s.live),
    'turn 1 应出现 live 过程时间线（.process-live）——mock 未走工具臂则本测试空洞',
  ).toBe(true)
  // 测试有效性：轮末折叠必须是净收缩（HOOK 场景的 BUG 前提）——live 峰值
  // scrollHeight 比终态高出 ≥100px。无收缩 = 守卫吃不到 killing sample，
  // 对旧版是空洞测试（旧前端单 sleep 轮实测净收缩≈0 的教训）。
  const maxLiveSh1 = Math.max(...trace1.filter((s) => s.live).map((s) => s.sh))
  const endSh1 = trace1[trace1.length - 1].sh
  expect(
    maxLiveSh1 - endSh1,
    `turn 1 轮末净收缩 ${maxLiveSh1 - endSh1}px（<100 = 折叠收缩场景失真，测试空洞）`,
  ).toBeGreaterThanOrEqual(100)

  // 核心断言 1：live 期贴底跟得住（增长贴底、收缩重钉均不脱队）
  const liveMaxDist = Math.max(...trace1.filter((s) => s.live).map((s) => s.dist))
  expect(liveMaxDist, `live 期最大距底 ${liveMaxDist}px（follow 脱队即堆积）`).toBeLessThanOrEqual(10)

  // 核心断言 2：终答折叠后 follow 存活——回底按钮不出现（旧守卫在此被
  // 几何收缩误杀 → 按钮立即出现）、内容贴底可见
  const m1 = await scrollMetrics(page)
  expect(m1!.btnVisible, 'turn 1 终答后不应出现回底按钮（折叠收缩误杀 follow 的直接症状）').toBe(false)
  expect(m1!.dist, `turn 1 终答后距底 ${m1!.dist}px（应贴底可见）`).toBeLessThanOrEqual(2)
  const btnDuring1 = trace1.some((s) => s.btn)
  expect(btnDuring1, 'turn 1 期间回底按钮不应出现').toBe(false)

  // ---- turn 2：折叠之后再来一轮（用户报的症状面：上一轮折叠杀 follow →
  // 本轮内容全部堆积视口下方看不到）----
  await startSampler(page)
  await sendChat(page, `E2E_TOOL_MULTI 滚动跟随验收 turn2 ${Date.now()}`)
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY', 60_000)
  await page.waitForTimeout(600)
  const { trace: trace2, wrapLog: wrapLog2, evtLog: evtLog2 } = await stopSampler(page)
  console.log('[trace2]', compressTrace(trace2))
  console.log('[wrapLog2]', JSON.stringify(wrapLog2))
  console.log('[evtLog2]', compressEvtLog(evtLog2))

  expect(
    trace2.some((s) => s.live),
    'turn 2 应出现 live 过程时间线',
  ).toBe(true)
  const maxLiveSh2 = Math.max(...trace2.filter((s) => s.live).map((s) => s.sh))
  const endSh2 = trace2[trace2.length - 1].sh
  expect(
    maxLiveSh2 - endSh2,
    `turn 2 轮末净收缩 ${maxLiveSh2 - endSh2}px（<100 = 折叠收缩场景失真）`,
  ).toBeGreaterThanOrEqual(100)
  const liveMaxDist2 = Math.max(...trace2.filter((s) => s.live).map((s) => s.dist))
  expect(liveMaxDist2, `turn 2 live 期最大距底 ${liveMaxDist2}px`).toBeLessThanOrEqual(10)
  const m2 = await scrollMetrics(page)
  expect(m2!.btnVisible, 'turn 2 终答后不应出现回底按钮').toBe(false)
  expect(m2!.dist, `turn 2 终答后距底 ${m2!.dist}px（应贴底可见）`).toBeLessThanOrEqual(2)
  expect(trace2.some((s) => s.btn), 'turn 2 期间回底按钮不应出现').toBe(false)

  // ---- 意图负对照：真实滚轮上滚必须放手（回底按钮出现），点按钮回底 ----
  await page.locator('.chat-messages').hover()
  await page.mouse.wheel(0, -240)
  // 等滚动手势落地（上一轮实测：wheel 后立即 dump 时滚动尚未生效——st 仍
  // 贴底、wrap 未挂载，dump 抓的是挂载前空态，白拍）。
  await page.waitForTimeout(800)
  // 诊断 dump（wrap「在场但 Playwright 判 hidden」之谜——此时刻 wrap 已挂
  // 载，dump 即 hidden 态的真实几何/样式现场，回归时同样有现场）。
  const wrapDump = await page.evaluate(() => {
    const w = document.querySelector('.back-to-bottom-wrap') as HTMLElement | null
    const b = document.querySelector('.back-to-bottom') as HTMLElement | null
    const cont = document.querySelector('.chat-messages-wrap') as HTMLElement | null
    const el = document.querySelector('.chat-messages') as HTMLElement | null
    const cs = (e: HTMLElement | null) => (e ? getComputedStyle(e) : null)
    return {
      wrapCount: document.querySelectorAll('.back-to-bottom-wrap').length,
      wrapRect: w ? w.getBoundingClientRect().toJSON() : null,
      wrapClientRects: w ? w.getClientRects().length : 0,
      wrapOffsetParentNull: w ? w.offsetParent === null : null,
      wrapOffset: w ? { w: w.offsetWidth, h: w.offsetHeight } : null,
      wrapDisplay: cs(w)?.display,
      wrapVisibility: cs(w)?.visibility,
      btnRect: b ? b.getBoundingClientRect().toJSON() : null,
      btnDisplay: cs(b)?.display,
      btnVisibility: cs(b)?.visibility,
      contRect: cont ? cont.getBoundingClientRect().toJSON() : null,
      contDisplay: cs(cont)?.display,
      scroll: el ? { st: el.scrollTop, sh: el.scrollHeight, ch: el.clientHeight } : null,
    }
  })
  console.log('[wrap-dump]', JSON.stringify(wrapDump))
  await expect(page.locator('.back-to-bottom-wrap')).toBeVisible({ timeout: 5_000 })
  const m3 = await scrollMetrics(page)
  expect(m3!.dist, '上滚后应离开底部').toBeGreaterThan(50)
  await page.locator('button.back-to-bottom').click()
  await expect(page.locator('.back-to-bottom-wrap')).toBeHidden({ timeout: 5_000 })
  const m4 = await scrollMetrics(page)
  expect(m4!.dist, '点回底按钮后应贴底').toBeLessThanOrEqual(2)
})
