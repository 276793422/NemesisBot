import { mount } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { ref, reactive, nextTick } from 'vue'

// Sidebar 折叠分组（2026-11-06 导航收敛）行为契约：
// - 管理/配置/其他默认收起（首跑无存储形态），主要等组默认展开；
// - 默认形态不落盘，首次交互（手动点击/导航展开）才持久化；
// - 点组标题切换收起/展开，状态持久化 localStorage（nb_nav_collapsed_groups）；
// - 导航进入被折叠组内页面自动展开（同口径持久化）；
// - 不带 immediate：本组页面上手动收起后刷新不反弹（跨页导航才触发）。
//
// 可见性断言用 v-show 的真实机制（inline style.display）而非 isVisible()：
// jsdom 对未 attach 到 document 的游离 DOM，getComputedStyle 的 display
// 恒返回 none（展开态也返回），isVisible() 在此环境是假信号——本仓库其余
// spec 均不用 isVisible，此为已验证的环境约束。

const routeMock = reactive({ path: '/' })

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn() }),
  useRoute: () => routeMock,
}))

vi.mock('../../composables/useSSE', () => ({ on: vi.fn(), off: vi.fn() }))
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: vi.fn().mockRejectedValue(new Error('ws 未就绪')) }),
}))
vi.mock('../../composables/useToast', () => ({
  useToast: () => ({
    toasts: [],
    info: vi.fn(),
    success: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
    remove: vi.fn(),
  }),
}))
vi.mock('../../stores/app', () => ({
  useAppStore: () => ({
    connected: true,
    sidebarCollapsed: false,
    showMobileSidebar: false,
    toggleSidebar: vi.fn(),
  }),
}))
vi.mock('../../stores/auth', () => ({ useAuthStore: () => ({ logout: vi.fn() }) }))
vi.mock('../../composables/useTheme', () => ({
  useTheme: () => ({ theme: ref('dark'), toggleTheme: vi.fn() }),
}))
vi.mock('../../composables/useEditorMode', () => ({
  useEditorMode: () => ({ fullAccess: ref(false) }),
}))
vi.mock('../../composables/usePanels', () => ({
  usePanels: () => ({ fetchPanels: vi.fn() }),
  panelNavItems: () => [],
}))

vi.mock('../../skins/navModel', () => ({
  visibleNavGroups: () => [
    { title: '主要', items: [{ id: 'chat', label: '聊天', icon: 'x', route: '/' }] },
    { title: '管理', items: [{ id: 'logs', label: '日志', icon: 'x', route: '/logs' }] },
    { title: '配置', items: [{ id: 'settings', label: '设置', icon: 'x', route: '/settings' }] },
    { title: '其他', items: [{ id: 'about', label: '关于', icon: 'x', route: '/about' }] },
  ],
}))

import Sidebar from '../Sidebar.vue'

const KEY = 'nb_nav_collapsed_groups'

function sectionOf(wrapper: ReturnType<typeof mount>, title: string) {
  return wrapper.findAll('.nav-section').find((s) => s.find('.nav-section-toggle').text().includes(title))
}

function itemsVisible(wrapper: ReturnType<typeof mount>, title: string): boolean {
  const sec = sectionOf(wrapper, title)
  if (!sec) return false
  const items = sec.findAll('.nav-item')
  return items.length > 0 && items.every((i) => (i.element as HTMLElement).style.display !== 'none')
}

function stored(): Record<string, boolean> {
  return JSON.parse(localStorage.getItem(KEY) || '{}')
}

beforeEach(() => {
  localStorage.clear()
  routeMock.path = '/'
})

describe('Sidebar 折叠分组', () => {
  it('默认形态：管理/配置/其他收起、主要展开，默认值不落盘', () => {
    const w = mount(Sidebar)
    expect(itemsVisible(w, '主要')).toBe(true)
    expect(itemsVisible(w, '管理')).toBe(false)
    expect(itemsVisible(w, '配置')).toBe(false)
    expect(itemsVisible(w, '其他')).toBe(false)
    expect(stored()).toEqual({})
  })

  it('点组标题切换：展开持久化 false，再收起持久化 true', async () => {
    const w = mount(Sidebar)
    await sectionOf(w, '管理')!.find('.nav-section-toggle').trigger('click')
    expect(itemsVisible(w, '管理')).toBe(true)
    expect(stored().管理).toBe(false)
    await sectionOf(w, '管理')!.find('.nav-section-toggle').trigger('click')
    expect(itemsVisible(w, '管理')).toBe(false)
    expect(stored().管理).toBe(true)
  })

  it('导航进入被折叠组内页面自动展开并持久化', async () => {
    localStorage.setItem(KEY, JSON.stringify({ 管理: true }))
    const w = mount(Sidebar)
    expect(itemsVisible(w, '管理')).toBe(false)
    routeMock.path = '/logs'
    await nextTick()
    await nextTick()
    expect(itemsVisible(w, '管理')).toBe(true)
    expect(stored().管理).toBe(false)
  })

  it('本组页面上刷新不反弹：初始 path 已在组内时保持收起', () => {
    localStorage.setItem(KEY, JSON.stringify({ 管理: true }))
    routeMock.path = '/logs'
    const w = mount(Sidebar)
    expect(itemsVisible(w, '管理')).toBe(false)
  })

  it('存储损坏回落默认形态', () => {
    localStorage.setItem(KEY, 'not-json{{{')
    const w = mount(Sidebar)
    expect(itemsVisible(w, '管理')).toBe(false)
    expect(itemsVisible(w, '主要')).toBe(true)
  })
})
