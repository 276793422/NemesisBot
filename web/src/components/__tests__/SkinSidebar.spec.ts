import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 皮肤骨架槽位：左侧栏（WB 形态）。AppLayout 在皮肤激活时以它替换主导航
// Sidebar——新建对话 + 4 导航 + 更多 flyout（全部管理页）+ 时间分组会话
// 历史 + 空间（本机）+ footer（用户 + 设置齿轮）。

const pushMock = vi.fn()
vi.mock('vue-router', () => ({
  useRouter: () => ({
    push: (...args: any[]) => pushMock(...args),
    currentRoute: { value: { path: '/' } },
    resolve: (path: string) => ({ matched: [{}] }), // 全量路由在列（裁剪过滤用）
  }),
}))

const listMock = vi.fn()
vi.mock('../../composables/useChatApi', async () => {
  const actual: any = await vi.importActual('../../composables/useChatApi')
  return {
    ...actual,
    useChatApi: () => ({
      list: (...args: any[]) => listMock(...args),
      create: vi.fn().mockResolvedValue({ session_id: 's-new', title: '新对话' }),
      delete: vi.fn().mockResolvedValue({}),
      rename: vi.fn().mockResolvedValue({}),
      export: vi.fn().mockResolvedValue({}),
      createBound: vi.fn().mockResolvedValue({ session_id: 's-b', reused: false }),
      projects: { list: vi.fn().mockResolvedValue({ projects: [] }) },
      createProject: vi.fn(),
      removeProject: vi.fn(),
      renameProject: vi.fn(),
    }),
  }
})

vi.mock('../../composables/useToast', () => ({
  useToast: () => ({ error: vi.fn(), success: vi.fn(), info: vi.fn(), warn: vi.fn() }),
}))

import SkinSidebar from '../SkinSidebar.vue'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  pushMock.mockReset()
  listMock.mockReset()
  listMock.mockResolvedValue({ sessions: [] })
})

async function mountSidebar() {
  const w = mount(SkinSidebar, { attachTo: document.body })
  await flushPromises()
  return w
}

describe('SkinSidebar（WB 形态左侧栏骨架槽位）', () => {
  it('渲染新建对话 + 4 导航 + 更多 + 空间 + footer', async () => {
    const w = await mountSidebar()
    expect(w.find('.nb-sb-newtask').text()).toContain('新建对话')
    const navs = w.findAll('.nb-sb-item')
    expect(navs.length).toBe(5) // 4 导航 + 更多
    expect(navs[0].text()).toBe('人格')
    expect(navs[3].text()).toBe('定时任务')
    expect(navs[4].text()).toBe('更多')
    expect(w.text()).toContain('空间')
    expect(w.text()).toContain('本机')
    expect(w.find('.nb-sb-footer').text()).toContain('本机用户')
    w.unmount()
  })

  it('空会话列表 → 「暂无会话」', async () => {
    const w = await mountSidebar()
    expect(w.text()).toContain('暂无会话')
    w.unmount()
  })

  it('会话按时间分组渲染（今天/更早），行=标题+相对时间', async () => {
    const now = Date.now()
    listMock.mockResolvedValue({
      sessions: [
        { id: 'a', channel: 'web', startTime: '', lastTime: new Date(now - 60000).toISOString(), messageCount: 1, firstMessage: '今早的问题' },
        { id: 'b', channel: 'web', startTime: '', lastTime: new Date(now - 40 * 86400000).toISOString(), messageCount: 2, firstMessage: '很久以前的问题', title: '老会话' },
      ],
    })
    const w = await mountSidebar()
    const groups = w.findAll('.nb-sb-group .nb-sb-section')
    expect(groups.map((g) => g.text())).toEqual(expect.arrayContaining(['今天', '更早']))
    const rows = w.findAll('.nb-sb-row-title')
    expect(rows.map((r) => r.text())).toContain('今早的问题')
    expect(rows.map((r) => r.text())).toContain('老会话') // title 优先于 firstMessage
    w.unmount()
  })

  it('点击会话行 → switchTo + 路由回聊天页', async () => {
    listMock.mockResolvedValue({
      sessions: [
        { id: 'a', channel: 'web', startTime: '', lastTime: new Date().toISOString(), messageCount: 1, firstMessage: 'q' },
      ],
    })
    const w = await mountSidebar()
    await w.find('.nb-sb-row').trigger('click')
    expect(useSessionStore().currentId).toBe('a')
    expect(pushMock).toHaveBeenCalledWith('/')
    w.unmount()
  })

  it('新建对话 → store.create + 路由回聊天页', async () => {
    const w = await mountSidebar()
    await w.find('.nb-sb-newtask').trigger('click')
    await flushPromises()
    expect(useSessionStore().currentId).toBe('s-new')
    expect(pushMock).toHaveBeenCalledWith('/')
    w.unmount()
  })

  it('「更多」展开 flyout 列全部管理页，点击跳转并收起', async () => {
    const w = await mountSidebar()
    expect(w.find('.nb-sb-flyout').exists()).toBe(false)
    await w.findAll('.nb-sb-item')[4].trigger('click')
    const flyout = w.find('.nb-sb-flyout')
    expect(flyout.exists()).toBe(true)
    expect(flyout.text()).toContain('模型')
    expect(flyout.text()).toContain('看板')
    await w.find('.nb-sb-flyout-item').trigger('click')
    expect(pushMock).toHaveBeenCalledWith('/overview')
    expect(w.find('.nb-sb-flyout').exists()).toBe(false)
    w.unmount()
  })

  it('flyout 点 wrap 外部收起（wrap 内点击不受 outside-click 影响）', async () => {
    const w = await mountSidebar()
    await w.findAll('.nb-sb-item')[4].trigger('click')
    expect(w.find('.nb-sb-flyout').exists()).toBe(true) // 按钮点击冒泡 document 但在 wrap 内 → 保持展开
    document.body.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await w.vm.$nextTick()
    expect(w.find('.nb-sb-flyout').exists()).toBe(false)
    w.unmount()
  })

  it('footer 齿轮 → 设置页', async () => {
    const w = await mountSidebar()
    await w.find('.nb-sb-gear').trigger('click')
    expect(pushMock).toHaveBeenCalledWith('/settings')
    w.unmount()
  })

  it('删除会话：confirm 确认后调 remove 并从列表消失，取消则不动', async () => {
    listMock.mockResolvedValue({
      sessions: [
        { id: 'a', channel: 'web', startTime: '', lastTime: new Date().toISOString(), messageCount: 1, firstMessage: 'q' },
      ],
    })
    const confirmSpy = vi.spyOn(window, 'confirm')
    const w = await mountSidebar()

    confirmSpy.mockReturnValue(false) // 取消
    await w.find('.nb-sb-row-del').trigger('click')
    await flushPromises()
    expect(useSessionStore().sessions.map((s) => s.id)).toContain('a')

    confirmSpy.mockReturnValue(true) // 确认
    await w.find('.nb-sb-row-del').trigger('click')
    await flushPromises()
    expect(useSessionStore().sessions.map((s) => s.id)).not.toContain('a')
    confirmSpy.mockRestore()
    w.unmount()
  })
})
