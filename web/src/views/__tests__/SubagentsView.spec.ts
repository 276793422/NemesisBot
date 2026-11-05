// SubagentsView 详情请求围栏测试（R575 BUG-20261106-006）：加载期 backdrop
// 尚未显示表格仍可点，两次在飞 openDetail 的响应按完成序覆盖——迟到响应
// 必须被代次围栏整包丢弃。WSAPI / WebSocket / vue-router / toast 全 mock，
// 不碰真实后端。
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'

const requestMock = vi.fn()

const wsMocks = vi.hoisted(() => ({
  addMessageHandler: vi.fn(),
  removeMessageHandler: vi.fn(),
}))
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: requestMock, connected: { value: true } }),
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
vi.mock('vue-router', () => ({
  useRoute: () => ({ query: {} }),
  useRouter: () => ({ replace: routerMocks.replace, push: routerMocks.push }),
}))

import SubagentsView from '../SubagentsView.vue'

function deferred<T>() {
  let resolve!: (v: T) => void
  let reject!: (e: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

function row(id: string, task: string) {
  return {
    id,
    parent_session_key: 'web:sess-1',
    parent_chat_id: 'sess-1',
    origin: 'detached',
    agent_id: 'agent-1',
    task,
    role: '',
    tools_profile: '',
    depth: 0,
    background: false,
    loop_origin: 'main',
    status: 'done',
    started_at_ms: 1,
    running: false,
  }
}

async function mountView() {
  const w = mount(SubagentsView)
  await flushPromises()
  return w
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  wsMocks.addMessageHandler.mockClear()
  wsMocks.removeMessageHandler.mockClear()
  toastMocks.success.mockClear()
  toastMocks.warn.mockClear()
  toastMocks.error.mockClear()
  routerMocks.replace.mockReset()
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'list') return Promise.resolve({ tasks: [] })
    if (cmd === 'events') return Promise.resolve({ events: [], running: false })
    return Promise.resolve({})
  })
})

describe('SubagentsView 详情请求围栏（R575）', () => {
  it('先点慢任务 A 再点快任务 B：A 迟到响应不把详情换回 A', async () => {
    const slowDetail = deferred<any>()
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('a', '慢任务 AAA'), row('b', '快任务 BBB')] })
      if (cmd === 'detail') {
        return data.id === 'a' ? slowDetail.promise : Promise.resolve(row('b', '快任务 BBB'))
      }
      if (cmd === 'events') return Promise.resolve({ events: [], running: false })
      return Promise.resolve({})
    })
    const w = await mountView()
    const rows = w.findAll('.subagent-row')

    // 点 A（detail 请求挂起——backdrop 未显示，表格仍可点）。
    await rows[0].trigger('click')
    // 点 B（detail/events 立即返回，modal 渲染 B）。
    await w.findAll('.subagent-row')[1].trigger('click')
    await flushPromises()
    expect(w.find('.modal-body').text()).toContain('快任务 BBB')
    expect(w.find('.modal-body').text()).not.toContain('慢任务 AAA')

    // A 迟到返回：围栏丢弃，详情仍是 B。
    slowDetail.resolve(row('a', '慢任务 AAA'))
    await flushPromises()
    expect(w.find('.modal-body').text()).toContain('快任务 BBB')
    expect(w.find('.modal-body').text()).not.toContain('慢任务 AAA')
    w.unmount()
  })

  it('先点慢任务 A 再点快任务 B：A 迟到拒绝不把详情翻成「任务不在册」', async () => {
    const slowDetail = deferred<any>()
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('a', '慢任务 AAA'), row('b', '快任务 BBB')] })
      if (cmd === 'detail') {
        return data.id === 'a' ? slowDetail.promise : Promise.resolve(row('b', '快任务 BBB'))
      }
      if (cmd === 'events') return Promise.resolve({ events: [], running: false })
      return Promise.resolve({})
    })
    const w = await mountView()
    await w.findAll('.subagent-row')[0].trigger('click')
    await w.findAll('.subagent-row')[1].trigger('click')
    await flushPromises()

    slowDetail.reject(new Error('boom'))
    await flushPromises()
    expect(w.find('.modal-body').text()).toContain('快任务 BBB')
    expect(w.text()).not.toContain('任务不在册')
    w.unmount()
  })

  it('无并发时详情正常打开（围栏不误伤正常路径）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('a', '普通任务')] })
      if (cmd === 'detail') return Promise.resolve(row('a', '普通任务'))
      if (cmd === 'events') return Promise.resolve({ events: [], running: false })
      return Promise.resolve({})
    })
    const w = await mountView()
    await w.findAll('.subagent-row')[0].trigger('click')
    await flushPromises()
    expect(w.find('.modal-body').text()).toContain('普通任务')
    // 正常路径关闭后 modal 消失。
    await w.find('.modal-close').trigger('click')
    expect(w.find('.modal-body').exists()).toBe(false)
    w.unmount()
  })
})
