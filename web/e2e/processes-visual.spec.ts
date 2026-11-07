/**
 * 后台进程实时输出视觉自测（2026-11-06 UI 批，UI 视觉自测纪律）。
 *
 * 走真实 gateway + mock LLM 全链路（E2E_TOOL_BG → background_start 工具
 * 起 BGSTREAM 流式输出器（每秒两行纯 ASCII，持续 ~10min），steerable=true），
 * 五态截图
 * 人审（双面方案：聊天内嵌迷你 tail + /processes 独立页，深链互通）：
 *   0. /processes 空态（e2e home 全新重建，注册表空）
 *   2. 聊天内嵌迷你 tail（WorkTimeline 展开态：● 运行点 + #id + 实时
 *      输出行 + 完整输出深链入口）
 *   3. 深链详情模态（/executors?tab=processes&focus=<id> 自动开）+ steer 注入面
 *      （steerable 运行中 → stdin 注入框 + 注入成功 toast）
 *   1. 列表页运行中形态（● 运行中徽标行）
 *   4. 停止终态（列表 ⛔ 已停止徽标 + 详情 tail 定格 ⛔ 已停止）
 *   5. 历史诚实过期（gateway 重启 → registry 内存态清空 → 历史工作块
 *      tail 轮询 no such job_id → 「输出已过期」注记）
 *
 * 审批面：ping 纯净命令——e2e 安全种子只有 *E2E_APPROVAL_MARKER* ask 规则，
 * onboard 模板 exec_unknown_policy=allow → 8 层管线直通，无审批卡（与
 * subagent spec 的 spawn 审批流刻意不同——那条覆盖卡流，这条验证直通）。
 *
 * 依赖 verify-mock-llm.mjs 的 bg_tool 路由。
 */
import { test, expect, type Page } from './fixtures'
import fs from 'node:fs'
import path from 'node:path'
import { newAuthedPage, restartGateway, sendChat, waitAssistantReply, wsapi } from './helpers'
import { BASE_URL } from './env'
const SHOT_DIR = path.join('test-results', 'processes-visual', 'shots')

function shot(page: Page, name: string): Promise<string> {
  const file = path.join(SHOT_DIR, name)
  // animations:'disabled'：headless 下 CSS 动画时钟只在帧产生时前进，插入后
  // 立即截图会把入场动画冻在第 0 帧（toast 实录：opacity 0 + translateX(100%)
  // 出画，visible 断言照过——快进到终态才是确定性证据；headed 真浏览器无此象）。
  return page.screenshot({ path: file, fullPage: false, animations: 'disabled' }).then(() => file)
}

interface BgJob {
  id: number
  running: boolean
  killed?: boolean
  exit_code?: number
}

async function bgJobs(page: Page): Promise<BgJob[]> {
  const r = await wsapi(page, 'background', 'list', {})
  if (!r.ok) throw new Error(`background.list 失败：${r.error}`)
  return ((r.data as { jobs?: BgJob[] })?.jobs ?? [])
}

