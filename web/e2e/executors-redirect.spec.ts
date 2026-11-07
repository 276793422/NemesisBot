/**
 * 执行单元复合页 redirect 兼容层 + TAB 状态机专项测试（2026-11-07 充分测试战役）。
 *
 * 针对复合页重构交付报告的残余风险补测：
 *   1. redirect 兼容矩阵：旧 /processes、/subagents 无参落地；旧深链跨
 *      redirect 的 query 全透传（focus 消费后清理只剥 focus、foo=bar 存活
 *      = 透传保真的最强证据）；无效 focus 双侧各自诚实降级（processes=
 *      「输出已过期」/ subagents=「任务不在册」）。
 *   2. TAB 状态机：SPA 存活期间 hash 直改（watch(route.query.tab) 消费
 *      路径）→ 浏览器 back/forward（popstate 路径）→ mount/unmount 循环
 *      ×3 → 模态开着时另一 TAB 深链到达（旧模态随组件卸载收走、新模态
 *      按新 focus 打开）→ 全程 console 零报错。
 *   3. 真实实体全链：spawn 真子代理（审批卡 → running → done，slow 臂
 *      ~20s）→ 旧路由深链 /#/subagents?focus=<realId> → redirect → 台账
 *      详情模态自动开 + URL 清理保 tab。
 *
 * focus 消费语义（产品路径核实）：两个 View 都只在 onMounted 消费
 * route.query.focus——真实深链构造点全在 chat 路由（目标视图必然未挂载）
 * 或 document 加载（初次导航走 redirect 后挂载），spec 时序按真实路径。
 *
 * hash 路由（createWebHashHistory）：goto 换 hash = document 导航；
 * page.evaluate 改 location.hash = 同文档导航（SPA 存活场景）。
 */
import { test, expect, type Page } from './fixtures'
import fs from 'node:fs'
import path from 'node:path'
import { gotoRoute, newAuthedPage, openSessionSidebar, sendChat, wsapi } from './helpers'
import { BASE_URL } from './env'
const SHOT_DIR = path.join('test-results', 'executors-redirect', 'shots')

function shot(page: Page, name: string): Promise<string> {
  const file = path.join(SHOT_DIR, name)
  return page.screenshot({ path: file, fullPage: false, animations: 'disabled' }).then(() => file)
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

test('redirect 兼容矩阵：无参落地/query 全透传/无效深链双侧诚实降级', async ({ browser }) => {
  test.setTimeout(120_000)
  fs.mkdirSync(SHOT_DIR, { recursive: true })
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1440, height: 900 })
  const errs = watchErrors(page)

  // (1) 旧 /processes 无参 → /executors?tab=processes，后台任务面板挂载
  await gotoRoute(page, `/#/processes`)
  await expect(page).toHaveURL(/\/executors\?tab=processes/, { timeout: 15_000 })
  await expect(page.locator('.page-processes')).toBeVisible({ timeout: 15_000 })
  await shot(page, 'redirect-processes-bare.png')

  // (2) 旧 /subagents 无参 → /executors?tab=subagents，子代理面板挂载
  await gotoRoute(page, `/#/subagents`)
  await expect(page).toHaveURL(/\/executors\?tab=subagents/, { timeout: 15_000 })
  await expect(page.locator('.page-subagents')).toBeVisible({ timeout: 15_000 })

  // (3) 旧深链带 focus + 额外参数 → redirect 透传全部 query；focus 消费后
  //     清理只剥 focus，foo=bar 存活 = 透传保真最强证据（redirect 若弄丢
  //     query，foo=bar 必然消失）。无效 id → processes 侧「输出已过期」。
  await gotoRoute(page, `/#/processes?focus=999999&foo=bar`)
  const modal = page.locator('.modal-wide')
  await expect(modal).toBeVisible({ timeout: 15_000 })
  await expect(modal.locator('.pot-empty')).toContainText('输出已过期', { timeout: 20_000 })
  await expect(page).toHaveURL(/foo=bar/, { timeout: 5_000 })
  await expect(page).toHaveURL(/tab=processes/)
  await expect(page).not.toHaveURL(/focus/)
  await shot(page, 'redirect-processes-expired-focus.png')
  await page.locator('.modal-close').click()
  await expect(modal).toBeHidden({ timeout: 5_000 })

  // (4) 旧深链 subagents 侧无效 id → 「任务不在册」诚实降级 + 清理保 tab
  await gotoRoute(page, `/#/subagents?focus=NBX-nonexistent`)
  await expect(page.locator('.modal-wide')).toBeVisible({ timeout: 15_000 })
  await expect(page.locator('.modal-wide .empty-state')).toContainText('任务不在册', {
    timeout: 20_000,
  })
  await expect(page).toHaveURL(/tab=subagents/, { timeout: 5_000 })
  await expect(page).not.toHaveURL(/focus/)
  await shot(page, 'redirect-subagents-notfound-focus.png')
  await page.locator('.modal-close').click()
  await expect(page.locator('.modal-wide')).toBeHidden({ timeout: 5_000 })

  expect(errs.pageErrors, errReport(errs.consoleErrors, errs.pageErrors)).toEqual([])
  expect(errs.consoleErrors, errReport(errs.consoleErrors, errs.pageErrors)).toEqual([])
})

