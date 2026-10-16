/**
 * e2e/board-ui.spec.ts — 看板页面真实 UI 路径（整改复核 §四.3）
 *
 * 现有 board.spec 只走 WSAPI 数据面（issue.create/list/status 直发）；本
 * spec 补 **页面按钮与 DOM 状态** 路径：建单表单 → 详情弹窗状态转移按钮 →
 * 评论发表 → 时间线 → 看板列卡片，全部经真实点击/输入驱动（不绕 UI）。
 *
 * 状态机（boardMeta TRANSITIONS，镜像 crates/nemesis-board state_machine）：
 * backlog → in_progress → done；终态无出边。
 */
import { expect, test } from '@playwright/test'
import { newAuthedPage } from './helpers'
import { BASE_URL } from './env'

test('看板 UI 全路径：建单 → 状态转移 → 评论 → 时间线 → 看板列卡片', async ({ browser }) => {
  test.setTimeout(180_000)
  const page = await newAuthedPage(browser)

  // hash 路由（createWebHashHistory）——path 形态 /board 会匹配 `/`（chat），
  // 首版失败的根因；必须走 /#/board。
  await page.goto(`${BASE_URL}/#/board`)
  // .first()：8 个页签 tab 同类名，strict mode 下裸 locator 直接违反唯一性。
  await expect(page.locator('.tabs .tab').first()).toBeVisible({ timeout: 20_000 })

  // ---- ① 列表页签 → 新建 Issue 表单 ----
  await page.locator('.tabs .tab', { hasText: '列表' }).click()
  const newBtn = page.locator('button.btn-primary', { hasText: '新建 Issue' })
  await expect(newBtn).toBeVisible({ timeout: 20_000 })
  await newBtn.click()

  const marker = `UI路径建单-${Date.now()}`
  const titleInput = page.locator('.modal input.form-input[placeholder="一句话描述任务"]')
  await expect(titleInput).toBeVisible({ timeout: 10_000 })
  await titleInput.fill(marker)
  await page.locator('.modal-footer button.btn-primary', { hasText: '创建' }).click()

  // 新单出现在列表（建单成功 = issue.create + 刷新链路生效）
  const row = page.locator('tr.issue-row', { hasText: marker })
  await expect(row).toBeVisible({ timeout: 20_000 })
  await expect(row.locator('code')).toContainText(/^NB-\d+$/)

  // ---- ② 行点击开详情弹窗 → 状态转移按钮 ----
  await row.click()
  const modal = page.locator('.modal', { hasText: marker })
  await expect(modal).toBeVisible({ timeout: 10_000 })
  await expect(modal.locator('.detail-meta .badge').first()).toContainText('Backlog')

  await modal.locator('button', { hasText: '→ 进行中' }).click()
  // 状态徽标实时更新（transition 成功 → detail 重拉）
  await expect(modal.locator('.detail-meta .badge').first()).toContainText('进行中', {
    timeout: 15_000,
  })

  // ---- ③ 评论：textarea 输入 → 发表评论 → 评论列表出现 ----
  const commentText = `UI路径评论-${Date.now()}`
  const commentBox = modal.locator('textarea.form-textarea[placeholder*="添加评论"]')
  await expect(commentBox).toBeVisible({ timeout: 10_000 })
  await commentBox.fill(commentText)
  await modal.locator('button.btn-primary', { hasText: '发表评论' }).click()
  await expect(modal.locator('.modal-body').getByText(commentText)).toBeVisible({
    timeout: 15_000,
  })

  // ---- ④ 时间线页签：状态转移与评论已落统一时间线 ----
  await modal.locator('button', { hasText: '时间线' }).click()
  const timelineRows = modal.locator('.timeline-row')
  await expect(timelineRows.first()).toBeVisible({ timeout: 15_000 })
  // 至少含建单、状态转移、评论三类事件（>1 行 = 时间线聚合链路生效）
  expect(await timelineRows.count()).toBeGreaterThanOrEqual(3)

  // ---- ⑤ 关闭弹窗 → 看板页签：卡片落在「进行中」列 ----
  await page.locator('.modal-backdrop').click({ position: { x: 5, y: 5 } })
  await expect(modal).toBeHidden({ timeout: 10_000 })
  await page.locator('.tabs .tab', { hasText: '看板' }).click()
  const inProgressCol = page.locator('.kanban-col', { hasText: '进行中' })
  await expect(
    inProgressCol.locator('.kanban-card', { hasText: marker }),
  ).toBeVisible({ timeout: 20_000 })

  await page.context().close()
})
