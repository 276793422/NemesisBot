/**
 * 后台进程 UI 边角实机测试（2026-11-06 UI 批加严，UI 视觉自测纪律）。
 *
 * 覆盖 processes-visual.spec.ts 之外的交互与形态边角：
 *   1. 全程 console error / pageerror 监控（截图好看 ≠ 无 JS 报错）
 *   2. 多任务并发（两条 E2E_TOOL_BG）+ 列表 id 倒序排序
 *   3. 大输出任务（E2E_TOOL_BG_BIG ~680KB > 256KB 环形缓冲）→
 *      「已丢弃开头」徽标 + tail 起点语义（head 被淘汰仍可看末窗）
 *   4. 上滚暂停 / ↓跟随最新（终态大输出上滚恢复 + 运行中暂停期间
 *      内容继续增长但视图不动——live 语义核心）
 *   5. 模态关闭重开 + 深链消费
 *   6. 暗色主题形态（data-theme=dark 全程）+ UI 停止按钮（暗色下）
 *   7. 无效深链 ?focus=999999 诚实降级（浅色主题交叉检查）
 *
 * 依赖 verify-mock-llm.mjs 的 bg_tool / bg_tool_big 路由。
 */
import { test, expect, type Page, type Locator } from './fixtures'
import fs from 'node:fs'
import path from 'node:path'
import { gotoRoute, newAuthedPage, restartGateway, sendChat, waitAssistantReply, wsapi } from './helpers'
import { BASE_URL } from './env'
const SHOT_DIR = path.join('test-results', 'processes-visual', 'shots')

function shot(page: Page, name: string): Promise<string> {
  const file = path.join(SHOT_DIR, name)
  // 同 visual spec：headless 动画时钟只在帧产生时前进，快进到终态再拍。
  return page.screenshot({ path: file, fullPage: false, animations: 'disabled' }).then(() => file)
}

interface BgJob {
  id: number
  running: boolean
  killed?: boolean
  exit_code?: number
  command?: string
}

async function bgJobs(page: Page): Promise<BgJob[]> {
  const r = await wsapi(page, 'background', 'list', {})
  if (!r.ok) throw new Error(`background.list 失败：${r.error}`)
  return ((r.data as { jobs?: BgJob[] })?.jobs ?? [])
}

/** console error / 未捕获异常收集器（全程挂页，断言收尾）。 */
function watchErrors(page: Page) {
  const consoleErrors: string[] = []
  const pageErrors: string[] = []
  page.on('console', (m) => {
    if (m.type() === 'error') consoleErrors.push(m.text())
  })
  page.on('pageerror', (e) => pageErrors.push(String(e)))
  return { consoleErrors, pageErrors }
}

function errReport(consoleErrors: string[], pageErrors: string[]): string {
  return `console errors=${JSON.stringify(consoleErrors, null, 2)}; pageerrors=${JSON.stringify(pageErrors, null, 2)}`
}

