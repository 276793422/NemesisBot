import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'

// WorkTimelineBlock exec 执行中输出尾窗渲染（2026-11-08 ToolOutput 实时流）。
// 契约：running + live_tail → .wt-live-tail pre + 字节 meta；Finished/无尾窗
// 不渲染；replacement 窗更新即重渲染；live 态尾窗变化 emit('grown')。

import WorkTimelineBlock from '../WorkTimelineBlock.vue'
import type { WorkTimeline } from '../../../stores/chat'

function workWithTool(tool: Record<string, unknown>): WorkTimeline {
  return {
    started_at_ms: 1000,
    ended_at_ms: 2000,
    rounds: [{ text: '', tools: [{ tool: 'exec', call_id: 'c1', ok: false, duration_ms: 0, ...tool } as any] }],
  }
}

describe('WorkTimelineBlock exec live tail (2026-11-08)', () => {
  it('running + live_tail 渲染尾窗 pre + 字节 meta', async () => {
    const w = mount(WorkTimelineBlock, {
      props: {
        work: workWithTool({ running: true, live_tail: 'PING 1.2.3.4 32 bytes\n', live_total_bytes: 22 }),
        live: true,
      },
    })
    // live 默认展开（点 header 会折叠——展开态直接断言）。
    expect(w.find('.wt-live-tail').exists()).toBe(true)
    expect(w.find('.wt-live-tail').text()).toContain('PING 1.2.3.4 32 bytes')
    expect(w.find('.wt-live-meta').text()).toContain('22B')
    w.unmount()
  })

  it('Finished（live_tail 已清）不渲染尾窗', async () => {
    const w = mount(WorkTimelineBlock, {
      props: {
        work: workWithTool({ ok: true, duration_ms: 300, result_preview: 'done' }),
        live: false,
      },
    })
    await w.find('.wt-header').trigger('click') // 落地默认折叠，点开
    expect(w.find('.wt-live-tail').exists()).toBe(false)
    w.unmount()
  })

  it('running 但无 live_tail（尚无输出）不渲染', async () => {
    const w = mount(WorkTimelineBlock, {
      props: { work: workWithTool({ running: true }), live: true },
    })
    expect(w.find('.wt-live-tail').exists()).toBe(false)
    w.unmount()
  })

  it('replacement 窗更新：props 替换后 pre 内容跟随（稳态同长度也更新）', async () => {
    const w = mount(WorkTimelineBlock, {
      props: {
        work: workWithTool({ running: true, live_tail: 'aaaa\n', live_total_bytes: 5 }),
        live: true,
      },
    })
    expect(w.find('.wt-live-tail').text()).toContain('aaaa')
    // 稳态替换：长度可能相同（尾窗恒 ≤2048），内容必须跟随。
    await w.setProps({
      work: workWithTool({ running: true, live_tail: 'bbbb\n', live_total_bytes: 2048 }),
    })
    expect(w.find('.wt-live-tail').text()).toContain('bbbb')
    expect(w.find('.wt-live-meta').text()).toContain('2048B')
    w.unmount()
  })

  it('live 态尾窗变化 emit grown（宿主贴底信号）', async () => {
    const w = mount(WorkTimelineBlock, {
      props: {
        work: workWithTool({ running: true, live_tail: 'one\n', live_total_bytes: 4 }),
        live: true,
      },
    })
    await w.setProps({
      work: workWithTool({ running: true, live_tail: 'one\ntwo\n', live_total_bytes: 8 }),
    })
    expect(w.emitted('grown')).toBeTruthy()
    w.unmount()
  })

  it('落地态尾窗变化不 emit grown（只有 live 态贴底）', async () => {
    const w = mount(WorkTimelineBlock, {
      props: {
        work: workWithTool({ running: true, live_tail: 'one\n', live_total_bytes: 4 }),
        live: false,
      },
    })
    await w.setProps({
      work: workWithTool({ running: true, live_tail: 'one\ntwo\n', live_total_bytes: 8 }),
    })
    expect(w.emitted('grown')).toBeFalsy()
    w.unmount()
  })
})