test('后台进程五态截图：空态/聊天tail/深链详情steer/列表运行中/停止终态', async ({ browser }) => {
  test.setTimeout(300_000)
  fs.mkdirSync(SHOT_DIR, { recursive: true })
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1440, height: 900 })
  // killJob / stopJob 的 window.confirm 原生守卫——Playwright 默认 dismiss
  // （=取消），必须显式接受，否则 background.kill 永远发不出。
  page.on('dialog', (d) => d.accept())

  // —— 状态 0：空态（侧栏真实点击 → /executors，切「后台任务」TAB）——
  // 2026-11-07 执行单元复合页重构：侧栏「后台任务」条目撤并进「执行单元」
  // （默认落子代理 TAB），切 TAB 覆盖 tab→URL 同步交互。
  // 点击 + toHaveURL 包 toPass 重试环：nav-item 点击偶发被吞（hit-test 与
  // 事件派发之间 Vue 重渲染替换节点，2026-11-07 WK 全量实录：URL 24 次采样
  // 稳定不变、无导航）——内层 5s 短窗给外层重试机会重发点击，外层 20s 总闸。
  await expect(async () => {
    await page.locator('.nav-item', { hasText: '执行单元' }).click()
    await expect(page).toHaveURL(/\/executors/, { timeout: 5_000 })
  }).toPass({ timeout: 20_000 })
  // toHaveURL（轮询）而非 waitForURL（导航事件）：webkit 下 same-document
  // hash 导航的 navigation 事件间歇不可见（2026-11-07 全量实录：点击成功、
  // URL 已变、waitForURL 仍 15s 超时）；SPA 断言 URL 用轮询语义才确定。
  await expect(page).toHaveURL(/\/executors/, { timeout: 15_000 })
  await page.locator('.exec-tab', { hasText: '后台任务' }).click()
  await expect(page).toHaveURL(/tab=processes/, { timeout: 15_000 })
  await expect(page.locator('.page-processes .empty-state')).toBeVisible({ timeout: 15_000 })
  await shot(page, 'state0-empty.png')

  // —— 起进程：回聊天发 E2E_TOOL_BG（mock → background_start BGSTREAM 流式输出器）——
  await page.goto(BASE_URL)
  await sendChat(page, 'E2E_TOOL_BG 帮我起一个持续输出 60 秒的后台进程')
  // 工具执行有装配窗口（agent loop 轮次）→ 轮询注册表等 running 行。
  await expect
    .poll(async () => (await bgJobs(page)).filter((j) => j.running).length, {
      timeout: 60_000,
      intervals: [1_000],
    })
    .toBeGreaterThanOrEqual(1)
  const jobId = (await bgJobs(page)).find((j) => j.running)!.id

  // —— 状态 2：聊天内嵌迷你 tail（落地块默认折叠 → 展开）——
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY', 30_000)
  const header = page.locator('.wt-header').last()
  await expect(header).toBeVisible()
  await header.click()
  const bgRow = page.locator('.wt-tool-bg').last()
  await expect(bgRow).toBeVisible({ timeout: 15_000 })
  await expect(bgRow).toContainText('启动后台进程')
  const compactTail = page.locator('.pot.compact').last()
  await expect(compactTail).toBeVisible({ timeout: 15_000 })
  await expect(compactTail.locator('.pot-id')).toContainText(`#${jobId}`)
  // tail live：流式输出器首行到达（纯 ASCII，无 GBK 乱码。2026-11-06 起
  // mock 载荷弃用 ping：本机回环 ICMP 回应被拦，ping 每请求 4s 超时才吐
  // 一行「请求超时」，不是实时输出载荷——实测 100% 丢包）。
  await expect(compactTail.locator('.pot-tail pre')).toContainText('BGSTREAM', {
    timeout: 20_000,
  })
  await expect(compactTail.locator('.pot-dot').first()).toHaveClass(/is-run/)
  await shot(page, 'state2-chat-embedded-tail.png')

  // —— 状态 3：完整输出深链 → /executors?tab=processes&focus=<id> 详情模态 + steer ——
  // pushState 捕获：消费端 openFromQuery 剥掉 focus 参（URL 清理），落地
  // URL 不含 focus——点击前包一层记录，确定性捕获路由带过的深链 URL。
  await page.evaluate(() => {
    const w = window as unknown as { __pushUrls?: string[] }
    w.__pushUrls = []
    const orig = history.pushState.bind(history)
    history.pushState = function (...args: Parameters<History['pushState']>) {
      w.__pushUrls!.push(String(args[2] ?? ''))
      return orig(...args)
    }
  })
  await compactTail.locator('.pot-link').click()
  // toHaveURL（轮询）而非 waitForURL（导航事件）：webkit 下 same-document
  // hash 导航的 navigation 事件间歇不可见（2026-11-07 全量实录：点击成功、
  // URL 已变、waitForURL 仍 15s 超时）；SPA 断言 URL 用轮询语义才确定。
  await expect(page).toHaveURL(/\/executors/, { timeout: 15_000 })
  // 模态自动打开 = focus 送达并消费的证据（openDetail 只由 route.query.focus 触发）。
  await expect(page.locator('.modal-wide')).toBeVisible({ timeout: 15_000 })
  await expect(page.locator('.modal-header h3')).toContainText(`#${jobId}`)
  const fullTail = page.locator('.modal-wide .pot')
  await expect(fullTail.locator('.pot-tail pre')).toContainText('BGSTREAM', {
    timeout: 20_000,
  })
  // steer 注入面（steerable=true + 运行中 → 注入框渲染）。
  const steerInput = page.locator('.modal-wide .pot-steer input.form-input')
  await expect(steerInput).toBeVisible()
  await steerInput.fill('E2E_STEER_LINE')
  await page.locator('.modal-wide .pot-steer button.btn').click()
  // toast 断言先于截图（成功反馈要在证据里可见；toast TTL 4s，可见断言
  // 一过立刻拍，首轮实录：text 断言与截图之间疑似 >4s 间隔致 toast 出画）。
  // 按文本过滤而非裸 .toast：任何 stray toast（如排队提示）都会触发 strict
  // mode violation 掩蔽真断言（2026-11-07 全量实录：幽灵排队 toast + steer
  // 成功 toast 并存致 2 元素 violation；产品侧根修见 ChatPanel wasStreaming）。
  const toastBox = page.locator('.toast', { hasText: '已注入' })
  await expect(toastBox).toBeVisible({ timeout: 10_000 })
  await shot(page, 'state3-detail-steer.png')
  await expect(toastBox).toContainText('已注入 stdin', { timeout: 10_000 })
  // 路由层证据：pushState 确实带过 /executors?tab=processes&focus=<id> 深链。
  const pushUrls = await page.evaluate(
    () => (window as unknown as { __pushUrls?: string[] }).__pushUrls ?? [],
  )
  expect(
    pushUrls.some((u) => new RegExp(`/executors\\?tab=processes&focus=${jobId}`).test(u)),
  ).toBeTruthy()

  // —— 状态 1：列表页运行中形态 ——
  await page.locator('.modal-close').click()
  await expect(page.locator('.modal-wide')).toBeHidden({ timeout: 5_000 })
  const rows = page.locator('.proc-row')
  await expect(rows).toHaveCount(1, { timeout: 15_000 })
  await expect(rows.first()).toContainText(String(jobId))
  await expect(rows.first().locator('span.badge')).toContainText('● 运行中')
  await shot(page, 'state1-processes-running.png')

  // —— 状态 4：停止终态（列表行停止 → kill → 徽标翻转 + 详情 tail 定格）——
  await rows.first().locator('button.btn-danger').click()
  await expect
    .poll(
      async () => {
        const j = (await bgJobs(page)).find((x) => x.id === jobId)
        return j ? (j.running ? 'running' : j.killed ? 'killed' : 'exited') : 'gone'
      },
      { timeout: 30_000, intervals: [1_000] },
    )
    .toBe('killed')
  await expect(rows.first().locator('span.badge')).toContainText('⛔ 已停止', {
    timeout: 15_000,
  })
  await shot(page, 'state4-processes-killed.png')
  // 详情 tail 定格：重开模态 → pot-exit ⛔ 已停止（组件自轮追平终态）。
  await rows.first().locator('button', { hasText: '详情' }).click()
  await expect(page.locator('.modal-wide')).toBeVisible({ timeout: 15_000 })
  await expect(page.locator('.modal-wide .pot-exit')).toContainText('⛔ 已停止', {
    timeout: 20_000,
  })
  await shot(page, 'state4-detail-frozen-tail.png')

  // —— 状态 5：历史诚实过期（gateway 重启 → registry 内存态清空，会话
  // 历史落盘存活 → 历史工作块 tail 轮询 no such job_id → expired 相位）。——
  // goto 而非 reload：状态 1-4 停在 /processes，reload 保 URL 重载后仍是
  // ProcessesView（聊天面板未挂载，wt-header 永不出现——首轮实录）；
  // goto BASE_URL = 换回聊天路由 + 全量导航，历史由 onMounted loadHistory 重拉。
  await restartGateway()
  await page.goto(BASE_URL)
  const header2 = page.locator('.wt-header').last()
  await expect(header2).toBeVisible({ timeout: 30_000 })
  await header2.click()
  const staleTail = page.locator('.pot.compact').last()
  await expect(staleTail).toBeVisible({ timeout: 15_000 })
  await expect(staleTail).toContainText('输出已过期', { timeout: 20_000 })
  await shot(page, 'state5-history-expired.png')
})
