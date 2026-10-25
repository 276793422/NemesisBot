import { describe, expect, it } from 'vitest'
import { toLocalInputValue } from '../datetime'

describe('toLocalInputValue', () => {
  it('formats local wall clock as YYYY-MM-DDTHH:mm', () => {
    // 用本地分量构造，断言与 TZ 无关。
    const d = new Date(2026, 9, 26, 8, 5) // local 2026-10-26 08:05
    expect(toLocalInputValue(d)).toBe('2026-10-26T08:05')
  })

  it('zero-pads month/day/hour/minute', () => {
    const d = new Date(2026, 0, 3, 7, 42) // local 2026-01-03 07:42
    expect(toLocalInputValue(d)).toBe('2026-01-03T07:42')
  })

  it('round-trips through new Date() at minute precision', () => {
    // datetime-local 精度是分钟：秒/毫秒归零后解析回读必须还原同一时刻
    //（旧 toISOString 形态在非 UTC 时区下 round-trip 必然平移时区差）。
    const d = new Date(2026, 9, 26, 8, 5, 0, 0)
    expect(new Date(toLocalInputValue(d)).getTime()).toBe(d.getTime())
  })
})
