import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// M6 补测（quality-hardening goal 2026-08-25）：SessionExplorer 批次新增的
// focusSession 定位（T6/U20）—— 外部 locate 切 sessions 子 tab + 按文件名
// stem 拉详情（即使不在当前列表页）、详情缓存、消息回填列表条目、失败容错。
// 后端 session_detail 契约由 handlers/logs/tests.rs 钉住。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import SessionExplorer from '../SessionExplorer.vue'
import type { SessionEntry } from '../mockData'

function entry(id: string): SessionEntry {
  return { id, channel: 'web', startTime: '', lastTime: '', messageCount: 0, model: '', firstMessage: '', triggerCluster: false, messages: [] }
}

function mountExplorer(focusSession?: string | null) {
  return mount(SessionExplorer, {
    props: { sessions: [entry('s1')], requests: [], tasks: [], focusSession: focusSession ?? null },
  })
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockResolvedValue({ messages: [{ role: 'user', content: 'hello', ts: '' }] as any })
})

describe('SessionExplorer focusSession 定位（T6）', () => {
  it('focusSession 变化 → 切回 sessions 子 tab + 按 id 拉 session_detail + 消息回填条目', async () => {
    const w = mountExplorer()
    // 先切到别的子 tab，验证 locate 能切回来
    await w.findAll('.sub-tab').find(b => b.text().includes('集群 RPC 任务'))!.trigger('click')
    expect(w.findAll('.sub-tab').find(b => b.text().includes('集群 RPC 任务'))!.classes()).toContain('active')

    await w.setProps({ focusSession: 's1' })
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('logs', 'session_detail', { session: 's1' })
    // 切回 sessions 子 tab（active）
    expect(w.findAll('.sub-tab').find(b => b.text().includes('对话历史'))!.classes()).toContain('active')
    // 消息回填进 props.sessions 条目
    expect((w.props('sessions') as SessionEntry[])[0].messages.length).toBe(1)
  })

  it('目标不在当前列表页也能拉详情（按 id 直查，不依赖列表）', async () => {
    const w = mountExplorer()
    await w.setProps({ focusSession: 'not-in-list' })
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('logs', 'session_detail', { session: 'not-in-list' })
  })

  it('同一会话二次定位 → 走缓存不再发请求（空列表隔离掉 SessionList 挂载自动选中）', async () => {
    const w = mount(SessionExplorer, {
      props: { sessions: [], requests: [], tasks: [], focusSession: null },
    })
    await w.setProps({ focusSession: 's1' })
    await flushPromises()
    await w.setProps({ focusSession: 's2' }) // 中转变一下才能再触发 watch
    await flushPromises()
    await w.setProps({ focusSession: 's1' })
    await flushPromises()
    const s1calls = requestMock.mock.calls.filter(c => c[2].session === 's1')
    expect(s1calls.length).toBe(1)
  })

  it('详情拉取中显示加载提示，失败静默容错不崩', async () => {
    let release!: (v: unknown) => void
    requestMock.mockImplementation(() => new Promise(r => (release = r)))
    const w = mountExplorer()
    await w.setProps({ focusSession: 's1' })
    await flushPromises()
    expect(w.find('.loading-hint').exists()).toBe(true)

    release({ messages: [] })
    await flushPromises()
    expect(w.find('.loading-hint').exists()).toBe(false)

    // 失败路径
    requestMock.mockRejectedValue(new Error('session 文件损坏'))
    const errSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    await w.setProps({ focusSession: 's2' })
    await flushPromises()
    expect(w.find('.loading-hint').exists()).toBe(false) // finally 复位
    expect(w.exists()).toBe(true)
    errSpy.mockRestore()
  })
})
