/**
 * 用户习惯画像视觉自测（Forge 化重构 2026-11-24，UI 视觉自测纪律；
 * 批 D 增补：五维分组 + 挖掘统计卡 + trust 类别）。
 *
 * 截图四处 UI 人工看图（test-results/profile-visual/shots/）：
 *   1. /#/profile 未开启空态（profile-disabled；统计卡零值诚实形态）
 *   2. 结构化条目列表：五维分组组头 + steps 编号清单 + auto 徽标 +
 *      quality_bar/trust 类别 + evidence 溯源 + pinned + 统计卡有数
 *      （profile-structured）
 *   3. 遥测开关关闭态（telemetry-toggle 取消勾选后 toast + 状态）
 *   4. 开启态 + 条目 + 遥测开（profile-enabled-telemetry-on）
 *
 * 数据种子：直写盘上真相源（config/user_profile.json + sessions/todo_*.json
 * + forge/userprofile/{signals,outbound_lessons}.jsonl）——handler
 * profile.list / profile.stats 每请求重读盘，无需 sweep 即可得结构化条目
 * 与统计卡视觉态；spec 结束恢复现场。
 */
import { test, expect } from './fixtures'
import path from 'node:path'
import fs from 'node:fs'
import { newAuthedPage, gotoRoute } from './helpers'
import { WORKSPACE_DIR } from './env'

const SHOT_DIR = path.join('test-results', 'profile-visual', 'shots')
const PROFILE = path.join(WORKSPACE_DIR, 'config', 'user_profile.json')
const TODO_SEED = path.join(WORKSPACE_DIR, 'sessions', 'todo_visual_seed.json')
const SIGNALS = path.join(WORKSPACE_DIR, 'forge', 'userprofile', 'signals.jsonl')
const LESSONS = path.join(WORKSPACE_DIR, 'forge', 'userprofile', 'outbound_lessons.jsonl')

function habit(over: Record<string, unknown>) {
  return {
    category: 'workflow',
    content: '计划式工作流：先制定计划，多轮回顾确认后转成步骤清单逐项执行',
    confidence: 0.8,
    hits: 2,
    first_seen: '2026-11-20T10:00:00+08:00',
    last_seen: '2026-11-24T10:00:00+08:00',
    status: 'active',
    pinned: false,
    steps: [],
    scope: '',
    evidence: [],
    auto: true,
    ...over,
  }
}

function seedProfile(enabledAt: string | null, habits: unknown[]): void {
  fs.mkdirSync(path.dirname(PROFILE), { recursive: true })
  fs.writeFileSync(
    PROFILE,
    JSON.stringify({ enabled_at: enabledAt, habits }, null, 2),
  )
}

function seedStatsSources(): void {
  // todo 快照（2 完成 + 1 进行中 → 完成度 67%；mtime=now 在 7 天窗内）。
  fs.mkdirSync(path.dirname(TODO_SEED), { recursive: true })
  fs.writeFileSync(
    TODO_SEED,
    JSON.stringify([
      { content: '写单元测试', status: 'completed' },
      { content: '更新文档', status: 'completed' },
      { content: '部署验证', status: 'in_progress' },
    ]),
  )
  // 交互强信号（审批卡 3 + 急停 1）。
  fs.mkdirSync(path.dirname(SIGNALS), { recursive: true })
  fs.writeFileSync(
    SIGNALS,
    [
      { ts_ms: 1764000000000, kind: 'approval', operation: 'file_write', risk_level: 'HIGH', approved: true, duration_ms: 3200, session_key: 's1' },
      { ts_ms: 1764000001000, kind: 'approval', operation: 'process_exec', risk_level: 'CRITICAL', approved: true, duration_ms: 4800, session_key: 's1' },
      { ts_ms: 1764000002000, kind: 'approval', operation: 'file_delete', risk_level: 'HIGH', approved: false, duration_ms: 15000, session_key: 's2' },
      { ts_ms: 1764000003000, kind: 'estop', action: 'trigger' },
    ].map((r) => JSON.stringify(r)).join('\n'),
  )
  // 教训队列（1 pending + 1 acked → 分流计数）。
  fs.mkdirSync(path.dirname(LESSONS), { recursive: true })
  fs.writeFileSync(
    LESSONS,
    [
      { id: '1764000000001', ts_ms: 1764000000000, kind: 'lesson', category: 'workflow', content: '长任务应先列步骤再执行', confidence: 0.6, status: 'pending' },
      { id: '1764000000002', ts_ms: 1764000001000, kind: 'lesson', category: 'style', content: '工具报错应附现场摘录', confidence: 0.7, status: 'acked' },
    ].map((r) => JSON.stringify(r)).join('\n'),
  )
}

function cleanupSeed(): void {
  for (const f of [PROFILE, TODO_SEED, SIGNALS, LESSONS]) {
    try {
      fs.rmSync(f, { force: true })
    } catch {
      /* ignore */
    }
  }
}

