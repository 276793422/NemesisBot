import { describe, it, expect } from 'vitest'
import { planRegenerate, type RegenRowLike } from '../regenerate'

// 重新生成规划（2026-10-20）：轮首定位（结构回溯）+ 自动重发守卫裁决。
// 真实语义 = rewind mode:"before" 到轮首 + 重发提问原文；本 spec 钉死定位
// 与守卫，副作用链（rewind/重发）在 ChatPanel。

function row(rowIndex: number, role: string, content: string, imageCount?: number): RegenRowLike {
  return { rowIndex, role, content, imageCount }
}

describe('planRegenerate 轮首定位', () => {
  it('点击 assistant 行 → 本轮提问行（A-1）', () => {
    const msgs = [row(0, 'user', 'q1'), row(1, 'assistant', 'a1')]
    expect(planRegenerate(msgs, 1, 'assistant')).toEqual({
      turnStart: 0,
      content: 'q1',
      noResendReason: null,
    })
  })

  it('点击提问行自身 → 自身即轮首', () => {
    const msgs = [row(0, 'user', 'q1'), row(1, 'assistant', 'a1'), row(2, 'user', 'q2'), row(3, 'assistant', 'a2')]
    expect(planRegenerate(msgs, 2, 'user')!.turnStart).toBe(2)
  })

  it('steer 插话布局（u, steer, a）：点插话行或回复行都定位到原始提问行', () => {
    const msgs = [
      row(0, 'user', 'q1'),
      row(1, 'user', '[steer] 补充一句'),
      row(2, 'assistant', 'a1'),
    ]
    expect(planRegenerate(msgs, 1, 'user')!.turnStart).toBe(0)
    expect(planRegenerate(msgs, 2, 'assistant')!.turnStart).toBe(0)
  })

  it('首轮提问行（index 0）直接定位 0（cut=0 形态）', () => {
    const msgs = [row(0, 'user', 'q1'), row(1, 'assistant', 'a1')]
    expect(planRegenerate(msgs, 0, 'user')!.turnStart).toBe(0)
  })

  it('中间轮：点第三轮回复/提问行 → 各自轮首', () => {
    const msgs = [
      row(0, 'user', 'q1'),
      row(1, 'assistant', 'a1'),
      row(2, 'user', 'q2'),
      row(3, 'assistant', 'a2'),
      row(4, 'user', 'q3'),
      row(5, 'assistant', 'a3'),
    ]
    expect(planRegenerate(msgs, 3, 'assistant')!.turnStart).toBe(2)
    expect(planRegenerate(msgs, 4, 'user')!.turnStart).toBe(4)
  })

  it('翻页窗口边界（rowIndex 跳号）：落在窗口内最早 user 行，不猜窗口外', () => {
    const msgs = [row(50, 'user', 'q50'), row(51, 'assistant', 'a50')]
    expect(planRegenerate(msgs, 51, 'assistant')!.turnStart).toBe(50)
  })
})

describe('planRegenerate 自动重发守卫', () => {
  it('无守卫命中 = null（正常重发）', () => {
    const msgs = [row(0, 'user', '修复登录 bug'), row(1, 'assistant', 'ok')]
    expect(planRegenerate(msgs, 1, 'assistant')!.noResendReason).toBeNull()
  })

  it('轮首即插话行（异常形态）→ 不自动重发', () => {
    const msgs = [row(0, 'user', '[steer] 补充'), row(1, 'assistant', 'a')]
    expect(planRegenerate(msgs, 1, 'assistant')!.noResendReason).toContain('插话')
  })

  it('系统触发轮（[System: 前缀，后端定时任务落盘形态）→ 不自动重发（防副作用重放）', () => {
    const msgs = [row(0, 'user', '[System: cron] 每日站会提醒'), row(1, 'assistant', 'ok')]
    expect(planRegenerate(msgs, 1, 'assistant')!.noResendReason).toContain('系统')
    // 直接点系统 user 行（提问行自身入口）同样命中。
    const msgs2 = [row(0, 'user', '[System: cron] 提醒'), row(1, 'assistant', 'ok')]
    expect(planRegenerate(msgs2, 0, 'user')!.noResendReason).toContain('系统')
  })

  it('提问带图片（历史只有计数没有路径）→ 不自动重发', () => {
    const msgs = [row(0, 'user', '看这张图', 2), row(1, 'assistant', 'a')]
    expect(planRegenerate(msgs, 1, 'assistant')!.noResendReason).toContain('图片')
  })

  it('斜杠命令 → 不自动重发（防副作用重放）', () => {
    const msgs = [row(0, 'user', '/compact'), row(1, 'assistant', 'a')]
    expect(planRegenerate(msgs, 1, 'assistant')!.noResendReason).toContain('斜杠')
  })

  it('空内容 → 不自动重发', () => {
    const msgs = [row(0, 'user', ''), row(1, 'assistant', 'a')]
    expect(planRegenerate(msgs, 1, 'assistant')!.noResendReason).toContain('为空')
  })
})
