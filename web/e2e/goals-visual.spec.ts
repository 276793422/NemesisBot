/**
 * 目标管理面视觉自测（2026-11-17，UI 视觉自测纪律）。
 *
 * 截图六处 UI 人工看图（test-results/goals-visual/shots/）：
 *   1. /#/goals 空态（goals-empty）
 *   2. 四状态 × 2 变体列表（运行中/已暂停/已停车/已完成 + 文件缺失 + ∞ 进度）
 *   3. blocked 行详情：红卡（code+message）+ 评估台账 continue 记录
 *   4. active 行详情 + 编辑形态（edit 表单）
 *   5. 创建表单（content/session/max_rounds/eval_model 四字段）
 *   6. goal.md 被手删详情：目标文件缺失警告
 *
 * 数据种子：直写盘上真相源（config/goals.json + goals/{id}.goal.md +
 * goals/{id}.evals.jsonl）——handler 每请求重读盘（load_registry），
 * 无需引擎回合即可得到 blocked/done 视觉态；spec 结束恢复现场。
 */
import { test, expect } from './fixtures'
import path from 'node:path'
import fs from 'node:fs'
import { newAuthedPage, gotoRoute } from './helpers'
import { WORKSPACE_DIR } from './env'

const SHOT_DIR = path.join('test-results', 'goals-visual', 'shots')
const REGISTRY = path.join(WORKSPACE_DIR, 'config', 'goals.json')
const GOALS_DIR = path.join(WORKSPACE_DIR, 'goals')

interface SeedEntry {
  id: string
  session_key: string
  status: string
  created_by: string
  max_rounds: number
  rounds_used: number
  eval_model: string | null
  blocked_code: string | null
  blocked_message: string | null
  created_at_ms: number
  updated_at_ms: number
}

function seedEntry(over: Partial<SeedEntry>): SeedEntry {
  return {
    id: 'G91',
    session_key: 'agent:main:session:seed-a',
    status: 'active',
    created_by: 'human',
    max_rounds: 256,
    rounds_used: 3,
    eval_model: null,
    blocked_code: null,
    blocked_message: null,
    created_at_ms: 1700000000000,
    updated_at_ms: 1700000100000,
    ...over,
  }
}

/** 顺序 = 列表行序（registry 数组序 → 行 index）。 */
function seedRegistry(): SeedEntry[] {
  return [
    seedEntry({}), // row 0: active
    seedEntry({ id: 'G92', session_key: 'agent:main:session:seed-b', status: 'paused', rounds_used: 1 }), // row 1
    seedEntry({
      id: 'G93',
      session_key: 'agent:main:session:seed-c',
      status: 'blocked',
      max_rounds: 1,
      rounds_used: 1,
      blocked_code: 'round-limit',
      blocked_message: '轮数预算耗尽（1/1 轮）',
    }), // row 2
    seedEntry({ id: 'G94', session_key: 'agent:main:session:seed-d', status: 'done', rounds_used: 0 }), // row 3
    seedEntry({ id: 'G95', session_key: 'agent:main:session:seed-e' }), // row 4: goal.md 缺失
    seedEntry({ id: 'G96', session_key: 'agent:main:session:seed-f', eval_model: 'small', max_rounds: 0, rounds_used: 7 }), // row 5: ∞
  ]
}

function writeGoalMd(id: string, content: string): void {
  fs.mkdirSync(GOALS_DIR, { recursive: true })
  fs.writeFileSync(path.join(GOALS_DIR, `${id}.goal.md`), content)
}

function writeEvals(id: string, records: unknown[]): void {
  fs.mkdirSync(GOALS_DIR, { recursive: true })
  fs.writeFileSync(path.join(GOALS_DIR, `${id}.evals.jsonl`), records.map((r) => JSON.stringify(r)).join('\n'))
}

function cleanupSeed(): void {
  try {
    fs.rmSync(REGISTRY, { force: true })
  } catch {
    /* ignore */
  }
  try {
    fs.rmSync(GOALS_DIR, { recursive: true, force: true })
  } catch {
    /* ignore */
  }
}