test('TAB 状态机：hash 直改/back-forward/mount-unmount 循环/模态并存时序', async ({ browser }) => {
  test.setTimeout(120_000)
  fs.mkdirSync(SHOT_DIR, { recursive: true })
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1440, height: 900 })
  const errs = watchErrors(page)

  // (1) SPA 存活（chat 路由）期间 hash 直改 → processes 面板挂载
  //     （watch(route.query.tab) 消费路径 forward 方向）
  await gotoRoute(page, `/#/executors?tab=processes`)
  await expect(page.locator('.page-processes')).toBeVisible({ timeout: 15_000 })

  // (2) hash 直改切回 subagents → 面板互换（旧面板卸载）
  await page.evaluate(() => {
    location.hash = '#/executors?tab=subagents'
  })
  await expect(page.locator('.page-subagents')).toBeVisible({ timeout: 15_000 })
  await expect(page.locator('.page-processes')).toHaveCount(0)

  // (3) 浏览器后退 → popstate → processes 恢复
  await page.goBack()
  await expect(page.locator('.page-processes')).toBeVisible({ timeout: 15_000 })
  await expect(page.locator('.page-subagents')).toHaveCount(0)

  // (4) 前进 → subagents 恢复
  await page.goForward()
  await expect(page.locator('.page-subagents')).toBeVisible({ timeout: 15_000 })
  await expect(page.locator('.page-processes')).toHaveCount(0)

  // (5) mount/unmount 循环 ×3（组件销毁重建；轮询定时器随 onBeforeUnmount
  //     清理、onMounted 重建——循环后面板仍健康）
  for (let i = 0; i < 3; i++) {
    await page.locator('.exec-tab', { hasText: '后台任务' }).click()
    await expect(page.locator('.page-processes')).toBeVisible({ timeout: 10_000 })
    await page.locator('.exec-tab', { hasText: '子代理' }).click()
    await expect(page.locator('.page-subagents')).toBeVisible({ timeout: 10_000 })
  }

  // (6) 模态开着时另一 TAB 深链到达（异常时序）：processes 侧先开「输出
  //     已过期」模态（chat→深链到达 = 产品真实路径：ProcessesView 此刻
  //     未挂载 → mount 消费 focus），随后 hash 直改到 subagents 带另一个
  //     无效 focus → 旧模态随组件卸载收走、新模态按新 focus 打开。
  await page.evaluate(() => {
    location.hash = '#/executors?tab=processes&focus=888888'
  })
  const expiredModal = page.locator('.modal-wide', { hasText: '输出已过期' })
  await expect(expiredModal).toBeVisible({ timeout: 15_000 })
  await shot(page, 'tab-state-modal-then-deeplink-before.png')
  await page.evaluate(() => {
    location.hash = '#/executors?tab=subagents&focus=NBX-another'
  })
  await expect(page.locator('.page-subagents')).toBeVisible({ timeout: 15_000 })
  const notFoundModal = page.locator('.modal-wide', { hasText: '任务不在册' })
  await expect(notFoundModal).toBeVisible({ timeout: 20_000 })
  // 旧模态已随 ProcessesView 卸载收走（不是两模态并存）
  await expect(expiredModal).toHaveCount(0)
  // 清理保 tab 剥 focus
  await expect(page).toHaveURL(/tab=subagents/, { timeout: 5_000 })
  await expect(page).not.toHaveURL(/focus/)
  await shot(page, 'tab-state-modal-then-deeplink-after.png')

  expect(errs.pageErrors, errReport(errs.consoleErrors, errs.pageErrors)).toEqual([])
  expect(errs.consoleErrors, errReport(errs.consoleErrors, errs.pageErrors)).toEqual([])
})

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

