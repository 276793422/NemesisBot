import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// usePlanning（拆解运行期可观测前端单例，2026-10-11 Part 1 配套）：
// - plan_started 立条目（attempt/startedAt）；1s 心跳自计时；
// - plan_ready/plan_failed 只撤 plan_id 匹配的条目（superseded 防误撤：
//   A 失败 → 第 2 次尝试覆盖 → 迟到的 A 失败不得撤第 2 次）；
// - reconcile 按行快照回填（投影 elapsed 反推起点）/撤下（终态/投影消失），
//   且只触碰入列行（过滤视图安全）；seeded 条目（plan_id 未知）任何
//   ready/failed 都可信。
// useSSE 的 on/off 打桩（连接层不在本测试范围）。

const handlers = new Map<string, ((data?: unknown) => void)[]>()

vi.mock('../useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    const arr = handlers.get(type) || []
    arr.push(handler)
    handlers.set(type, arr)
  }),
  off: vi.fn(),
}))

import { usePlanning, fmtElapsed, ABNORMAL_SECS, _resetPlanningForTest } from '../usePlanning'

function dispatch(type: string, data?: unknown) {
  for (const h of handlers.get(type) || []) h(data)
}

beforeEach(() => {
  handlers.clear()
  _resetPlanningForTest()
  vi.useFakeTimers()
  vi.setSystemTime(0)
})

afterEach(() => {
  _resetPlanningForTest()
  vi.useRealTimers()
})

describe('usePlanning', () => {
  it('plan_started 立条目；elapsed 自计时；fmtElapsed m:ss 格式', () => {
    const { list, elapsed } = usePlanning()
    dispatch('board.plan_started', { plan_id: 'p1', issue_id: 5, issue_number: 'NB-5', attempt: 2 })

    expect(list.value.length).toBe(1)
    const e = list.value[0]
    expect(e.planId).toBe('p1')
    expect(e.issueId).toBe(5)
    expect(e.issueNumber).toBe('NB-5')
    expect(e.attempt).toBe(2)
    expect(e.seeded).toBe(false)
    expect(elapsed(5)).toBe(0)

    vi.advanceTimersByTime(65_000)
    expect(elapsed(5)).toBe(65)
    expect(fmtElapsed(65)).toBe('1:05')
    expect(fmtElapsed(0)).toBe('0:00')
    expect(fmtElapsed(605)).toBe('10:05')
    expect(ABNORMAL_SECS).toBe(1200)
  })

  it('plan_ready/plan_failed 只撤 plan_id 匹配条目（superseded 防误撤）', () => {
    const { list, elapsed } = usePlanning()
    dispatch('board.plan_started', { plan_id: 'p1', issue_id: 5, issue_number: 'NB-5', attempt: 1 })

    // 迟到/异源失败事件：plan_id 不匹配 → 条目保留。
    dispatch('board.plan_failed', { plan_id: 'pX', issue_id: 5 })
    expect(list.value.length).toBe(1)

    // 第 2 次尝试覆盖条目（attempt 更新）。
    dispatch('board.plan_started', { plan_id: 'p2', issue_id: 5, issue_number: 'NB-5', attempt: 2 })
    expect(list.value.length).toBe(1)
    expect(list.value[0].planId).toBe('p2')
    expect(list.value[0].attempt).toBe(2)

    // 第 1 次尝试的失败事件迟到 → 不得撤第 2 次尝试。
    dispatch('board.plan_failed', { plan_id: 'p1', issue_id: 5 })
    expect(list.value.length).toBe(1)

    // 匹配 plan_id 的 ready → 撤下。
    dispatch('board.plan_ready', { plan_id: 'p2', issue_id: 5, subs: [] })
    expect(list.value.length).toBe(0)
    expect(elapsed(5)).toBe(0)
  })

  it('reconcile：投影回填（elapsed 反推起点）/终态或投影消失撤下/未入列行不误撤', () => {
    const { list, elapsed, reconcile } = usePlanning()
    dispatch('board.plan_started', { plan_id: 'p1', issue_id: 5, issue_number: 'NB-5', attempt: 1 })

    // 只入列 issue 7（过滤视图）；issue 5 不在行集 → 不得误撤。
    reconcile([{ id: 7, number: 'NB-7', status: 'backlog', planning: { elapsed_secs: 120, attempt: 1 } }])
    expect(list.value.length).toBe(2)
    const seeded = list.value.find((e) => e.issueId === 7)!
    expect(seeded.seeded).toBe(true)
    expect(seeded.planId).toBe('')
    expect(elapsed(7)).toBe(120)
    expect(elapsed(5)).toBe(0)

    // issue 7 到达终态 → 撤下；issue 5 仍在。
    reconcile([{ id: 7, number: 'NB-7', status: 'done' }])
    expect(list.value.map((e) => e.issueId)).toEqual([5])

    // issue 5 的行没了 planning 投影（SSE 丢失兜底）→ 撤下。
    reconcile([{ id: 5, number: 'NB-5', status: 'backlog' }])
    expect(list.value.length).toBe(0)
  })

  it('seeded 条目（plan_id 未知）任何该单 ready/failed 都可信', () => {
    const { list, reconcile } = usePlanning()
    reconcile([{ id: 7, number: 'NB-7', status: 'backlog', planning: { elapsed_secs: 30, attempt: 1 } }])
    expect(list.value.length).toBe(1)

    dispatch('board.plan_ready', { plan_id: 'whatever', issue_id: 7 })
    expect(list.value.length).toBe(0)
  })

  it('畸形载荷（缺 issue_id / 非数值）诚实忽略，不炸', () => {
    const { list } = usePlanning()
    dispatch('board.plan_started', { plan_id: 'p1' })
    dispatch('board.plan_started', null)
    dispatch('board.plan_failed', {})
    expect(list.value.length).toBe(0)
  })
})
