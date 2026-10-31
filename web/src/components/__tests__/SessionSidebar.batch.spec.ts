import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'
import { useToast } from '../../composables/useToast'

// 批量管理（2026-10-30 会话回收站）dispatch 级 spec：
// - 「☑ 批量」进入多选模式：底部浮动工具条出现、行首勾选框可见、
//   「⋯」行菜单按钮隐藏；
// - 批量模式下行点击 = 勾选切换（不切换当前会话 currentId）；
// - 对话区「全选」/ 组头「全选本组」（含孤儿组）勾满对应行；
// - 删除所选(N) → confirm → 逐条顺序调 sessions.delete（默认移入回收站；
//   拍板③：批量删除 = 批量进回收站）→ 成功 toast + 强制刷新列表 + 退出
//   批量模式；单条失败不中断批次，汇总 toast 报成功/失败数；
// - 取消：清空选择并退出模式，不发生任何 delete 调用。
// 后端软删/回收站语义由 sessions_trash_tests.rs + trash_tests.rs 钉住。

const listMock = vi.fn()
const deleteMock = vi.fn()
const listProjectsMock = vi.fn()

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn() }),
}))
vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({
    list: (...a: any[]) => listMock(...a),
    delete: (...a: any[]) => deleteMock(...a),
    listProjects: (...a: any[]) => listProjectsMock(...a),
    create: vi.fn(),
    rename: vi.fn(),
    clear: vi.fn(),
    export: vi.fn(),
    turns: vi.fn(),
    fork: vi.fn(),
    detach: vi.fn(),
    snooze: vi.fn(),
    unsnooze: vi.fn(),
    openProjectDir: vi.fn(),
  }),
}))

import SessionSidebar from '../SessionSidebar.vue'
import { useSessionStore } from '../../stores/session'

