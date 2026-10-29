import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// BUG-20261030-206 回归锁：EventStream（Logs 实时事件流 tab）此前不消费
// SSE `resync`——L2 契约（useSSE.ts / server.rs SSE 端点注释）要求订阅方
// 收到 resync（缺口滑出重放窗口 / 网关重启 seq 域重置）时全量刷新兜底，
// ChatPanel 有 onSSEResync，本组件漏实现：断线窗口内发布超回放窗（1000 帧）
// 后实时流带洞不自愈，只有重挂载才恢复。修复 = 订阅 resync → 清状态重拉
// /api/logs（文件全量真相源天然覆盖洞口），重载期间实时帧走 pendingDuringLoad
// 缓冲 + seq 去重 flush。
//
// 用例：
// 1. resync 触发全量重载：entries 重置 + 重拉历史 + 重载后旧 seq 实时帧去重；
// 2. 首载在飞（historyLoaded=false）时 resync 不叠加第二次拉取；
// 3. paused（用户冻结视图）时 resync 不重灌。

const sseHandlers = new Map<string, (data?: unknown) => void>()
vi.mock('../../../composables/useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    sseHandlers.set(type, handler)
  }),
  off: vi.fn((type: string) => {
    sseHandlers.delete(type)
  }),
  connectEvents: vi.fn(),
  sseReadyState: vi.fn(() => 1),
}))

const fetchCalls: string[] = []
// 每次调用的响应页（按调用序消费；耗尽后重复最后一页）
let historyPages: Array<Array<Record<string, unknown>>> = []
const authedFetchMock = vi.fn((...args: any[]) => {
  fetchCalls.push(String(args[0]))
  const idx = Math.min(fetchCalls.length - 1, historyPages.length - 1)
  const page = historyPages[idx] ?? []
  return Promise.resolve({
    ok: true,
    json: () => Promise.resolve({ entries: page }),
  })
})
vi.mock('../../../lib/authFetch', () => ({
  authedFetch: (...args: any[]) => authedFetchMock(...args),
}))

import EventStream from '../EventStream.vue'

function logEntry(seq: number, message: string) {
  return {
    seq,
    level: 'INFO',
    source: 'general',
    component: 'test_comp',
    message,
    timestamp: '2026-10-30T00:00:00+08:00',
    file: '',
    line: 0,
    fields: {},
  }
}

function fire(type: string, data?: unknown) {
  const h = sseHandlers.get(type)
  if (!h) throw new Error(`no SSE handler registered for ${type}`)
  h(data)
}

beforeEach(() => {
  sseHandlers.clear()
  fetchCalls.length = 0
  historyPages = []
  authedFetchMock.mockClear()
  authedFetchMock.mockImplementation((...args: any[]) => {
    fetchCalls.push(String(args[0]))
    const idx = Math.min(fetchCalls.length - 1, historyPages.length - 1)
    const page = historyPages[idx] ?? []
    return Promise.resolve({
      ok: true,
      json: () => Promise.resolve({ entries: page }),
    })
  })
})

describe('EventStream resync（BUG-20261030-206）', () => {
  it('resync 触发全量重载：清空重拉 + 重载后旧 seq 实时帧去重', async () => {
    // 首次历史：seq 10、11（重放窗口内能补到的部分）
    historyPages = [[logEntry(10, 'old-10'), logEntry(11, 'old-11')]]
    const wrapper = mount(EventStream)
    await flushPromises()
    expect(fetchCalls.length).toBe(1)
    expect((wrapper.vm as any).entries.length).toBe(2)

    // 实时帧推进游标（断线前最后看到的 seq）
    fire('log', logEntry(15, 'live-15'))
    expect((wrapper.vm as any).entries.length).toBe(3)

    // 断线窗口滑出重放窗 → 服务端发 resync；重载返回覆盖洞口的完整页
    historyPages = [[
      logEntry(10, 'old-10'),
      logEntry(11, 'old-11'),
      logEntry(12, 'healed-12'),
      logEntry(13, 'healed-13'),
      logEntry(14, 'healed-14'),
      logEntry(15, 'live-15'),
      logEntry(16, 'healed-16'),
    ]]
    fire('resync', { reason: 'events_outside_replay_window' })
    await flushPromises()

    // 重载发生（第二次拉取）；视图 = 文件全量真相（含洞口 12-14）
    expect(fetchCalls.length).toBe(2)
    const messages = (wrapper.vm as any).entries.map((e: any) => e.message)
    expect(messages).toEqual([
      'old-10', 'old-11', 'healed-12', 'healed-13', 'healed-14', 'live-15', 'healed-16',
    ])

    // 重载后迟到/重放的旧 seq 帧（≤ 历史最大 seq 16）被去重，不重复入列
    fire('log', logEntry(14, 'replayed-14'))
    fire('log', logEntry(17, 'live-17'))
    await flushPromises()
    expect((wrapper.vm as any).entries.length).toBe(8)
    expect((wrapper.vm as any).entries[7].message).toBe('live-17')

    wrapper.unmount()
  })

  it('historyLoaded=false（首载在飞）时 resync 不叠加第二次拉取', async () => {
    historyPages = [[logEntry(1, 'first')]]
    // 首次调用挂起，制造「首载在飞」窗口
    let releaseFirst: () => void = () => {}
    const gate = new Promise<void>((resolve) => { releaseFirst = resolve })
    authedFetchMock.mockImplementationOnce((...args: any[]) => {
      fetchCalls.push(String(args[0]))
      return gate.then(() => ({
        ok: true,
        json: () => Promise.resolve({ entries: historyPages[0] ?? [] }),
      }))
    })

    const wrapper = mount(EventStream)
    // 首载挂起中（historyLoaded=false）：resync 应被守卫跳过
    fire('resync', { reason: 'events_outside_replay_window' })
    releaseFirst()
    await flushPromises()

    // 首载 1 次拉取，resync 未叠加
    expect(fetchCalls.length).toBe(1)
    expect((wrapper.vm as any).entries.length).toBe(1)
    wrapper.unmount()
  })

  it('paused（视图冻结）时 resync 不重灌', async () => {
    historyPages = [[logEntry(10, 'old-10')]]
    const wrapper = mount(EventStream)
    await flushPromises()
    expect(fetchCalls.length).toBe(1)

    // 点「⏸ 暂停」冻结视图
    const pauseBtn = wrapper.findAll('button').find(b => b.text().includes('暂停'))
    expect(pauseBtn).toBeTruthy()
    await pauseBtn!.trigger('click')

    fire('resync', { reason: 'events_outside_replay_window' })
    await flushPromises()
    // 视图冻结：不重拉、不重灌
    expect(fetchCalls.length).toBe(1)
    expect((wrapper.vm as any).entries.length).toBe(1)
    wrapper.unmount()
  })
})
