// BUG-20261118 B.4：DiscussionPanel 主持人信息性提示契约。
// - 本节点非 coordinator → 信息条可见（主持人缺席提示，不做发帖拦截）；
// - coordinator / config.get 失败 → 信息条不显示（fail-quiet）。
// 后端统一 handler 全员装配（board_bus::build_nb_bus_handler）由
// board_bus/tests.rs unified_handler_routes_wake_and_comment_union 钉住；
// 本 spec 只钉前端提示面。
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  // initWSAPI：useWebSocket 模块加载期调用（sendRaw 注入），mock 为 no-op。
  initWSAPI: () => {},
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('../../../composables/useToast', () => ({
  useToast: () => ({
    success: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
    toasts: [],
  }),
}))

import DiscussionPanel from '../DiscussionPanel.vue'

function mockBackend(role: string | Error) {
  requestMock.mockImplementation(async (_m: string, cmd: string) => {
    if (cmd === 'config.get') {
      if (role instanceof Error) throw role
      return { node_id: 'node-self', name: 'Self', role }
    }
    if (cmd === 'channel.list') return { channels: [{ id: 1, name: '#dev', description: '', created_at: 1 }] }
    if (cmd === 'channel.messages') return { messages: [] }
    if (cmd === 'nodes.list') return { nodes: [] }
    return {}
  })
}

beforeEach(() => {
  requestMock.mockReset()
})

describe('DiscussionPanel 主持人信息条（B.4）', () => {
  it('非 coordinator 节点显示主持人缺席提示（信息性，不拦截输入）', async () => {
    mockBackend('worker')
    const w = mount(DiscussionPanel)
    await flushPromises()

    const note = w.find('.moderator-note')
    expect(note.exists()).toBe(true)
    expect(note.text()).toContain('本节点非 coordinator')
    expect(note.text()).toContain('@all')
    // 发送输入照常可用（不做拦截——信息性提示而已）。
    expect(w.find('textarea.composer-input').exists()).toBe(true)
    w.unmount()
  })

  it('coordinator 节点不显示提示', async () => {
    mockBackend('coordinator')
    const w = mount(DiscussionPanel)
    await flushPromises()

    expect(w.find('.moderator-note').exists()).toBe(false)
    w.unmount()
  })

  it('config.get 失败 = 静默不显示（fail-quiet，不阻塞消息加载）', async () => {
    mockBackend(new Error('boom'))
    const w = mount(DiscussionPanel)
    await flushPromises()

    expect(w.find('.moderator-note').exists()).toBe(false)
    // 频道照常加载（config.get 失败不阻塞）。
    expect(w.findAll('.channel-item').length).toBe(1)
    w.unmount()
  })
})
