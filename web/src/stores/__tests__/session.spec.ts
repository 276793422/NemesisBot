import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useToast } from '../../composables/useToast'

// M6 补测（quality-hardening goal 2026-08-25）：session store 批次改动 ——
// remove() 的 paused_cron_jobs 可见化 + 删除后 currentId 回退；
// fetchList 的 5s 缓存与 force 旁路。
// 后端「删除会话级联停用 cron」由 handlers/tests.rs:5465 钉住。

const apiMock = {
  list: vi.fn(),
  create: vi.fn(),
  rename: vi.fn(),
  clear: vi.fn(),
  delete: vi.fn(),
  export: vi.fn(),
}
vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => apiMock,
}))

import { useSessionStore } from '../session'

function entry(id: string): import('../../composables/useChatApi').SessionEntry {
  return { id, channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'm-' + id, model: '' }
}

beforeEach(() => {
  setActivePinia(createPinia())
  apiMock.list.mockReset()
  apiMock.delete.mockReset()
  useToast().toasts.splice(0)
})

describe('session store remove()', () => {
  it('后端停用了绑定 cron → warn toast 点名任务；列表移除', async () => {
    apiMock.delete.mockResolvedValue({
      deleted: 's1',
      paused_cron_jobs: [{ id: 'job-1', name: '日报' }, { id: 'job-2', name: '巡检' }],
    })
    const store = useSessionStore()
    store.sessions = [entry('s1'), entry('s2')]
    await store.remove('s1')

    expect(store.sessions.map(s => s.id)).toEqual(['s2'])
    const t = useToast().toasts.find(t => t.type === 'warn')!
    expect(t.message).toContain('日报、巡检')
    expect(t.message).toContain('可在任务页重新启用')
  })

  it('无绑定 cron → 不发 toast', async () => {
    apiMock.delete.mockResolvedValue({ deleted: 's1', paused_cron_jobs: [] })
    const store = useSessionStore()
    store.sessions = [entry('s1')]
    await store.remove('s1')
    expect(useToast().toasts.length).toBe(0)
  })

  it('删除当前会话 → currentId 回退到剩余首个；删非当前 → currentId 不动；删空 → null', async () => {
    apiMock.delete.mockResolvedValue({ deleted: 'x' })
    const store = useSessionStore()
    store.sessions = [entry('s1'), entry('s2'), entry('s3')]
    store.currentId = 's1'
    await store.remove('s2') // 删非当前
    expect(store.currentId).toBe('s1')

    await store.remove('s1') // 删当前 → 回退剩余首个
    expect(store.currentId).toBe('s3')

    await store.remove('s3') // 删空
    expect(store.currentId).toBe(null)
  })

  it('删除失败 → 列表不动（静默，由调用方 toast）', async () => {
    apiMock.delete.mockRejectedValue(new Error('会话不存在'))
    const store = useSessionStore()
    store.sessions = [entry('s1')]
    await store.remove('s1')
    expect(store.sessions.map(s => s.id)).toEqual(['s1'])
  })
})

describe('session store fetchList()', () => {
  it('5s 缓存：二次调用不发请求；force 旁路', async () => {
    apiMock.list.mockResolvedValue({ sessions: [entry('a')] })
    const store = useSessionStore()
    await store.fetchList()
    await store.fetchList() // 缓存内
    expect(apiMock.list).toHaveBeenCalledTimes(1)

    await store.fetchList(true)
    expect(apiMock.list).toHaveBeenCalledTimes(2)
  })

  it('列表失败 → listError 置位', async () => {
    apiMock.list.mockRejectedValue(new Error('WS 断开'))
    const store = useSessionStore()
    await store.fetchList(true)
    expect(store.listError).toBeTruthy()
  })
})
