import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// M6（2026-09-07）：会话 pin 快速槽——纯前端。pinned 集合存 localStorage
// （nb_pinned_sessions），pinned 会话置顶显示（组内保持原序）；删除会话
// 顺带清 pin 防残留 id 永久占顶。

const deleteMock = vi.fn().mockResolvedValue({})
vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({
    list: (...a: any[]) => listMock(...a),
    create: vi.fn(),
    rename: vi.fn(),
    clear: vi.fn(),
    export: vi.fn(),
    delete: (...a: any[]) => deleteMock(...a),
    turns: vi.fn(),
    fork: vi.fn(),
  }),
}))

import SessionSidebar from '../SessionSidebar.vue'

const PIN_KEY = 'nb_pinned_sessions'
const listMock = vi.fn()

beforeEach(() => {
  setActivePinia(createPinia())
  listMock.mockReset()
  deleteMock.mockClear()
  localStorage.clear()
})

async function mountWith(sessions: any[]) {
  listMock.mockResolvedValue({ sessions })
  const w = mount(SessionSidebar)
  await flushPromises()
  return w
}

function titles(w: any): string[] {
  // pinned 行标题带 📌 前缀标记，剥掉后比对纯标题。
  return w.findAll('.session-item .session-title').map((t: any) => t.text().replace('📌', '').trim())
}

describe('SessionSidebar pin 快速槽 (M6)', () => {
  it('点击 📌 → 该会话置顶 + pin 标记 + localStorage 持久化；再点取消', async () => {
    const w = await mountWith([
      { id: 'a', firstMessage: '会话 A' },
      { id: 'b', firstMessage: '会话 B' },
      { id: 'c', firstMessage: '会话 C' },
    ])
    expect(titles(w)).toEqual(['会话 A', '会话 B', '会话 C'])

    // 置顶第三个。
    await w.findAll('.pin-btn')[2].trigger('click')
    expect(titles(w)).toEqual(['会话 C', '会话 A', '会话 B'])
    expect(JSON.parse(localStorage.getItem(PIN_KEY)!)).toEqual(['c'])
    // pinned 行有标题前缀标记 + 按钮 pinned 态。
    expect(w.find('.session-item .pin-flag').exists()).toBe(true)
    expect(w.findAll('.pin-btn')[0].classes()).toContain('pinned')

    // 再点取消 → 顺序恢复。
    await w.findAll('.pin-btn')[0].trigger('click')
    expect(titles(w)).toEqual(['会话 A', '会话 B', '会话 C'])
    expect(JSON.parse(localStorage.getItem(PIN_KEY)!)).toEqual([])
    w.unmount()
  })

  it('localStorage 预置 pin → 挂载即置顶（跨会话持久）', async () => {
    localStorage.setItem(PIN_KEY, JSON.stringify(['b']))
    const w = await mountWith([
      { id: 'a', firstMessage: '会话 A' },
      { id: 'b', firstMessage: '会话 B' },
    ])
    expect(titles(w)).toEqual(['会话 B', '会话 A'])
    w.unmount()
  })

  it('置顶不影响会话选择与删除按钮事件冒泡（stopPropagation）', async () => {
    const w = await mountWith([
      { id: 'a', firstMessage: '会话 A' },
      { id: 'b', firstMessage: '会话 B' },
    ])
    // 点击 pin 不切换当前会话（stopPropagation）。
    await w.findAll('.pin-btn')[1].trigger('click')
    const { useSessionStore } = await import('../../stores/session')
    expect(useSessionStore().currentId).toBeNull()
    w.unmount()
  })

  it('删除置顶会话 → pin 一并清除（防残留 id 永久占顶）', async () => {
    vi.stubGlobal('confirm', () => true)
    localStorage.setItem(PIN_KEY, JSON.stringify(['a']))
    const w = await mountWith([
      { id: 'a', firstMessage: '会话 A' },
      { id: 'b', firstMessage: '会话 B' },
    ])
    expect(titles(w)).toEqual(['会话 A', '会话 B'])

    // 删掉置顶的 A（最后一个 del-btn 是删除按钮 ×）。
    const delBtns = w.findAll('.session-item')[0].findAll('.del-btn')
    await delBtns[delBtns.length - 1].trigger('click')
    await flushPromises()
    expect(deleteMock).toHaveBeenCalledWith('a')
    expect(JSON.parse(localStorage.getItem(PIN_KEY)!)).toEqual([])
    vi.unstubAllGlobals()
    w.unmount()
  })
})