/** 行序按解析 id 断言降序（bigId 在最前）。 */
async function rowIdOrder(page: Page, expectTopFirst: number[]): Promise<void> {
  const rows = page.locator('.proc-row')
  const texts = await rows.allTextContents()
  const ids = texts
    .map((t) => t.match(/#(\d+)/)?.[1])
    .filter((v): v is string => !!v)
    .map(Number)
  const pos = expectTopFirst.map((id) => ids.indexOf(id))
  expect(
    pos.every((p) => p >= 0),
    `rows=${JSON.stringify(ids)} expectTopFirst=${expectTopFirst}`,
  ).toBeTruthy()
  expect(
    pos[0] < pos[1] && pos[1] < pos[2],
    `id 倒序违例：rows=${JSON.stringify(ids)} bigId=${expectTopFirst[0]} idB=${expectTopFirst[1]} idA=${expectTopFirst[2]}`,
  ).toBeTruthy()
}

/** 上滚暂停（对当前打开的模态内 .pot-tail）。带重试环：scrollTop=0 的
 * scroll 事件异步落地，若恰好撞上轮询 tick 的 scrollToBottom（置回底部）
 * 会错过暂停窗口——设到可见失败为止（live 语义核心，值得稳）。钮可见 +
 * is-paused 之外还要断言 scrollTop 真到顶：组件曾有一版贴底竞态（同步贴
 * 底读旧 scrollHeight）会在无用户滚动时自暂停并把视图冻在中部、钮照常
 * 亮着——scrollTop 稳态断言防回归（2026-11-07）。 */
async function pauseFollow(page: Page, modal: Locator): Promise<void> {
  const tail = modal.locator('.pot-tail')
  const followBtn = modal.locator('.pot-follow')
  await expect(async () => {
    await tail.evaluate((el) => {
      el.scrollTop = 0
    })
    await expect(followBtn).toBeVisible({ timeout: 1_000 })
  }).toPass({ timeout: 20_000 })
  await expect(tail).toHaveClass(/is-paused/)
  await expect
    .poll(async () => tail.evaluate((el) => el.scrollTop), { timeout: 5_000, intervals: [250] })
    .toBeLessThan(40)
}

test('后台进程边角：多任务排序/丢弃徽标/上滚暂停/暗色主题/console 零报错', async ({ browser }) => {
  test.setTimeout(300_000)
  fs.mkdirSync(SHOT_DIR, { recursive: true })
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1440, height: 900 })
  const errs = watchErrors(page)
  page.on('dialog', (d) => d.accept())
  // 暗色主题从加载即生效（useTheme 读 localStorage）。try/catch 防 opaque
  // origin（about:blank 中转）SecurityError——本测试现走 page.goto 直达不经
  // gotoRoute，但同类脚本已在 wk7 实录过污染零报错断言（见下方浅色交叉）。
  await page.addInitScript(() => {
    try {
      localStorage.setItem('nemesisbot_theme', 'dark')
    } catch {
      /* opaque origin：跳过预置 */
    }
  })

  // —— 起两个并发流式输出进程（先 A 后 B，id 单调递增；BGSTREAM 载荷每秒
  // 两行纯 ASCII——ping 载荷已弃用：本机回环 ICMP 回应被拦，ping 每请求 4s
  // 超时才吐一行「请求超时」，不是实时输出，实测 100% 丢包）——
  await page.goto(BASE_URL)
  await sendChat(page, 'E2E_TOOL_BG 帮我起后台进程 A')
  await expect
    .poll(async () => (await bgJobs(page)).filter((j) => j.running).length, {
      timeout: 60_000,
      intervals: [1_000],
    })
    .toBeGreaterThanOrEqual(1)
  const idA = (await bgJobs(page)).find((j) => j.running)!.id

  await sendChat(page, 'E2E_TOOL_BG 帮我起后台进程 B')
  await expect
    .poll(async () => (await bgJobs(page)).filter((j) => j.running).length, {
      timeout: 60_000,
      intervals: [1_000],
    })
    .toBeGreaterThanOrEqual(2)
  const idB = Math.max(...(await bgJobs(page)).filter((j) => j.running).map((j) => j.id))
  expect(idB, 'B 后起 id 更大').toBeGreaterThan(idA)

  // —— 大输出任务（秒级自终；steerable=false）——
  await sendChat(page, 'E2E_TOOL_BG_BIG 帮我起一个大量输出的后台进程')
  const bigCmdNeedle = '0123456789ABCDEF'
  await expect
    .poll(
      async () =>
        (await bgJobs(page)).some((j) => (j.command ?? '').includes(bigCmdNeedle) && !j.running),
      { timeout: 60_000, intervals: [1_000] },
    )
    .toBe(true)
  const bigId = (await bgJobs(page)).find((j) => (j.command ?? '').includes(bigCmdNeedle))!.id
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY', 30_000)

  // —— 列表：三个已知 id 倒序（bigId 最前）——
  // router 是 hash 模式：进执行单元页走侧栏点击（与 visual spec 同款，最保
  // 真），落地默认子代理 TAB → 切「后台任务」TAB（2026-11-07 复合页重构）。
  await page.goto(BASE_URL)
  // 点击 + toHaveURL 包 toPass 重试环（processes-visual 同款）：nav-item
  // 点击偶发被吞（Vue 重渲染竞速），内层 5s 短窗给外层重试机会重发点击。
  await expect(async () => {
    await page.locator('.nav-item', { hasText: '执行单元' }).click()
    await expect(page).toHaveURL(/\/executors/, { timeout: 5_000 })
  }).toPass({ timeout: 20_000 })
  // toHaveURL（轮询）而非 waitForURL：webkit same-document hash 导航事件
  // 间歇不可见（见 processes-visual 同款注记）。
  await expect(page).toHaveURL(/\/executors/, { timeout: 15_000 })
  await page.locator('.exec-tab', { hasText: '后台任务' }).click()
  await expect(page).toHaveURL(/tab=processes/, { timeout: 15_000 })
  const rows = page.locator('.proc-row')
  await expect(rows.filter({ hasText: `#${bigId}` })).toBeVisible({ timeout: 15_000 })
  await rowIdOrder(page, [bigId, idB, idA])
  // 列表行内丢弃指示（详情徽标之外的第二呈现面）。
  await expect(rows.filter({ hasText: `#${bigId}` })).toContainText('已丢')
  await shot(page, 'edge-dark-list-order.png')

  // —— 大输出详情：丢弃徽标 + tail 仍可看末窗（起点 = max(dropped, total-window)）——
  await rows.filter({ hasText: `#${bigId}` }).locator('button', { hasText: '详情' }).click()
  const modal = page.locator('.modal-wide')
  await expect(modal).toBeVisible({ timeout: 15_000 })
  await expect(modal.locator('.pot-exit')).toContainText('✓ 退出', { timeout: 20_000 })
  await expect(modal.locator('.pot-dropped')).toContainText('已丢弃开头', { timeout: 10_000 })
  const bigTail = modal.locator('.pot-tail pre')
  await expect(bigTail).toContainText(bigCmdNeedle, { timeout: 10_000 })
  await shot(page, 'edge-dark-dropped-badge.png')

  // 终态大输出：上滚 → 暂停钮 → 点击 → 回到底部（终态滚动机械）。
  await pauseFollow(page, modal)
  await shot(page, 'edge-dark-follow-paused.png')
  await modal.locator('.pot-follow').click()
  await expect(modal.locator('.pot-follow')).toBeHidden({ timeout: 5_000 })
  const atBottom = await modal
    .locator('.pot-tail')
    .evaluate((el) => el.scrollTop + el.clientHeight >= el.scrollHeight - 40)
  expect(atBottom, '恢复跟随后回到贴底').toBeTruthy()

  // 模态关闭重开。
  await page.locator('.modal-close').click()
  await expect(modal).toBeHidden({ timeout: 5_000 })

  // —— 运行中进程详情：live 暂停语义（暂停期间内容继续长、视图不动）——
  await rows.filter({ hasText: `#${idB}` }).locator('button', { hasText: '详情' }).click()
  await expect(modal).toBeVisible({ timeout: 15_000 })
  const liveTail = modal.locator('.pot-tail')
  // 流式输出器每秒两行（~26B/s），等内容高过 360px 容器（~5s）。
  await expect
    .poll(async () => liveTail.evaluate((el) => el.scrollHeight > el.clientHeight), {
      timeout: 40_000,
      intervals: [1_000],
    })
    .toBe(true)
  const before = await liveTail.locator('pre').textContent()
  await pauseFollow(page, modal)
  await shot(page, 'edge-dark-live-paused.png')
  // 暂停期间：轮询继续拉新行（内容增长）但视图停在顶部（scrollToBottom 被跳过）。
  await page.waitForTimeout(2_500)
  const after = await liveTail.locator('pre').textContent()
  expect((after?.length ?? 0), '暂停期间内容继续增长').toBeGreaterThan(before?.length ?? 0)
  const stayedTop = await liveTail.evaluate((el) => el.scrollTop < 40)
  expect(stayedTop, '暂停期间视图不跟随').toBeTruthy()
  // 恢复跟随 → 贴底 → 按钮消失。
  await modal.locator('.pot-follow').click()
  await expect(modal.locator('.pot-follow')).toBeHidden({ timeout: 5_000 })
  await expect(modal.locator('.pot-tail')).not.toHaveClass(/is-paused/)

  // —— 暗色下 UI 停止按钮（顺带清理两个流式进程）——
  await page.locator('.modal-close').click()
  await expect(modal).toBeHidden({ timeout: 5_000 })
  await rows.filter({ hasText: `#${idA}` }).locator('button.btn-danger').click()
  await expect(rows.filter({ hasText: `#${idA}` }).locator('span.badge')).toContainText(
    '⛔ 已停止',
    { timeout: 30_000 },
  )
  await rows.filter({ hasText: `#${idB}` }).locator('button.btn-danger').click()
  await expect(rows.filter({ hasText: `#${idB}` }).locator('span.badge')).toContainText(
    '⛔ 已停止',
    { timeout: 30_000 },
  )
  await shot(page, 'edge-dark-killed.png')

  // —— 全程零 JS 报错 ——
  expect(errs.pageErrors, errReport(errs.consoleErrors, errs.pageErrors)).toEqual([])
  expect(errs.consoleErrors, errReport(errs.consoleErrors, errs.pageErrors)).toEqual([])

  // —— 跨 spec 隔离：全量跑时单 gateway 贯穿所有 spec，注册表跨文件
  // 持久——processes-visual 的空态/行数断言依赖干净注册表，这里收尾
  // 重启清场（重启窗口的 WS 断连属预期）。放在零报错断言之后：重启
  // 窗口的 WS 重连噪声不进本测试自己的错误收集。
  await restartGateway()
})

