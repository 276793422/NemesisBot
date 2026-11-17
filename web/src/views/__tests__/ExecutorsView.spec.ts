// ExecutorsView 复合页测试（2026-11-15 已归档 TAB 扩展）：三 TAB 渲染 /
// TAB 状态纯 route query 派生（单一真相源 = URL）/ 已归档 TAB 挂 archived
// 形态 SubagentsView / 面板 emit('archived-count') 投影 TAB 徽标。
// 子组件 WSAPI / WebSocket / toast / vue-router 全 mock，不碰真实后端。
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: requestMock, connected: { value: true } }),
}))
const wsMocks = vi.hoisted(() => ({
  addMessageHandler: vi.fn(),
  removeMessageHandler: vi.fn(),
}))
vi.mock('../../composables/useWebSocket', () => ({
  addMessageHandler: wsMocks.addMessageHandler,
  removeMessageHandler: wsMocks.removeMessageHandler,
  useWebSocket: () => ({
    addMessageHandler: wsMocks.addMessageHandler,
    removeMessageHandler: wsMocks.removeMessageHandler,
    connect: vi.fn(),
    sendMessage: vi.fn(),
  }),
}))
const toastMocks = vi.hoisted(() => ({
  success: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
}))
vi.mock('../../composables/useToast', () => ({
  useToast: () => toastMocks,
}))
const routerMocks = vi.hoisted(() => ({
  replace: vi.fn(),
  push: vi.fn(),
}))
let queryMock: Record<string, unknown> = {}
vi.mock('vue-router', () => ({
  useRoute: () => ({ query: queryMock }),
  useRouter: () => ({ replace: routerMocks.replace, push: routerMocks.push }),
}))
vi.mock('../../components/process/ProcessOutputTail.vue', () => ({
  default: { template: '<div class="stub-tail" />', props: ['jobId', 'mode'] },
}))

import ExecutorsView from '../ExecutorsView.vue'
import SubagentsView from '../SubagentsView.vue'
import ProcessesView from '../ProcessesView.vue'
import LoopsView from '../LoopsView.vue'

beforeEach(() => {
  queryMock = {}
  requestMock.mockReset()
  routerMocks.replace.mockReset()
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'list') return Promise.resolve({ tasks: [], jobs: [] })
    if (cmd === 'events') return Promise.resolve({ events: [], running: false })
    return Promise.resolve({})
  })
})

describe('ExecutorsView 多 TAB', () => {
  it('TAB 序：子代理/后台任务/执行循环/已归档…；缺省挂子代理工作面（无 archived prop）；加载后计数投影 0', async () => {
    const w = mount(ExecutorsView)
    await flushPromises()
    const tabs = w.findAll('.tab').map((t) => t.text().trim())
    expect(tabs.slice(0, 2)).toEqual(['子代理', '后台任务'])
    expect(tabs[2]).toBe('执行循环')
    const sv = w.findComponent(SubagentsView)
    expect(sv.exists()).toBe(true)
    // Vue Boolean prop 缺省归一为 false（isArchived 判 === true，行为一致）。
    expect(sv.props('archived')).toBe(false)
    expect(w.findComponent(ProcessesView).exists()).toBe(false)
    expect(w.findComponent(LoopsView).exists()).toBe(false)
    // 缺省 list 响应无 archived_count → 投影 0 → 括号渲染 (0)。
    expect(tabs[3]).toBe('已归档 (0)')
    w.unmount()
  })

  it('直链后台任务 TAB：无面板加载 → 计数保持 null（不渲染括号）', async () => {
    queryMock = { tab: 'processes' }
    const w = mount(ExecutorsView)
    await flushPromises()
    expect(w.findAll('.tab')[3].text().trim()).toBe('已归档')
    w.unmount()
  })

  it('route query 派生：tab=processes 挂后台任务面板', async () => {
    queryMock = { tab: 'processes' }
    const w = mount(ExecutorsView)
    await flushPromises()
    expect(w.findComponent(ProcessesView).exists()).toBe(true)
    expect(w.findComponent(SubagentsView).exists()).toBe(false)
    w.unmount()
  })

  it('route query 派生：tab=loops 挂执行循环面板（异步组件 flush 后解析；loops.list 响应 {} → 空态）', async () => {
    queryMock = { tab: 'loops' }
    const w = mount(ExecutorsView)
    await flushPromises()
    expect(w.findComponent(LoopsView).exists()).toBe(true)
    expect(w.findComponent(SubagentsView).exists()).toBe(false)
    expect(w.find('.page-loops').exists()).toBe(true)
    expect(w.find('[data-testid="loops-empty"]').exists()).toBe(true)
    w.unmount()
  })

  it('route query 派生：tab=archived 挂归档形态 SubagentsView（archived=true）', async () => {
    queryMock = { tab: 'archived' }
    const w = mount(ExecutorsView)
    await flushPromises()
    const sv = w.findComponent(SubagentsView)
    expect(sv.exists()).toBe(true)
    expect(sv.props('archived')).toBe(true)
    expect(w.findComponent(ProcessesView).exists()).toBe(false)
    w.unmount()
  })

  it('点 TAB 走 router.replace（保留其余 query，不产生历史栈）', async () => {
    queryMock = { tab: 'subagents', keep: '1' }
    const w = mount(ExecutorsView)
    await flushPromises()
    await w.findAll('.tab')[3].trigger('click')
    expect(routerMocks.replace).toHaveBeenCalledTimes(1)
    expect(routerMocks.replace).toHaveBeenCalledWith({ query: { tab: 'archived', keep: '1' } })
    w.unmount()
  })

  it('面板 emit archived-count → 已归档 TAB 渲染计数括号', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [], archived_count: 7 })
      return Promise.resolve({})
    })
    const w = mount(ExecutorsView)
    await flushPromises()
    expect(w.findAll('.tab')[3].text()).toContain('(7)')
    w.unmount()
  })

  it('点执行循环 TAB → replace 落 tab=loops（query 派生挂面板）', async () => {
    queryMock = { tab: 'subagents' }
    const w = mount(ExecutorsView)
    await flushPromises()
    await w.findAll('.tab')[2].trigger('click')
    expect(routerMocks.replace).toHaveBeenCalledWith({ query: { tab: 'loops' } })
    w.unmount()
  })
})
