import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

// L14（2026-11-01）：ExecStatsPanel —— 执行统计展开拉取（带会话缓存）、
// 8 字段表格渲染、聚合器未装配诚实提示、切换会话丢弃在途响应。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import ExecStatsPanel from '../ExecStatsPanel.vue'

function stats(over: Partial<Record<string, number>> = {}) {
  return {
    session_id: 's1',
    stats: {
      tool_calls: 12,
      tool_failures: 2,
      retries: 1,
      validation_failures: 0,
      compactions: 3,
      approvals_total: 4,
      approvals_approved: 3,
      approvals_denied: 1,
      ...over,
    },
  }
}

async function mountPanel(session = 'agent_main_session_s1') {
  const wrapper = mount(ExecStatsPanel, { props: { session } })
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  requestMock.mockReset()
})

describe('ExecStatsPanel', () => {
  it('默认折叠不拉取；展开后调 sessions.stats 并渲染 8 行统计', async () => {
    requestMock.mockResolvedValue(stats())
    const wrapper = await mountPanel()

    expect(requestMock).not.toHaveBeenCalled()
    expect(wrapper.find('.panel-body').exists()).toBe(false)

    await wrapper.find('.panel-toggle').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sessions', 'stats', {
      session_id: 'agent_main_session_s1',
    })
    // 8 行两列：标签 + 数值
    const rows = wrapper.findAll('.stats-table tr')
    expect(rows.length).toBe(8)
    expect(wrapper.find('.stats-table').text()).toContain('工具调用')
    const values = wrapper.findAll('.stats-value').map(v => v.text())
    expect(values).toEqual(['12', '2', '1', '0', '3', '4', '3', '1'])
    // 折叠条聚合计数
    expect(wrapper.find('.toggle-count').text()).toContain('12 次工具调用')
    expect(wrapper.find('.toggle-count').text()).toContain('4 次审批')

    // 会话缓存：折叠再展开不重复拉取
    await wrapper.find('.panel-toggle').trigger('click')
    await wrapper.find('.panel-toggle').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })

  it('聚合器未装配（请求失败）= 诚实提示不渲染表格', async () => {
    requestMock.mockRejectedValue(new Error('会话执行统计未装配（聚合器未运行）'))
    const wrapper = await mountPanel()
    await wrapper.find('.panel-toggle').trigger('click')
    await flushPromises()

    expect(wrapper.find('.stats-table').exists()).toBe(false)
    expect(wrapper.find('.panel-hint').text()).toContain('聚合器未装配')
    wrapper.unmount()
  })

  it('切换会话清空视图并拉取新会话；在途响应不污染新会话', async () => {
    let resolveFirst: ((v: any) => void) | null = null
    requestMock.mockImplementation(
      (_m: string, _cmd: string, data: any) =>
        data.session_id === 'agent_main_session_s1'
          ? new Promise(resolve => { resolveFirst = resolve })
          : Promise.resolve(stats()),
    )
    const wrapper = await mountPanel()
    await wrapper.find('.panel-toggle').trigger('click')
    await flushPromises()

    // 在途（s1 未 resolve）时切换到 s2：迟到的 s1 响应必须被丢弃。
    await wrapper.setProps({ session: 'agent_main_session_s2' })
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sessions', 'stats', {
      session_id: 'agent_main_session_s2',
    })
    expect(wrapper.find('.stats-table').text()).toContain('工具调用')
    resolveFirst!(stats())
    await flushPromises()
    // s1 的迟到数据没有覆盖 s2 的视图（数值仍是 s2 的 12）。
    expect(wrapper.findAll('.stats-value').map(v => v.text())).toEqual([
      '12', '2', '1', '0', '3', '4', '3', '1',
    ])
    wrapper.unmount()
  })
})
