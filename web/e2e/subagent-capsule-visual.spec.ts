/**
 * 子代理胶囊视觉自测（2026-11-06 胶囊化改造，UI 视觉自测纪律）。
 *
 * 走真实 gateway + mock LLM 全链路（E2E_TOOL_SPAWN(2) → spawn 工具
 * background=true → 注册表 running 行 → 前端胶囊），四个状态截图人审：
 *   1. 运行中单胶囊（spinner + 任务文本 + 用时 + 悬停停止钮）
 *   2. 多任务并列（两条胶囊同容器）
 *   3. 完成后消失（终态行不进 live 容器——核心回归的活体验证）
 *   4. 历史工作块 spawn 行钻取（任务文本显示 + /subagents?focus=<id>
 *      深链 → 台账详情模态打开）
 *
 * 依赖 verify-mock-llm.mjs 的 spawn/slow 路由（任务文本含 E2E_SLOW →
 * 子代理 LLM 调用延迟 20s 应答，运行窗口给截图留时间）。
 */
import { test, expect, type Page } from '@playwright/test'
import fs from 'node:fs'
import path from 'node:path'
import { newAuthedPage, sendChat, wsapi } from './helpers'
const SHOT_DIR = path.join('test-results', 'subagent-capsule-visual', 'shots')

function shot(page: Page, name: string): Promise<string> {
  const file = path.join(SHOT_DIR, name)
  return page.screenshot({ path: file, fullPage: false }).then(() => file)
}

interface RegistryTask {
  id: string
  task: string
  status: string
}

async function registryTasks(page: Page): Promise<RegistryTask[]> {
  const r = await wsapi(page, 'subagents', 'list', {})
  if (!r.ok) throw new Error(`subagents.list 失败：${r.error}`)
  return ((r.data as { tasks?: RegistryTask[] })?.tasks ?? [])
}

test('子代理胶囊四态截图：运行中/多任务/完成消失/历史钻取', async ({ browser }) => {
  test.setTimeout(300_000)
  fs.mkdirSync(SHOT_DIR, { recursive: true })
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1440, height: 900 })

  // spawn 工具落地要起子进程 → process_spawn CRITICAL 走审批卡（e2e 安全
  // 种子 pattern=* 全拦）。第一条消息的卡走真实 UI 批准（顺便建立会话，
  // 会话级「全部放行」按钮在有 sid 前是 disabled）；第二条起免卡。
  // 注：process_spawn 是 CRITICAL，卡上「总是允许」被后端忽略（F3 层级
  // 安全门），普通批准即可。

  // —— 状态 1：单胶囊运行中 ——
  // detached 流时序（首轮实证）：spawn 后主代理落罐头回复「已派后台子代理
  // 任务…」，而聊天里下一条可见回复就是子代理完成本身——所以**不能**先等
  // 最终回复再轮询（轮询开始时子代理已 done）。批准后立即轮询注册表。
  await sendChat(page, 'E2E_TOOL_SPAWN 帮我后台分析任务一')
  const allowBtn = page.locator('.session-allow-btn')
  const approveBtn = page.locator('.approval-card button.btn-success').first()
  await expect(approveBtn).toBeVisible({ timeout: 30_000 })
  await approveBtn.click()
  await expect(approveBtn).toBeHidden({ timeout: 15_000 })
  // 注册表 running 行先于前端轮询出现（闸门：硬闸 status==='running' 生效中）。
  await expect
    .poll(async () => (await registryTasks(page)).filter((t) => t.status === 'running').length, {
      timeout: 30_000,
      intervals: [1_000],
    })
    .toBe(1)
  const capsules = page.locator('.subagent-capsule')
  await expect(capsules).toHaveCount(1, { timeout: 15_000 })
  // 胶囊四件套：spinner + 任务文本 + 用时 + 停止钮。
  const cap1 = capsules.first()
  await expect(cap1.locator('.sc-spinner')).toBeVisible()
  await expect(cap1.locator('.sc-task')).toContainText('子代理胶囊视觉验证任务一')
  expect((await cap1.locator('.sc-elapsed').textContent()) ?? '').toMatch(/\d+[ms]/)
  await expect(cap1.locator('.sc-stop')).toBeAttached()
  // 终态行不进 live 容器：容器里没有已完成文案。
  const strip1 = page.locator('.subagent-capsules')
  await expect(strip1).not.toContainText('已完成')
  await shot(page, 'state1-running-capsule.png')
  await strip1.screenshot({ path: path.join(SHOT_DIR, 'state1-capsules-strip.png') })

  // 会话已建立（首条消息已落）→ 开会话级全部放行（真实 UI 路径；第二条
  // spawn 免卡，不用再点批准）。
  await expect(allowBtn).toBeEnabled({ timeout: 15_000 })
  await allowBtn.click()
  await expect(allowBtn).toHaveClass(/active/, { timeout: 10_000 })

  // —— 状态 2（多任务并列）：第一条还在跑时发第二条 ——
  await sendChat(page, 'E2E_TOOL_SPAWN2 帮我后台编排回归测试')
  await expect
    .poll(async () => (await registryTasks(page)).filter((t) => t.status === 'running').length, {
      timeout: 30_000,
      intervals: [1_000],
    })
    .toBe(2)
  await expect(capsules).toHaveCount(2, { timeout: 15_000 })
  // 注册表 newest-first（新任务在前），断言按文本锚定不按位次。
  await expect(capsules.filter({ hasText: '任务一' })).toHaveCount(1, { timeout: 15_000 })
  await expect(capsules.filter({ hasText: '任务二' })).toHaveCount(1, { timeout: 15_000 })
  await shot(page, 'state2-two-capsules.png')

  // —— 状态 3：完成后消失（慢速臂 ~20s，两条同窗口收尾）——
  await expect
    .poll(async () => (await registryTasks(page)).filter((t) => t.status === 'done').length, {
      timeout: 60_000,
      intervals: [2_000],
    })
    .toBeGreaterThanOrEqual(2)
  // 台账已终态 → 胶囊随轮询/推帧收走（无宽限窗）。
  await expect(page.locator('.subagent-capsules')).toHaveCount(0, { timeout: 15_000 })
  await shot(page, 'state3-completed-gone.png')

  // —— 状态 4：历史工作块 spawn 行钻取 ——
  await page.reload()
  const headers = page.locator('.wt-header')
  await expect(headers.last()).toBeVisible({ timeout: 20_000 })
  // 落地态默认折叠：点开最后一块（任务二那轮）。
  await headers.last().click()
  const spawnRow = page.locator('.wt-tool-spawn').last()
  await expect(spawnRow).toBeVisible()
  await expect(spawnRow.locator('.wt-tool-task')).toContainText('任务二')
  await shot(page, 'state4-work-block-expanded.png')
  // pushState 捕获：消费端（SubagentsView）openDetail 后立即剥掉 focus 参
  // （URL 清理），落地 URL 不含 focus——断言时机落在剥参之后就永远等不到。
  // 在点击前包一层 pushState 记录，确定性捕获路由带过的 URL（无竞态）。
  await page.evaluate(() => {
    const w = window as unknown as { __pushUrls?: string[] }
    w.__pushUrls = []
    const orig = history.pushState.bind(history)
    history.pushState = function (...args: Parameters<History['pushState']>) {
      w.__pushUrls!.push(String(args[2] ?? ''))
      return orig(...args)
    }
  })
  await spawnRow.click()
  await page.waitForURL(/\/subagents/, { timeout: 15_000 })
  // 深链 focus=<id> → 台账详情模态自动打开、聚焦被点任务（模态打开本身
  // 就是 focus 送达并消费的证据——openDetail 只由 route.query.focus 触发）。
  await expect(page.locator('.modal-wide')).toBeVisible({ timeout: 15_000 })
  await expect(page.locator('.modal-wide')).toContainText('任务二')
  await shot(page, 'state4-subagents-focus-detail.png')
  // 路由层证据：pushState 确实带过 /subagents?focus=<id> 深链。
  const pushUrls = await page.evaluate(
    () => (window as unknown as { __pushUrls?: string[] }).__pushUrls ?? [],
  )
  expect(pushUrls.some((u) => /\/subagents\?focus=/.test(u))).toBeTruthy()
})