test('goals 视觉自测：空态/四态列表/blocked 红卡详情/编辑形态/创建表单/文件缺失', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  // 高视口：AppLayout 主区是内层滚动容器，fullPage 撑不开（实测恒 1440x900）——
  // 详情/台账/编辑表单在折叠线以下时截图会缺内容，直接加高视口让整页放下。
  await page.setViewportSize({ width: 1440, height: 2200 })
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

  // —— 1. 空态 ——
  await gotoRoute(page, '/#/goals')
  await page.locator('[data-testid="goals-empty"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: path.join(SHOT_DIR, '1-goals-empty.png'), fullPage: true })

  // —— 2. 四状态列表（种盘 → 整页重载）——
  const entries = seedRegistry()
  fs.mkdirSync(path.dirname(REGISTRY), { recursive: true })
  fs.writeFileSync(REGISTRY, JSON.stringify(entries, null, 2))
  writeGoalMd('G91', 'GOALS-VISUAL-A 把回复压到一行\n验收：回复不含换行')
  writeGoalMd('G92', 'GOALS-VISUAL-B 暂停中的目标')
  writeGoalMd('G93', 'GOALS-VISUAL-C 停车中的目标\n验收：性能数据齐备')
  writeGoalMd('G94', 'GOALS-VISUAL-D 已完成的历史目标')
  // G95 不写 goal.md（content_missing 视觉分支）
  writeGoalMd('G96', 'GOALS-VISUAL-F 不限轮数目标')
  writeEvals('G93', [
    { at_ms: 1700000050000, round: 0, verdict: 'continue', gap: '缺性能数据（p95 未采集）', model: 'testai-1.1', duration_ms: 120, raw_excerpt: '' },
    { at_ms: 1700000060000, round: 1, verdict: 'continue', gap: '缺回归对比', model: 'testai-1.1', duration_ms: 95, raw_excerpt: '' },
  ])
  writeEvals('G94', [
    { at_ms: 1700000070000, round: 0, verdict: 'done', gap: null, model: 'testai-1.1', duration_ms: 60, raw_excerpt: '' },
  ])

  await gotoRoute(page, '/#/goals')
  await page.locator('[data-testid="goal-row-0"]').waitFor({ state: 'visible' })
  // 六行 + 四态徽标 + ∞ 进度 + blocked 行启用按钮 + done 行只剩删除。
  const rowCount = await page.locator('[data-testid^="goal-row-"]').count()
  expect(rowCount).toBe(6)
  await expect(page.locator('[data-testid="goal-status-0"]')).toContainText('运行中')
  await expect(page.locator('[data-testid="goal-status-1"]')).toContainText('已暂停')
  await expect(page.locator('[data-testid="goal-status-2"]')).toContainText('已停车')
  await expect(page.locator('[data-testid="goal-status-3"]')).toContainText('已完成')
  await expect(page.locator('[data-testid="enable-2"]')).toBeVisible()
  await expect(page.locator('[data-testid="disable-3"]')).toHaveCount(0)
  await expect(page.locator('[data-testid="delete-3"]')).toBeVisible()
  await expect(page.getByText('7/∞ 轮')).toBeVisible()
  await page.screenshot({ path: path.join(SHOT_DIR, '2-goals-list-4status.png'), fullPage: true })

  // —— 3. blocked 行详情：红卡 + 评估台账 ——
  // （message 断言收窄到详情红卡——同一文本也在列表「停车原因」列；详情外层
  // 容器也是 .card 且 hasText 连带命中祖先，.last() 取 DOM 序内层红卡）
  await page.locator('[data-testid="goal-row-2"]').click()
  const blockedCard = page.locator('.card').filter({ hasText: '停车（round-limit）' }).last()
  await expect(blockedCard).toBeVisible()
  await expect(blockedCard).toContainText('轮数预算耗尽（1/1 轮）')
  await expect(page.getByText('缺性能数据（p95 未采集）')).toBeVisible()
  await page.screenshot({ path: path.join(SHOT_DIR, '3-goals-detail-blocked.png'), fullPage: true })

  // —— 4. active 行详情 + 编辑形态 ——
  await page.locator('[data-testid="goal-row-0"]').click()
  await page.locator('[data-testid="detail-content"]').waitFor({ state: 'visible' })
  await page.locator('[data-testid="edit-btn"]').click()
  await page.locator('[data-testid="edit-content"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: path.join(SHOT_DIR, '4-goals-edit-form.png'), fullPage: true })
  // 收起编辑（取消按钮无 testid——creating/editing 互斥保证任一时刻仅一个「取消」在 DOM）
  await page.getByRole('button', { name: '取消' }).click()

  // —— 5. 创建表单 ——
  await page.locator('[data-testid="create-btn"]').click()
  await page.locator('[data-testid="create-content"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: path.join(SHOT_DIR, '5-goals-create-form.png'), fullPage: true })
  await page.getByRole('button', { name: '取消' }).click()

  // —— 6. goal.md 缺失详情 ——
  await page.locator('[data-testid="goal-row-4"]').click()
  await expect(page.getByText('目标文件缺失')).toBeVisible()
  await page.screenshot({ path: path.join(SHOT_DIR, '6-goals-detail-missing.png'), fullPage: true })

  cleanupSeed()
  if (consoleErrs.length > 0) {
    console.log('CONSOLE ERRORS:\n' + consoleErrs.join('\n'))
  }
  expect(consoleErrs).toEqual([])
})
