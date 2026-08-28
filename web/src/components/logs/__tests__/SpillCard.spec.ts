import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

// G3：SpillCard —— 挂载即拉取状态、字节数格式化、立即清理走 spill_cleanup
// 并展示删除数、空树禁用清理按钮。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import SpillCard from '../SpillCard.vue'

function spillStatus(over: Partial<any> = {}) {
  return {
    root: '/home/logs/spill',
    files: 2,
    bytes: 2560,
    oldest: '2026-08-20T07:00:00+08:00',
    retention_days: 7,
    threshold_chars: 65536,
    ...over,
  }
}

beforeEach(() => {
  requestMock.mockReset()
})

describe('SpillCard', () => {
  it('挂载即拉取 spill_status 并渲染统计（字节格式化 + 保留期）', async () => {
    requestMock.mockResolvedValue(spillStatus())
    const wrapper = mount(SpillCard)
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('logs', 'spill_status', {})
    expect(wrapper.text()).toContain('2 个文件')
    expect(wrapper.text()).toContain('2.5 KB')
    expect(wrapper.text()).toContain('最早 2026-08-20 07:00:00')
    expect(wrapper.text()).toContain('保留 7 天')
    const btn = wrapper.find('.spill-clean-btn')
    expect(btn.attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })

  it('立即清理 → spill_cleanup → 显示删除数并采用返回的新状态', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'spill_status') return Promise.resolve(spillStatus())
      if (cmd === 'spill_cleanup') {
        return Promise.resolve(spillStatus({ deleted: 1, files: 1, bytes: 60 }))
      }
      return Promise.reject(new Error('unexpected ' + cmd))
    })
    const wrapper = mount(SpillCard)
    await flushPromises()

    await wrapper.find('.spill-clean-btn').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('logs', 'spill_cleanup', {})
    expect(wrapper.text()).toContain('已清理 1 个文件')
    expect(wrapper.text()).toContain('1 个文件') // 新状态
    wrapper.unmount()
  })

  it('空树 → 禁用清理按钮；状态不可用 → 提示文案', async () => {
    requestMock.mockResolvedValue(spillStatus({ files: 0, bytes: 0, oldest: null }))
    const wrapper = mount(SpillCard)
    await flushPromises()
    expect(wrapper.find('.spill-clean-btn').attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('暂无落盘文件')
    wrapper.unmount()

    requestMock.mockRejectedValue('down')
    const wrapper2 = mount(SpillCard)
    await flushPromises()
    expect(wrapper2.text()).toContain('spill 状态不可用')
    wrapper2.unmount()
  })
})