test('胶囊停止钮真实取消链路 + 深色主题胶囊形态', async ({ browser }) => {
  test.setTimeout(180_000)
  fs.mkdirSync(SHOT_DIR, { recursive: true })
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1440, height: 900 })
  // stop() 的 window.confirm 原生守卫——Playwright 默认 dismiss（=取消），
  // 必须显式接受，否则 subagents.cancel 永远发不出（真实链路的关键一环）。
  page.on('dialog', (d) => d.accept())

  // 本 spec 与主四态 test 共用 gateway 的默认会话：若主 test 已开会话级全部
  // 放行（sidecar 持久 + 本页水合），spawn 会被自动批准不发审批卡。真实 UI
  // 点掉它再走卡批准（顺带覆盖全部放行的关闭路径）。
  const allowBtn = page.locator('.session-allow-btn')
  await expect(allowBtn).toBeEnabled({ timeout: 15_000 })
  if (await allowBtn.evaluate((el) => el.classList.contains('active'))) {
    await allowBtn.click()
    await expect(allowBtn).not.toHaveClass(/active/, { timeout: 10_000 })
  }

  await sendChat(page, 'E2E_TOOL_SPAWN 帮我后台分析任务一')
  const approveBtn = page.locator('.approval-card button.btn-success').first()
  await expect(approveBtn).toBeVisible({ timeout: 30_000 })
  await approveBtn.click()
  await expect
    .poll(async () => (await registryTasks(page)).filter((t) => t.status === 'running').length, {
      timeout: 30_000,
      intervals: [1_000],
    })
    .toBe(1)
  const capsule = page.locator('.subagent-capsule')
  await expect(capsule).toHaveCount(1, { timeout: 15_000 })

  // 深色主题下的胶囊形态（data-theme 属性 + CSS 变量即时生效，无需 reload）。
  await page.evaluate(() => {
    localStorage.setItem('nemesisbot_theme', 'dark')
    document.documentElement.setAttribute('data-theme', 'dark')
  })
  await expect(capsule.locator('.sc-spinner')).toBeVisible()
  await shot(page, 'state5-dark-running-capsule.png')

  // 真实点击停止钮（hover 显现；click 自带 hover）→ confirm 接受 →
  // subagents.cancel → 注册表 cancelled → 终态行不进 live 容器（胶囊消失）。
  await capsule.locator('.sc-stop').click()
  await expect
    .poll(async () => (await registryTasks(page)).filter((t) => t.status === 'cancelled').length, {
      timeout: 30_000,
      intervals: [1_000],
    })
    .toBeGreaterThanOrEqual(1)
  await expect(page.locator('.subagent-capsules')).toHaveCount(0, { timeout: 15_000 })
  await shot(page, 'state5-cancelled-gone.png')
})
