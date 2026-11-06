import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'
import { useToast } from '../../composables/useToast'

// BUG-20261106-029 dispatch 级 spec：
// - 变更对比弹窗（openChanges）在飞围栏：stale 导出响应/失败不得写别人
//   （或新开）的弹窗——关 A 开 B 后 A 晚到不得覆盖 B 的 file_changes、
//   A 晚败不得把正在看的 B 弹窗强制关掉（同族先例 BUG-20261106-006
//   SubagentsView.openDetail，本位点是该族扫描漏网）；
// - 会话行操作失败反馈：删除失败 → error toast 且 pin 不丢（旧实现在
//   await 前就 unpin）；重命名/清空失败 → error toast（store 改为返回
//   成败布尔，调用方出声——store 注释「caller may surface a toast」的
//   契约兑现）。
// 后端导出/删除语义由 sessions_trash_tests.rs 等钉住，此处只钉前端围栏
// 与反馈链。

const listMock = vi.fn()
const deleteMock = vi.fn()
const listProjectsMock = vi.fn()
const exportMock = vi.fn()
const renameMock = vi.fn()
const clearMock = vi.fn()

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn() }),
}))
vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({
    list: (...a: any[]) => listMock(...a),
    delete: (...a: any[]) => deleteMock(...a),
    listProjects: (...a: any[]) => listProjectsMock(...a),
    create: vi.fn(),
    rename: (...a: any[]) => renameMock(...a),
    clear: (...a: any[]) => clearMock(...a),
    export: (...a: any[]) => exportMock(...a),
    turns: vi.fn(),
    fork: vi.fn(),
    detach: vi.fn(),
    snooze: vi.fn(),
    unsnooze: vi.fn(),
    openProjectDir: vi.fn(),
    importSession: vi.fn(),
  }),
}))

import SessionSidebar from '../SessionSidebar.vue'
import SessionChangesPanel from '../logs/SessionChangesPanel.vue'
import { useSessionStore } from '../../stores/session'

function sess(id: string, over: Record<string, unknown> = {}) {
  return {
    id,
    channel: 'web',
    startTime: '2026-11-06T01:00:00Z',
    lastTime: '2026-11-06T02:00:00Z',
    messageCount: 3,
    firstMessage: `会话 ${id}`,
    model: '',
    ...over,
  }
}

beforeEach(() => {
  setActivePinia(createPinia())
  listMock.mockReset().mockResolvedValue({ sessions: [] })
  deleteMock.mockReset().mockResolvedValue({ deleted: 'x', trashed: true })
  listProjectsMock.mockReset().mockResolvedValue({ projects: [] })
  exportMock.mockReset().mockResolvedValue({ messages: [] })
  renameMock.mockReset().mockResolvedValue({})
  clearMock.mockReset().mockResolvedValue({})
  vi.spyOn(window, 'confirm').mockReturnValue(true)
  vi.spyOn(window, 'prompt').mockReturnValue('改名')
  useToast().toasts.splice(0)
  localStorage.clear()
})

async function mountWith(sessions: any[], projects: any[] = []) {
  listMock.mockResolvedValue({ sessions })
  listProjectsMock.mockResolvedValue({ projects })
  const w = mount(SessionSidebar)
  await flushPromises()
  return w
}

/** 打开指定会话的「变更对比」弹窗（行菜单 → 菜单项）。 */
async function openChangesFor(w: any, id: string) {
  const row = w.find(`.session-item[data-session-id="${id}"]`)
  await row.find('.row-more').trigger('click')
  const item = w.findAll('.row-menu .menu-item').find((b: any) => b.text().includes('变更对比'))!
  await item.trigger('click')
  await flushPromises()
}

