import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { nextTick } from 'vue'

// B（2026-09-23 会话绑定注册表）配套：fetchList 在飞共享 Promise。
// 旧实现「listLoading 时直接 return」让后到的强制刷新被静默吞掉——
// agent-gen 建会话后的刷新撞上前一轮在飞，乐观行被在飞响应落盘整表
// 替换冲掉，下轮判「不在列表」→ 重复新建。共享后并发调用方 await 同
// 一次拉取，谁都不少拿结果。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  // initWSAPI：useWebSocket 模块加载时回注 sendRaw（auth store 引链触发），
  // 缺导出会让本 spec 在收集期就炸。
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import { useSessionStore } from '../../stores/session'

function row(id: string): any {
  return { id, channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: id, model: '' }
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
})

describe('session store fetchList 在飞共享', () => {
  it('并发多次调用只发一次请求，全部拿到同一份结果', async () => {
    let resolveList!: (v: any) => void
    requestMock.mockImplementation(() => new Promise((res) => { resolveList = res }))

    const store = useSessionStore()
    const p1 = store.fetchList(true)
    const p2 = store.fetchList(true)
    const p3 = store.fetchList(true)
    resolveList({ sessions: [row('a'), row('b')] })
    await Promise.all([p1, p2, p3])
    await nextTick()

    expect(requestMock).toHaveBeenCalledTimes(1)
    expect(store.sessions.map(s => s.id)).toEqual(['a', 'b'])
  })

  it('成功后 5s 缓存内非强制调用不再发请求；强制调用照样重发', async () => {
    requestMock.mockResolvedValue({ sessions: [row('a')] })
    const store = useSessionStore()

    await store.fetchList(true)
    await store.fetchList() // 缓存命中
    expect(requestMock).toHaveBeenCalledTimes(1)

    await store.fetchList(true) // 强制 → 重发
    expect(requestMock).toHaveBeenCalledTimes(2)
  })

  it('在飞结束后共享解除：新一轮调用发起新请求', async () => {
    let resolveList!: (v: any) => void
    requestMock.mockImplementationOnce(() => new Promise((res) => { resolveList = res }))

    const store = useSessionStore()
    const p1 = store.fetchList(true)
    resolveList({ sessions: [row('a')] })
    await p1

    requestMock.mockResolvedValueOnce({ sessions: [row('a'), row('b')] })
    await store.fetchList(true)
    expect(requestMock).toHaveBeenCalledTimes(2)
    expect(store.sessions.map(s => s.id)).toEqual(['a', 'b'])
  })
})
