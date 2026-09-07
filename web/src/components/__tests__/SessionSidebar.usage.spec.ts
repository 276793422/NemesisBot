import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// M5（2026-09-05）：SessionSidebar 会话用量小字——sessions.list 回填的
// tokens/cost 经 fmtUsageLine 渲染；无记录的条目不占位。

const listMock = vi.fn()
vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({
    list: (...a: any[]) => listMock(...a),
    create: vi.fn(),
    rename: vi.fn(),
    clear: vi.fn(),
    export: vi.fn(),
    delete: vi.fn(),
    turns: vi.fn(),
    fork: vi.fn(),
  }),
}))

import SessionSidebar from '../SessionSidebar.vue'

beforeEach(() => {
  setActivePinia(createPinia())
  listMock.mockReset()
})

describe('SessionSidebar M5 用量小字', () => {
  it('有用量记录的条目渲染 tokens/cost，无记录的不占位', async () => {
    listMock.mockResolvedValue({
      sessions: [
        { id: 's1', firstMessage: '带用量的会话', tokens: 1234, cost: 0.0031 },
        { id: 's2', firstMessage: '无用量会话' },
      ],
    })
    const w = mount(SessionSidebar)
    await flushPromises()

    const lines = w.findAll('.session-usage')
    // 只有带用量的条目渲染小字
    expect(lines.length).toBe(1)
    expect(lines[0].text()).toContain('1.2k tok')
    expect(lines[0].text()).toContain('$0.0031')
    w.unmount()
  })

  it('整表无用量记录 → 任何小字都不渲染', async () => {
    listMock.mockResolvedValue({
      sessions: [{ id: 's1', firstMessage: '干净会话' }],
    })
    const w = mount(SessionSidebar)
    await flushPromises()

    expect(w.findAll('.session-usage').length).toBe(0)
    w.unmount()
  })
})
