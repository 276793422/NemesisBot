import { describe, expect, it } from 'vitest'
import { initialTailOffset, stripAnsi } from '../tailUtil'

describe('stripAnsi', () => {
  it('普通文本原样透传', () => {
    expect(stripAnsi('正在响应来自 127.0.0.1 的请求')).toBe('正在响应来自 127.0.0.1 的请求')
  })

  it('剥 SGR 颜色码（CSI m）', () => {
    expect(stripAnsi('\x1b[32mOK\x1b[0m')).toBe('OK')
    expect(stripAnsi('\x1b[1;31mERR\x1b[0m: boom')).toBe('ERR: boom')
  })

  it('剥光标移动/清屏类 CSI', () => {
    expect(stripAnsi('a\x1b[2Kb\x1b[1Gc')).toBe('abc')
  })

  it('剥 OSC（BEL 结尾与 ST 结尾两种）', () => {
    expect(stripAnsi('\x1b]0;title\x07body')).toBe('body')
    expect(stripAnsi('\x1b]0;title\x1b\\body')).toBe('body')
  })

  it('混合序列连续剥离', () => {
    expect(stripAnsi('\x1b[?25l\x1b]2;x\x07\r\n\x1b[36m»\x1b[39m done')).toBe('\r\n» done')
  })
})

describe('initialTailOffset', () => {
  it('有丢弃：从丢弃线开始（dropped 之前字节已物理不在）', () => {
    expect(initialTailOffset(10_000, 2_000, 4_000)).toBe(2_000)
  })

  it('总量不超过窗口：从头开始', () => {
    expect(initialTailOffset(1_000, 0, 4_000)).toBe(0)
    expect(initialTailOffset(4_000, 0, 4_000)).toBe(0)
  })

  it('超窗口：取总量-窗口的尾块起点', () => {
    expect(initialTailOffset(10_000, 0, 4_000)).toBe(6_000)
  })

  it('空缓冲：0', () => {
    expect(initialTailOffset(0, 0, 4_000)).toBe(0)
  })
})