function sess(id: string, over: Record<string, unknown> = {}) {
  return {
    id,
    channel: 'web',
    startTime: '2026-10-30T01:00:00Z',
    lastTime: '2026-10-30T02:00:00Z',
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
  vi.spyOn(window, 'confirm').mockReturnValue(true)
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

describe('SessionSidebar 批量管理（2026-10-30 会话回收站）', () => {
  it('进入批量模式：工具条出现、勾选框可见、行菜单隐藏；退出后还原', async () => {
    const w = await mountWith([sess('a1'), sess('a2')])

    // 非批量态：无工具条、无勾选框、行菜单按钮在
    expect(w.find('[data-test="batch-toolbar"]').exists()).toBe(false)
    expect(w.find('[data-test="batch-check"]').exists()).toBe(false)
    expect(w.find('.row-more').exists()).toBe(true)

    await w.find('[data-test="batch-toggle"]').trigger('click')
    expect(w.find('[data-test="batch-toolbar"]').exists()).toBe(true)
    expect(w.findAll('[data-test="batch-check"]').length).toBe(2)
    expect(w.find('.row-more').exists()).toBe(false)
    // 对话区标题条出现「全选」入口
    expect(w.find('[data-test="batch-select-chat"]').exists()).toBe(true)

    // 取消 → 退出模式，工具条消失
    await w.find('[data-test="batch-cancel"]').trigger('click')
    expect(w.find('[data-test="batch-toolbar"]').exists()).toBe(false)
    expect(w.find('[data-test="batch-check"]').exists()).toBe(false)
    w.unmount()
  })

  it('批量模式下行点击=勾选切换，不切换当前会话；再点取消勾选', async () => {
    const w = await mountWith([sess('a1'), sess('a2')])
    const store = useSessionStore()

    await w.find('[data-test="batch-toggle"]').trigger('click')
    const row = w.find('.session-item[data-session-id="a1"]')
    await row.trigger('click')

    // 勾选高亮 + ☑，且 currentId 未被切换
    expect(row.classes()).toContain('picked')
    expect(row.text()).toContain('☑')
    expect(store.currentId).toBeNull()

    // 再点一次 → 取消勾选
    await w.find('.session-item[data-session-id="a1"]').trigger('click')
    expect(w.find('.session-item[data-session-id="a1"]').classes()).not.toContain('picked')
    expect(w.find('.session-item[data-session-id="a1"]').text()).toContain('☐')
    w.unmount()
  })

  it('对话区「全选」勾满对话组；组头「全选本组」只勾本组（含孤儿组入口）', async () => {
    const w = await mountWith(
      [sess('a1'), sess('a2'), sess('o1', { projectId: 'gone-p' })],
      [{ id: 'p1', name: 'MyProj', path: 'C:\\x', running: true }],
    )
    // o1 的 projectId 不在注册表 → 孤儿组（批量模式下也有全选入口）
    await w.find('[data-test="batch-toggle"]').trigger('click')

    // 对话区全选：只勾对话组的 2 条
    await w.find('[data-test="batch-select-chat"]').trigger('click')
    expect(w.find('.session-item[data-session-id="a1"]').classes()).toContain('picked')
    expect(w.find('.session-item[data-session-id="a2"]').classes()).toContain('picked')
    expect(w.find('.session-item[data-session-id="o1"]').classes()).not.toContain('picked')

    // 孤儿组组头「全选本组」：勾上 o1（工具条计数 3）。注册表里的空项目组
    // （MyProj）在批量模式同样渲染全选入口——共 2 个组头入口，取孤儿组那个。
    const groupSelects = w.findAll('[data-test="batch-select-group"]')
    expect(groupSelects.length).toBe(2)
    const orphanSelect = w.find('.group-header.orphan [data-test="batch-select-group"]')
    expect(orphanSelect.exists()).toBe(true)
    await orphanSelect.trigger('click')
    expect(w.find('.session-item[data-session-id="o1"]').classes()).toContain('picked')
    expect(w.find('[data-test="batch-delete"]').text()).toContain('删除所选(3)')

    // 工具条「全选」幂等
    await w.find('[data-test="batch-select-all"]').trigger('click')
    expect(w.find('[data-test="batch-delete"]').text()).toContain('删除所选(3)')
    w.unmount()
  })

  it('删除所选(N)：confirm → 逐条顺序 delete → 成功 toast + 强制刷新 + 退出批量模式', async () => {
    // 刷新后的列表不再含被删行（mount 1 次两行，之后只有 a2）。
    // 不走 mountWith——它会覆盖持久 mock；此处内联挂载。
    listMock.mockReset()
      .mockResolvedValueOnce({ sessions: [sess('a1'), sess('a2')] })
      .mockResolvedValue({ sessions: [sess('a2')] })
    listProjectsMock.mockResolvedValue({ projects: [] })
    const w = mount(SessionSidebar)
    await flushPromises()
    const store = useSessionStore()
    expect(w.find('.session-item[data-session-id="a1"]').exists()).toBe(true)

    await w.find('[data-test="batch-toggle"]').trigger('click')
    await w.find('.session-item[data-session-id="a1"]').trigger('click')
    await w.find('.session-item[data-session-id="a2"]').trigger('click')

    const delBtn = w.find('[data-test="batch-delete"]')
    expect(delBtn.text()).toContain('删除所选(2)')
    await delBtn.trigger('click')
    await flushPromises()

    // 确认文案 = 拍板③ 口径（移入回收站 + 90 天 + 回收站入口指路）
    expect(window.confirm).toHaveBeenCalledTimes(1)
    const msg = vi.mocked(window.confirm).mock.calls[0][0] as string
    expect(msg).toContain('2 个会话移入回收站')
    expect(msg).toContain('回收站')

    // 逐条顺序调用（无批量 WSAPI）；第三参 source='batch'（2026-10-30
    // 浏览器 E2E T4 根修：回收站来源徽标「批量删除」的依据）。
    expect(deleteMock).toHaveBeenCalledTimes(2)
    expect(deleteMock).toHaveBeenNthCalledWith(1, 'a1', undefined, 'batch')
    expect(deleteMock).toHaveBeenNthCalledWith(2, 'a2', undefined, 'batch')

    // 批次完成：退出批量模式 + 强制刷新列表（mount 1 次 + force 1 次）
    expect(w.find('[data-test="batch-toolbar"]').exists()).toBe(false)
    expect(listMock.mock.calls.length).toBeGreaterThanOrEqual(2)
    // 刷新后的列表已不含被删行（UI 与 store 同步收敛到 a2）
    expect(w.find('.session-item[data-session-id="a1"]').exists()).toBe(false)
    expect(store.sessions.find(s => s.id === 'a2')).toBeTruthy()

    const toasts = useToast().toasts
    expect(toasts.some(t => t.type === 'success' && t.message.includes('已将 2 个会话移入回收站'))).toBe(true)
    w.unmount()
  })

  it('confirm 拒绝 → 不发生任何 delete 调用，选择保留', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false)
    const w = await mountWith([sess('a1'), sess('a2')])

    await w.find('[data-test="batch-toggle"]').trigger('click')
    await w.find('.session-item[data-session-id="a1"]').trigger('click')
    await w.find('[data-test="batch-delete"]').trigger('click')
    await flushPromises()

    expect(deleteMock).not.toHaveBeenCalled()
    // 未退出批量模式、选择未清
    expect(w.find('[data-test="batch-toolbar"]').exists()).toBe(true)
    expect(w.find('.session-item[data-session-id="a1"]').classes()).toContain('picked')
    w.unmount()
  })

  it('单条失败不中断批次：汇总 toast 报成功/失败数（失败 id 列出）', async () => {
    deleteMock.mockImplementation((id: string) =>
      id === 'b1' ? Promise.reject('backend boom') : Promise.resolve({ deleted: id, trashed: true }),
    )
    const w = await mountWith([sess('a1'), sess('b1')])

    await w.find('[data-test="batch-toggle"]').trigger('click')
    await w.find('.session-item[data-session-id="a1"]').trigger('click')
    await w.find('.session-item[data-session-id="b1"]').trigger('click')
    await w.find('[data-test="batch-delete"]').trigger('click')
    await flushPromises()

    // 两条都被尝试（失败不中断）
    expect(deleteMock).toHaveBeenCalledTimes(2)
    const toasts = useToast().toasts
    const err = toasts.find(t => t.type === 'error')
    expect(err).toBeTruthy()
    expect(err!.message).toContain('成功 1 个')
    expect(err!.message).toContain('失败 1 个')
    expect(err!.message).toContain('b1')
    w.unmount()
  })

  it('取消：清空选择并退出模式，零 delete 调用；重进批量模式选择已重置', async () => {
    const w = await mountWith([sess('a1'), sess('a2')])

    await w.find('[data-test="batch-toggle"]').trigger('click')
    await w.find('.session-item[data-session-id="a1"]').trigger('click')
    await w.find('[data-test="batch-cancel"]').trigger('click')

    expect(deleteMock).not.toHaveBeenCalled()
    expect(w.find('[data-test="batch-toolbar"]').exists()).toBe(false)

    // 重进批量模式：旧选择不残留
    await w.find('[data-test="batch-toggle"]').trigger('click')
    expect(w.find('.session-item[data-session-id="a1"]').classes()).not.toContain('picked')
    expect(w.find('[data-test="batch-delete"]').text()).toContain('删除所选(0)')
    expect((w.find('[data-test="batch-delete"]').element as HTMLButtonElement).disabled).toBe(true)
    w.unmount()
  })
})