test('真实实体全链：spawn 子代理 → 旧路由深链 redirect → 台账模态自动开', async ({ browser }) => {
  test.setTimeout(180_000)
  fs.mkdirSync(SHOT_DIR, { recursive: true })
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1440, height: 900 })
  const errs = watchErrors(page)

  // spawn 工具 process_spawn CRITICAL 走审批卡 → 先种干净新会话（裸 sendChat
  // 落在 auto-select 最近活跃会话上，可能是前面 spec 留下的项目会话 → 发送
  // 命中「项目当前不可用」诚实报错，卡永不来）。等 active 行变化再 fill
  // （disconnect/resync/subagent-capsule 三 spec 同款纪律）。
  await openSessionSidebar(page)
  const prevActive = await page
    .locator('.session-item.active')
    .getAttribute('data-session-id')
    .catch(() => null)
  await page.locator('.fn-new-chat').click()
  // toPass 双条件环而非 expect.poll：poll 回调内 getAttribute 抛错立即传播
  // 不重试（capsule spec 同款注记）；且需断言 active 行可见 AND id 已变，
  // 裸「id 变化」在零 active 行窗口会假绿。
  await expect(async () => {
    const row = page.locator('.session-item.active')
    await expect(row.first()).toBeVisible({ timeout: 2_000 })
    const id = await row.first().getAttribute('data-session-id', { timeout: 1_000 })
    expect(id, `active id 未变：${prevActive}`).not.toBe(prevActive)
  }).toPass({ timeout: 15_000 })

  // spawn（mock spawn_tool 臂 → 任务文本固定含「任务一」，E2E_SLOW 慢速臂
  // ~20s running 窗口）→ 审批卡批准 → 注册表 running 行。
  await sendChat(page, 'E2E_TOOL_SPAWN 帮我后台分析 redirect 兼容验证')
  const approveBtn = page.locator('.approval-card button.btn-success').first()
  await expect(approveBtn).toBeVisible({ timeout: 30_000 })
  await approveBtn.click()
  await expect(approveBtn).toBeHidden({ timeout: 15_000 })
  let spawnId = ''
  await expect
    .poll(
      async () => {
        const t = (await registryTasks(page)).find((x) => (x.task ?? '').includes('任务一'))
        if (t) spawnId = t.id
        return t?.status ?? ''
      },
      { timeout: 30_000, intervals: [1_000] },
    )
    .toBe('running')
  expect(spawnId, 'running 行必须带出 id 供深链构造').toBeTruthy()

  // 等 done（slow 臂 ~20s）——台账终态行是深链目标。
  await expect
    .poll(
      async () => (await registryTasks(page)).find((x) => x.id === spawnId)?.status ?? '',
      { timeout: 60_000, intervals: [2_000] },
    )
    .toBe('done')

  // 旧路由深链（redirect 全链）：/#/subagents?focus=<realId> → redirect →
  // /executors?tab=subagents&focus=<id> → 挂载消费 → 台账详情模态自动开、
  // 聚焦该任务 → URL 清理剥 focus 保 tab。
  await gotoRoute(page, `/#/subagents?focus=${encodeURIComponent(spawnId)}`)
  const modal = page.locator('.modal-wide')
  await expect(modal).toBeVisible({ timeout: 15_000 })
  await expect(modal).toContainText('任务一')
  await expect(page).toHaveURL(/tab=subagents/, { timeout: 5_000 })
  await expect(page).not.toHaveURL(/focus/)
  await shot(page, 'redirect-real-entity-modal.png')
  // 模态内容确属该任务（id 呈现），不是首行巧合
  await expect(modal).toContainText(spawnId)

  expect(errs.pageErrors, errReport(errs.consoleErrors, errs.pageErrors)).toEqual([])
  expect(errs.consoleErrors, errReport(errs.consoleErrors, errs.pageErrors)).toEqual([])
})
