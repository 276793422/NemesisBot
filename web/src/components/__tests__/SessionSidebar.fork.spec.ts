import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// E4（2026-09-05）：SessionSidebar fork 血缘标记——sessions.list 回填的
// parent/parentTitle/forkedAtTurn 渲染「分叉自「{标题}」· 第 N 轮」；父会话
// 在列表里可点击跳转（switchTo），不在（已删/异源）则纯文本不误导。

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
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  listMock.mockReset()
})

async function mountWith(sessions: any[]) {
  listMock.mockResolvedValue({ sessions })
  const w = mount(SessionSidebar)
  await flushPromises()
  return w
}

describe('SessionSidebar E4 fork 血缘标记', () => {
  it('渲染血缘标记（父标题 + 轮次），父在列表可点击跳转父会话', async () => {
    const w = await mountWith([
      { id: 'p1', firstMessage: '父会话' },
      { id: 'f1', firstMessage: '子会话', parent: 'agent:main:session:p1', parentTitle: '父会话', forkedAtTurn: 3 },
    ])
    const store = useSessionStore()

    const line = w.find('.session-fork-line')
    expect(line.exists()).toBe(true)
    expect(line.text()).toContain('分叉自「父会话」')
    expect(line.text()).toContain('第 3 轮')
    expect(line.classes()).toContain('link')

    // 点击血缘标记跳转到父会话
    await line.trigger('click')
    expect(store.currentId).toBe('p1')
    w.unmount()
  })

  it('父不在列表 → 无 link 类、点击不导航；无 parent 的条目不渲染标记', async () => {
    const w = await mountWith([
      { id: 'gone', firstMessage: '已被删除的父会话' },
      { id: 'orphan', firstMessage: '孤儿 fork', parent: 'agent:main:session:gone2', forkedAtTurn: 2 },
      { id: 'plain', firstMessage: '普通会话' },
    ])
    const store = useSessionStore()

    const lines = w.findAll('.session-fork-line')
    // 只有带 parent 的条目渲染标记
    expect(lines.length).toBe(1)
    // 无 parentTitle → 回退显示 parent key 的 sid 段。
    expect(lines[0].text()).toContain('分叉自「gone2」')
    expect(lines[0].text()).toContain('第 2 轮')
    expect(lines[0].classes()).not.toContain('link')

    // 父会话不在列表 → 点击不导航
    await lines[0].trigger('click')
    expect(store.currentId).toBeNull()
    w.unmount()
  })
})
