/**
 * e2e/board.spec.ts — 看板 WSAPI 表面（零 LLM 参与）。
 *
 * 建单 → 列表可见 → 状态机流转 in_progress → 评论 → 评论可读 →
 * 单条时间线可查。全部走 WSAPI request/response，纯数据面冒烟。
 */
import { expect, test } from '@playwright/test'
import { newAuthedPage, wsapi } from './helpers'

interface IssueView {
  id: number
  number: string
  status?: string
}

test('看板 WSAPI：建单/列表/流转/评论/时间线', async ({ browser }) => {
  const page = await newAuthedPage(browser)

  // 建单
  const created = await wsapi(page, 'board', 'issue.create', { title: 'e2e 冒烟单', priority: 2 })
  expect(created.ok, `issue.create 失败: ${created.error}`).toBe(true)
  const issue = (created.data as { created: boolean; issue: IssueView }).issue
  expect(created.data && (created.data as { created: boolean }).created).toBe(true)
  expect(issue.id).toBeGreaterThan(0)
  expect(issue.number).toMatch(/^NB-/)

  // 列表可见
  const list = await wsapi(page, 'board', 'issue.list', {})
  expect(list.ok, `issue.list 失败: ${list.error}`).toBe(true)
  const rows = (list.data as { issues: IssueView[]; total: number }).issues
  expect(rows.some((r) => r.id === issue.id)).toBe(true)

  // 状态机流转
  const st = await wsapi(page, 'board', 'issue.status', { id: issue.id, status: 'in_progress' })
  expect(st.ok, `issue.status 失败: ${st.error}`).toBe(true)
  expect((st.data as { changed: boolean; issue: IssueView }).issue.status).toBe('in_progress')

  // 评论 + 回读
  const cm = await wsapi(page, 'board', 'comment.add', { issue_id: issue.id, content: 'e2e 评论锚点' })
  expect(cm.ok, `comment.add 失败: ${cm.error}`).toBe(true)
  const cl = await wsapi(page, 'board', 'comment.list', { issue_id: issue.id })
  expect(cl.ok, `comment.list 失败: ${cl.error}`).toBe(true)
  expect(JSON.stringify(cl.data)).toContain('e2e 评论锚点')

  // 单条时间线可查（聚合面）
  const tl = await wsapi(page, 'board', 'issue.timeline', { id: issue.id })
  expect(tl.ok, `issue.timeline 失败: ${tl.error}`).toBe(true)
  expect(tl.data).not.toBeNull()

  await page.context().close()
})
