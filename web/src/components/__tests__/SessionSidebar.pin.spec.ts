import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// M6（2026-09-07）：会话 pin——纯前端。pinned 集合存 localStorage
// （nb_pinned_sessions），pinned 会话置顶显示（组内保持原序）；删除会话
// 顺带清 pin 防残留 id 永久占顶。2026-09-08 改版：入口从行内常驻 📌 按钮
// 收进 hover「⋯」行菜单（置顶/取消置顶同一条目切换文案）。

const deleteMock = vi.fn().mockResolvedValue({})
const routerPushMock = vi.fn()
vi.mock('vue-router', () => ({
  useRouter: () => ({ push: (...a: any[]) => routerPushMock(...a) }),
}))
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

/** 打开第 idx 行的行菜单。 */
async function openRowMenu(w: any, idx: number) {
  await w.findAll('.session-item')[idx].find('.row-more').trigger('click')
}

/** 在（当前打开的）行菜单里点指定文案的条目。 */
async function clickMenuItem(w: any, text: string) {
  const item = w
    .findAll('.row-menu .menu-item')
    .find((b: any) => b.text().includes(text))
  expect(item, `menu item "${text}" must exist`).toBeTruthy()
  await item.trigger('click')
}

describe('SessionSidebar pin（行菜单入口）', () => {
  it('行菜单「置顶」→ 该会话置顶 + pin 标记 + localStorage 持久化；再点「取消置顶」还原', async () => {
    const w = await mountWith([
      { id: 'a', firstMessage: '会话 A' },
      { id: 'b', firstMessage: '会话 B' },
      { id: 'c', firstMessage: '会话 C' },
    ])
    expect(titles(w)).toEqual(['会话 A', '会话 B', '会话 C'])

    // 置顶第三个。
    await openRowMenu(w, 2)
    await clickMenuItem(w, '置顶')
    expect(titles(w)).toEqual(['会话 C', '会话 A', '会话 B'])
    expect(JSON.parse(localStorage.getItem(PIN_KEY)!)).toEqual(['c'])
    // pinned 行有标题前缀标记；行菜单条目反转为「取消置顶」。
    expect(w.find('.session-item .pin-flag').exists()).toBe(true)

    await openRowMenu(w, 0)
    await clickMenuItem(w, '取消置顶')
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

  it('行菜单「置顶」不切换当前会话（菜单容器 @click.stop 阻断冒泡）', async () => {
    const w = await mountWith([
      { id: 'a', firstMessage: '会话 A' },
      { id: 'b', firstMessage: '会话 B' },
    ])
    await openRowMenu(w, 1)
    await clickMenuItem(w, '置顶')
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

    // 行菜单 → 删除会话。
    await openRowMenu(w, 0)
    await clickMenuItem(w, '删除会话')
    await flushPromises()
    expect(deleteMock).toHaveBeenCalledWith('a')
    expect(JSON.parse(localStorage.getItem(PIN_KEY)!)).toEqual([])
    vi.unstubAllGlobals()
    w.unmount()
  })
})
