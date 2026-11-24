import { describe, expect, it, vi, beforeEach } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'

// ProcessOutputTail 折叠态（2026-11-24 改版）：compact 默认收起成一行
// （状态行即全部），点状态行展开输出窗、再点收起；「完整输出 →」深链
// 仅展开态可见且 @click.stop 不触发行收起；full 态（执行单元页详情
// 模态）恒展开、无切换语义，行为与改版前一致。

const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))

vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: requestMock }),
}))
vi.mock('../../../composables/useToast', () => ({
  useToast: () => ({ success: vi.fn(), error: vi.fn() }),
}))
vi.mock('vue-router', () => ({ useRouter: () => ({ push: vi.fn() }) }))

import ProcessOutputTail from '../ProcessOutputTail.vue'

function mockDoneOutput() {
  // 终态输出：一发请求即 done（running=false 且 next_offset >= total_bytes）。
  requestMock.mockResolvedValue({
    command: 'python -B out/v8-freeze/main.py',
    running: false,
    steerable: false,
    total_bytes: 12,
    dropped_bytes: 0,
    started_at_unix: Math.floor(Date.now() / 1000),
    exit_code: 0,
    next_offset: 12,
    chunk: 'line1\nline2\n',
  })
}

beforeEach(() => {
  requestMock.mockReset()
  mockDoneOutput()
})

describe('ProcessOutputTail compact 折叠态 (2026-11-24)', () => {
  it('compact 默认收起：只有状态行，tail/empty 均不渲染', async () => {
    const w = mount(ProcessOutputTail, { props: { jobId: 3, compact: true } })
    await flushPromises()
    // 状态行已渲染（meta 到手），但整个展开区不在文档。
    expect(w.find('.pot-status').exists()).toBe(true)
    expect(w.text()).toContain('#3')
    expect(w.find('.pot-tail').exists()).toBe(false)
    expect(w.find('.pot-empty').exists()).toBe(false)
    expect(w.find('.pot-caret').text()).toBe('▸')
    expect(w.find('.pot-status').classes()).toContain('is-toggle')
    expect(w.classes()).toContain('collapsed')
    w.unmount()
  })

  it('点状态行展开：tail 出现 + caret ▾ + 深链按钮可见；再点收起', async () => {
    const w = mount(ProcessOutputTail, { props: { jobId: 3, compact: true } })
    await flushPromises()
    await w.find('.pot-status').trigger('click')
    expect(w.find('.pot-tail').exists()).toBe(true)
    expect(w.find('.pot-tail').text()).toContain('line1')
    expect(w.find('.pot-caret').text()).toBe('▾')
    expect(w.classes()).not.toContain('collapsed')
    const link = w.find('.pot-link')
    expect(link.exists()).toBe(true)
    expect(link.text()).toContain('完整输出')
    // 再点同一行 = 收起。
    await w.find('.pot-status').trigger('click')
    expect(w.find('.pot-tail').exists()).toBe(false)
    expect(w.find('.pot-caret').text()).toBe('▸')
    w.unmount()
  })

  it('深链按钮 @click.stop：点击后仍保持展开（不触发行收起）', async () => {
    const w = mount(ProcessOutputTail, { props: { jobId: 3, compact: true } })
    await flushPromises()
    await w.find('.pot-status').trigger('click')
    await w.find('.pot-link').trigger('click')
    expect(w.find('.pot-tail').exists()).toBe(true)
    expect(w.find('.pot-caret').text()).toBe('▾')
    w.unmount()
  })

  it('full 态恒展开：初挂即有 tail，状态行无切换语义', async () => {
    const w = mount(ProcessOutputTail, { props: { jobId: 3 } })
    await flushPromises()
    expect(w.find('.pot-tail').exists()).toBe(true)
    expect(w.find('.pot-status').classes()).not.toContain('is-toggle')
    expect(w.find('.pot-caret').exists()).toBe(false)
    // 点状态行不折叠（full 无切换入口）。
    await w.find('.pot-status').trigger('click')
    expect(w.find('.pot-tail').exists()).toBe(true)
    w.unmount()
  })
})
