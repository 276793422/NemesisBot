import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import ToolCallCard from '../ToolCallCard.vue'
import type { ToolEvent } from '../../../stores/chat'

// M1b（2026-09-05）：单条工具调用卡片——状态徽标（running 旋转 / ok ✓ /
// error ✗）、参数摘要首行 ≤120 字、耗时格式、结果预览点击展开（默认收起）。

describe('ToolCallCard', () => {
  it('running: spinner shown, no duration', () => {
    const ev: ToolEvent = { callId: 'c1', tool: 'exec', state: 'running', argsPreview: '{"cmd":"ls -la"}' }
    const w = mount(ToolCallCard, { props: { event: ev } })
    expect(w.find('.tool-spinner').exists()).toBe(true)
    expect(w.find('.tool-name').text()).toBe('exec')
    expect(w.text()).toContain('ls -la')
    expect(w.find('.tool-duration').exists()).toBe(false)
    expect(w.find('.tool-result').exists()).toBe(false)
  })

  it('ok: check badge + duration in seconds', () => {
    const ev: ToolEvent = { callId: 'c1', tool: 'exec', state: 'ok', durationMs: 1500 }
    const w = mount(ToolCallCard, { props: { event: ev } })
    expect(w.find('.tool-spinner').exists()).toBe(false)
    expect(w.find('.tool-state').text()).toBe('✓')
    expect(w.find('.tool-duration').text()).toBe('1.5s')
    expect(w.classes()).toContain('is-ok')
  })

  it('error: cross badge + millisecond duration + error border class', () => {
    const ev: ToolEvent = { callId: 'c1', tool: 'exec', state: 'error', durationMs: 42, resultPreview: 'boom' }
    const w = mount(ToolCallCard, { props: { event: ev } })
    expect(w.find('.tool-state').text()).toBe('✗')
    expect(w.find('.tool-duration').text()).toBe('42ms')
    expect(w.classes()).toContain('is-error')
  })

  it('args summary takes the first line only', () => {
    const ev: ToolEvent = { callId: 'c1', tool: 'write_file', state: 'ok', argsPreview: '{"path":"/tmp/x"}\nsecond line' }
    const w = mount(ToolCallCard, { props: { event: ev } })
    expect(w.find('.tool-args').text()).toBe('{"path":"/tmp/x"}')
    expect(w.text()).not.toContain('second line')
  })

  it('args summary truncates past 120 chars with ellipsis', () => {
    const long = 'a'.repeat(200)
    const ev: ToolEvent = { callId: 'c1', tool: 'exec', state: 'ok', argsPreview: long }
    const w = mount(ToolCallCard, { props: { event: ev } })
    const shown = w.find('.tool-args').text()
    expect([...shown].length).toBe(121) // 120 chars + …
    expect(shown.endsWith('…')).toBe(true)
  })

  it('result preview hidden by default; click expands, click again collapses', async () => {
    const ev: ToolEvent = { callId: 'c1', tool: 'grep', state: 'ok', durationMs: 10, resultPreview: 'match line 1\nmatch line 2' }
    const w = mount(ToolCallCard, { props: { event: ev } })
    expect(w.find('.tool-result').exists()).toBe(false)
    await w.find('.tool-card').trigger('click')
    expect(w.find('.tool-result').exists()).toBe(true)
    expect(w.find('.tool-result').text()).toContain('match line 2')
    await w.find('.tool-card').trigger('click')
    expect(w.find('.tool-result').exists()).toBe(false)
  })

  it('no result preview: click does nothing (card not expandable)', async () => {
    const ev: ToolEvent = { callId: 'c1', tool: 'exec', state: 'ok', durationMs: 10 }
    const w = mount(ToolCallCard, { props: { event: ev } })
    expect(w.find('.tool-card').classes()).not.toContain('is-expandable')
    await w.find('.tool-card').trigger('click')
    expect(w.find('.tool-result').exists()).toBe(false)
  })
})