function deferred<T>() {
  let resolve!: (v: T) => void
  let reject!: (e: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

describe('SessionSidebar 变更对比在飞围栏（BUG-20261106-029）', () => {
  it('stale 成功响应不得覆盖新弹窗：关 A 开 B 后 A 晚到，B 的 messages 不被冲掉', async () => {
    const defs: Record<string, ReturnType<typeof deferred<{ messages: any[] }>>> = {}
    exportMock.mockImplementation((sid: string) => {
      defs[sid] = deferred<{ messages: any[] }>()
      return defs[sid].promise
    })
    const w = await mountWith([sess('a1'), sess('b1')])

    await openChangesFor(w, 'a1')
    expect(w.find('.pr-box').exists()).toBe(true)
    // 关掉 A 的弹窗（在飞请求继续存活）
    await w.find('.pr-box .info-close').trigger('click')
    expect(w.find('.pr-box').exists()).toBe(false)

    // 开 B，正常完成
    await openChangesFor(w, 'b1')
    const bMessages = [
      { role: 'assistant', content: 'B 的变更', file_changes: [{ path: 'b.txt', kind: 'write' }] },
    ]
    defs['b1'].resolve({ messages: bMessages })
    await flushPromises()
    expect(w.findComponent(SessionChangesPanel).props('messages')).toEqual(bMessages)

    // A 的 stale 响应晚到——必须被围栏丢弃，B 的数据原样
    defs['a1'].resolve({
      messages: [
        { role: 'assistant', content: 'A 的变更', file_changes: [{ path: 'a.txt', kind: 'write' }] },
      ],
    })
    await flushPromises()
    expect(w.find('.pr-box').exists()).toBe(true)
    expect(w.findComponent(SessionChangesPanel).props('messages')).toEqual(bMessages)
  })

  it('stale 失败不得关掉新弹窗：关 A 开 B 后 A 晚败，B 弹窗保留、无误导 toast', async () => {
    const defs: Record<string, ReturnType<typeof deferred<{ messages: any[] }>>> = {}
    exportMock.mockImplementation((sid: string) => {
      defs[sid] = deferred<{ messages: any[] }>()
      return defs[sid].promise
    })
    const w = await mountWith([sess('a1'), sess('b1')])

    await openChangesFor(w, 'a1')
    await w.find('.pr-box .info-close').trigger('click')
    await openChangesFor(w, 'b1')
    defs['b1'].resolve({ messages: [] })
    await flushPromises()
    expect(w.find('.pr-box').exists()).toBe(true)

    // A 的 stale 失败晚到——不得关 B 的弹窗、不得出误导错误
    defs['a1'].reject(new Error('stale failure'))
    await flushPromises()
    expect(w.find('.pr-box').exists()).toBe(true)
    expect(useToast().toasts.filter(t => t.type === 'error').length).toBe(0)
  })
})

describe('SessionSidebar 行操作失败反馈（BUG-20261106-029 横向）', () => {
  it('删除失败：error toast 且 pin 不丢、会话行保留；成功后 pin 才清', async () => {
    const w = await mountWith([sess('a1')])
    const menuItems = async (w: any) => {
      await w.find('.session-item[data-session-id="a1"] .row-more').trigger('click')
      return w.findAll('.row-menu .menu-item')
    }

    // 先置顶
    let items = await menuItems(w)
    await items.find((b: any) => b.text().includes('置顶'))!.trigger('click')
    await flushPromises()
    expect(JSON.parse(localStorage.getItem('nb_pinned_sessions') || '[]')).toContain('a1')

    // 删除失败：store.remove 吞错返回 false → toast + pin 保留
    deleteMock.mockRejectedValueOnce(new Error('backend down'))
    items = await menuItems(w)
    await items.find((b: any) => b.text().includes('删除会话'))!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('删除会话失败'))).toBe(true)
    expect(JSON.parse(localStorage.getItem('nb_pinned_sessions') || '[]')).toContain('a1')
    expect(w.find('.session-item[data-session-id="a1"]').exists()).toBe(true)

    // 删除成功：pin 才清
    items = await menuItems(w)
    await items.find((b: any) => b.text().includes('删除会话'))!.trigger('click')
    await flushPromises()
    expect(JSON.parse(localStorage.getItem('nb_pinned_sessions') || '[]')).not.toContain('a1')
    expect(w.find('.session-item[data-session-id="a1"]').exists()).toBe(false)
  })

  it('重命名失败：error toast；成功：无 error 且行标题更新', async () => {
    const w = await mountWith([sess('a1')])
    const openMenu = async () => {
      await w.find('.session-item[data-session-id="a1"] .row-more').trigger('click')
      return w.findAll('.row-menu .menu-item')
    }

    renameMock.mockRejectedValueOnce(new Error('backend down'))
    let items = await openMenu()
    await items.find((b: any) => b.text().includes('重命名'))!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('重命名失败'))).toBe(true)

    // 成功臂前清掉失败臂残留的 toast（error 时长 6s，真实 timer 不跨臂过期）
    useToast().toasts.splice(0)
    items = await openMenu()
    await items.find((b: any) => b.text().includes('重命名'))!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.filter(t => t.type === 'error').length).toBe(0)
    expect(w.find('.session-item[data-session-id="a1"]').text()).toContain('改名')
  })

  it('清空失败：error toast（confirm 通过、api.clear 拒绝）', async () => {
    const w = await mountWith([sess('a1')])
    await w.find('.session-item[data-session-id="a1"] .row-more').trigger('click')
    const items = w.findAll('.row-menu .menu-item')
    clearMock.mockRejectedValueOnce(new Error('backend down'))
    await items.find((b: any) => b.text().includes('清空消息'))!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('清空失败'))).toBe(true)
  })
})
