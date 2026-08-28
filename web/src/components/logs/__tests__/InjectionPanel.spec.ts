import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

// G2（U9 ②）：InjectionPanel —— 台账聚合展开拉取（带会话缓存）、轮次
// 注入来源标签、逐轮校验重放与判定徽标、无台账提示。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import InjectionPanel from '../InjectionPanel.vue'

function ledgerSummary(over: Partial<any> = {}) {
  return {
    available: true,
    session: 'agent_main_session_s1',
    total_rounds: 1,
    total_injections: 1,
    rounds: [
      {
        round: 1,
        trace_id: 't1',
        ts: '2026-08-28T07:00:00+08:00',
        messages_count: 4,
        history_len: 3,
        injections: [{ source: 'context_digest', index: 0, role: 'user', chars: 120 }],
        voice_append: false,
        summary_used: true,
        summary_covers_up_to: 2,
      },
    ],
    ...over,
  }
}

async function mountPanel(session = 'agent_main_session_s1') {
  const wrapper = mount(InjectionPanel, { props: { session } })
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  requestMock.mockReset()
})

describe('InjectionPanel', () => {
  it('默认折叠不拉取；展开后拉取台账并显示聚合计数', async () => {
    requestMock.mockResolvedValue(ledgerSummary())
    const wrapper = await mountPanel()

    expect(requestMock).not.toHaveBeenCalled()
    expect(wrapper.find('.panel-body').exists()).toBe(false)

    await wrapper.find('.panel-toggle').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('logs', 'injection_summary', {
      session: 'agent_main_session_s1',
    })
    expect(wrapper.find('.toggle-count').text()).toContain('1 轮')
    expect(wrapper.find('.toggle-count').text()).toContain('1 次注入')

    // 会话缓存：折叠再展开不重复拉取
    await wrapper.find('.panel-toggle').trigger('click')
    await wrapper.find('.panel-toggle').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })

  it('轮次行渲染注入来源标签与摘要标记；校验重放显示逐字节一致徽标', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'injection_summary') return Promise.resolve(ledgerSummary())
      if (cmd === 'replay_verify') {
        return Promise.resolve({ ok: true, verdict: 'byte_exact', request_id: 'r1' })
      }
      return Promise.reject(new Error('unexpected ' + cmd))
    })
    const wrapper = await mountPanel()
    await wrapper.find('.panel-toggle').trigger('click')
    await flushPromises()

    const row = wrapper.find('.round-row')
    expect(row.exists()).toBe(true)
    expect(row.text()).toContain('第 1 轮')
    expect(row.find('.inj-tag').text()).toContain('上下文摘要')
    expect(row.find('.inj-tag').text()).toContain('#0')
    expect(row.text()).toContain('📄 摘要')

    await row.find('.verify-btn').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('logs', 'replay_verify', {
      session: 'agent_main_session_s1',
      round: 1,
      trace_id: 't1',
    })
    const badge = wrapper.find('.verify-result')
    expect(badge.classes()).toContain('ok')
    expect(badge.text()).toContain('逐字节一致')
    wrapper.unmount()
  })

  it('校验带 trace_id（同号轮消歧）；切换会话后丢弃在途结论', async () => {
    let resolveVerify: ((v: any) => void) | null = null
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'injection_summary') return Promise.resolve(ledgerSummary())
      if (cmd === 'replay_verify') {
        return new Promise(resolve => { resolveVerify = resolve })
      }
      return Promise.reject(new Error('unexpected ' + cmd))
    })
    const wrapper = await mountPanel()
    await wrapper.find('.panel-toggle').trigger('click')
    await flushPromises()

    // 在途时切换会话：迟到的结论必须被丢弃，不得挂到新会话上。
    await wrapper.find('.verify-btn').trigger('click')
    expect(wrapper.vm.verifyResults['t1:1']).toBeUndefined()
    await wrapper.setProps({ session: 'agent_main_session_s2' })
    await flushPromises()
    resolveVerify!({ ok: true, verdict: 'byte_exact' })
    await flushPromises()
    expect(wrapper.vm.verifyResults['t1:1']).toBeUndefined()
    wrapper.unmount()
  })

  it('mismatch 徽标展示首差异；unavailable 展示裁剪细节', async () => {
    let mode = 'mismatch'
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'injection_summary') return Promise.resolve(ledgerSummary())
      if (cmd === 'replay_verify') {
        if (mode === 'mismatch') {
          return Promise.resolve({
            ok: false,
            verdict: 'mismatch',
            first_diff: { index: 1, kind: 'content', detail: 'byte 3 differs' },
          })
        }
        return Promise.resolve({ ok: false, verdict: 'unavailable', needed: 5, available: 2 })
      }
      return Promise.reject(new Error('unexpected ' + cmd))
    })
    const wrapper = await mountPanel()
    await wrapper.find('.panel-toggle').trigger('click')
    await flushPromises()

    await wrapper.find('.verify-btn').trigger('click')
    await flushPromises()
    let badge = wrapper.find('.verify-result')
    expect(badge.classes()).toContain('bad')
    expect(badge.text()).toContain('#1 content')

    // 换轮次（用第 2 轮）触发 unavailable 分支
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'injection_summary') {
        return Promise.resolve(
          ledgerSummary({
            total_rounds: 2,
            total_injections: 1,
            rounds: [
              ...ledgerSummary().rounds,
              {
                round: 2,
                trace_id: 't2',
                ts: 'x',
                messages_count: 2,
                history_len: 2,
                injections: [],
                voice_append: false,
                summary_used: false,
                summary_covers_up_to: null,
              },
            ],
          }),
        )
      }
      if (cmd === 'replay_verify') {
        return Promise.resolve({ ok: false, verdict: 'unavailable', needed: 5, available: 2 })
      }
      return Promise.reject(new Error('unexpected ' + cmd))
    })
    mode = 'unavailable'
    // 直接对新出现的轮次校验（重新挂载以拿到 2 轮台账）
    wrapper.unmount()
    const wrapper2 = await mountPanel()
    await wrapper2.find('.panel-toggle').trigger('click')
    await flushPromises()
    const rows = wrapper2.findAll('.round-row')
    expect(rows.length).toBe(2)
    await rows[1].find('.verify-btn').trigger('click')
    await flushPromises()
    badge = rows[1].find('.verify-result')
    expect(badge.classes()).toContain('warn')
    expect(badge.text()).toContain('需 5 条')
    wrapper2.unmount()
  })

  it('无台账会话显示提示；切换会话后重新拉取', async () => {
    requestMock.mockResolvedValue({ available: false, session: 'x', rounds: [], total_rounds: 0, total_injections: 0 })
    const wrapper = await mountPanel()
    await wrapper.find('.panel-toggle').trigger('click')
    await flushPromises()
    expect(wrapper.find('.panel-hint').text()).toContain('无注入台账')

    // 切换会话 → 清空快照并重新拉取新会话
    requestMock.mockResolvedValue(ledgerSummary({ session: 'agent_main_session_s2' }))
    await wrapper.setProps({ session: 'agent_main_session_s2' })
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('logs', 'injection_summary', {
      session: 'agent_main_session_s2',
    })
    expect(wrapper.find('.toggle-count').exists()).toBe(true)
    wrapper.unmount()
  })
})