test('profile 视觉自测：未开启空态/结构化条目/遥测开关/开启态', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1440, height: 1400 })
  const consoleErrs: string[] = []
  page.on('console', (m) => {
    if (m.type() === 'error') {
      consoleErrs.push(m.text().slice(0, 300))
      console.log('[console-err] ' + m.text().slice(0, 300))
    }
  })
  page.on('pageerror', (e) => {
    consoleErrs.push('PAGE-ERR: ' + String(e).slice(0, 400))
    console.log('[page-err] ' + String(e).slice(0, 400))
  })

  cleanupSeed()

  // —— 1. 未开启空态（无盘上画像 + config 默认关；统计卡零值诚实形态）——
  await gotoRoute(page, '/#/profile')
  await page.locator('[data-testid="empty-state"]').waitFor({ state: 'visible' })
  await expect(page.locator('[data-testid="stats-card"]')).toContainText('待消费 0')
  await page.screenshot({ path: path.join(SHOT_DIR, '1-profile-disabled.png'), fullPage: true })

  // —— 2. 结构化条目列表（种盘 → 重载；批 D：五维分组 + trust + 统计卡有数）——
  seedStatsSources()
  seedProfile('2026-11-20T09:00:00+08:00', [
    habit({
      // row 0: auto + 4 步清单 + 溯源 + 外部来源徽标（批 A 接口二）
      steps: ['先制定计划', '回顾确认', '转成步骤清单', '测试收尾'],
      evidence: ['session-a', 'session-b'],
      source: 'forge-pattern',
    }),
    habit({
      category: 'quality_bar',
      content: '任务清单常以测试/检查项收尾——完成主体步骤后主动做测试验证再交付',
      confidence: 0.85,
      hits: 4,
      steps: ['跑通单测', '验证结果落盘'],
      evidence: ['todo_web_xxx.json'],
      auto: true,
    }), // row 1
    habit({
      category: 'style',
      content: '用户偏好简洁回复',
      confidence: 0.9,
      hits: 12,
      pinned: true,
      auto: false,
    }), // row 2: 反思条目（manual + pinned，无 steps）
    habit({
      category: 'trust',
      content: '审批卡平均 3 秒批准——对常规文件/进程操作有心理预期，说明精炼即可',
      confidence: 0.7,
      hits: 6,
      evidence: ['approval avg_ms=3000 n=5'],
      auto: true,
    }), // row 3: 批 C 强信号挖掘条目（trust 维度）
  ])
  await gotoRoute(page, '/#/profile')
  await page.locator('[data-testid="habit-row-0"]').waitFor({ state: 'visible' })
  expect(await page.locator('[data-testid^="habit-row-"]').count()).toBe(4)
  await expect(page.locator('[data-testid="habit-step-0-0"]')).toContainText('制定计划')
  await expect(page.locator('[data-testid="auto-badge"]').first()).toBeVisible()
  await expect(page.getByText('质量线')).toBeVisible()
  await expect(page.getByText('溯源：session-a、session-b')).toBeVisible()
  // 批 D：五维分组组头（流程仪式/节奏分工/表达粒度/信任画像各有条目）。
  await expect(page.getByText('流程仪式')).toBeVisible()
  await expect(page.getByText('节奏分工')).toBeVisible()
  await expect(page.getByText('表达粒度')).toBeVisible()
  await expect(page.getByText('信任画像', { exact: false }).first()).toBeVisible()
  // 批 A 接口二：外部来源徽标只在 source 非空的行渲染。
  await expect(page.locator('[data-testid="source-badge-0"]')).toContainText('来自 forge-pattern')
  expect(await page.locator('[data-testid="source-badge-1"]').count()).toBe(0)
  expect(await page.locator('[data-testid="source-badge-2"]').count()).toBe(0)
  expect(await page.locator('[data-testid="source-badge-3"]').count()).toBe(0)
  // 批 D：统计卡有数（todo 2/3≈67% + 教训 1/2 + 审批卡 3 · 急停 1）。
  await expect(page.locator('[data-testid="stats-todos"]')).toContainText('67%')
  await expect(page.locator('[data-testid="stats-todos"]')).toContainText('2/3 项')
  await expect(page.locator('[data-testid="stats-lessons"]')).toContainText('待消费 1')
  await expect(page.locator('[data-testid="stats-lessons"]')).toContainText('已消费 1')
  await expect(page.locator('[data-testid="stats-signals"]')).toContainText('审批卡 3')
  await expect(page.locator('[data-testid="stats-signals"]')).toContainText('急停 1')
  await page.screenshot({ path: path.join(SHOT_DIR, '2-profile-structured.png'), fullPage: true })

  // —— 3. 遥测开关关闭态 ——
  await page.locator('[data-testid="telemetry-toggle"]').uncheck()
  await expect(page.locator('[data-testid="telemetry-toggle"]')).not.toBeChecked()
  await page.screenshot({ path: path.join(SHOT_DIR, '3-profile-telemetry-off.png'), fullPage: true })

  // —— 4. 恢复开 + 开启态整页 ——
  await page.locator('[data-testid="telemetry-toggle"]').check()
  await expect(page.locator('[data-testid="telemetry-toggle"]')).toBeChecked()
  await page.screenshot({ path: path.join(SHOT_DIR, '4-profile-enabled-telemetry-on.png'), fullPage: true })

  cleanupSeed()
  if (consoleErrs.length > 0) {
    console.log('CONSOLE ERRORS:\n' + consoleErrs.join('\n'))
  }
  expect(consoleErrs).toEqual([])
})