test('后台进程无效深链诚实降级（浅色主题交叉）', async ({ browser }) => {
  test.setTimeout(120_000)
  fs.mkdirSync(SHOT_DIR, { recursive: true })
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1440, height: 900 })
  const errs = watchErrors(page)
  // 显式设浅色（localStorage 直注）：不依赖 colorScheme 模拟——WK WinPort
  // 在宿主系统 dark 下模拟不生效，useTheme 会落 dark（wk6 实录），浅色交叉
  // 必须钉在 localStorage 键上。与 visual spec 的形态基线同源。
  // try/catch：gotoRoute 先经 about:blank 中转，opaque origin 上 setItem 抛
  // SecurityError 污染下方零报错断言（wk7 实录）；真实页面上重跑本脚本时
  // 正常预置（helpers.ts newAuthedPage 同款修法）。
  await page.addInitScript(() => {
    try {
      localStorage.setItem('nemesisbot_theme', 'light')
    } catch {
      /* opaque origin：跳过预置 */
    }
  })
  // router 是 hash 模式：深链形如 /#/executors?tab=processes&focus=999999
  // （2026-11-07 复合页重构：终态深链直写，不再经 /processes 旧路由 redirect）。
  await gotoRoute(page, `/#/executors?tab=processes&focus=999999`)
  const modal = page.locator('.modal-wide')
  // focus 送达并消费：模态自动开；job 不存在 → 诚实「输出已过期」。
  await expect(modal).toBeVisible({ timeout: 15_000 })
  await expect(modal.locator('.pot-empty')).toContainText('输出已过期', { timeout: 20_000 })
  await shot(page, 'edge-light-expired-focus.png')
  // 深链消费后 URL 清理（query-only replace：focus 剥离、tab 保留）。
  await expect(page).toHaveURL(/\/executors\?tab=processes/, { timeout: 5_000 })
  await expect(page).not.toHaveURL(/focus/)
  await page.locator('.modal-close').click()
  await expect(modal).toBeHidden({ timeout: 5_000 })
  // 页面仍可交互（不因无效 id 崩白）。空态断言不做——全量跑时 registry
  // 残留其他 spec 的终态行，空态只在全新 home 成立。
  await expect(page.locator('.page-processes')).toBeVisible({ timeout: 15_000 })

  expect(errs.pageErrors, errReport(errs.consoleErrors, errs.pageErrors)).toEqual([])
  expect(errs.consoleErrors, errReport(errs.consoleErrors, errs.pageErrors)).toEqual([])
})
