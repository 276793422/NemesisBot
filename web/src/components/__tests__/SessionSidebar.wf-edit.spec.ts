import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// wf_edit 会话「主聊天侧栏隐藏」判定（2026-10-19 修复）。
//
// 根因：隐藏标记此前只有浏览器 localStorage 登记表
// （`nemesisbot_wf_edit_sessions`，gen TAB 挂载时 register）这一个真相源；
// 服务端 sessions.list 回带的 bindings（`wf_agentgen:*` → sid，B 项绑定
// 注册表）被 store.fetchList 丢弃。任何切断 (browser, origin) ↔ localStorage
// 连续性的事件（重启后端口 bump / localhost↔127.0.0.1 / 换浏览器 / 清站点
// 数据）都让登记失忆 → 工作流编辑会话假性出现在聊天侧栏；回到 gen TAB
// 又被 register「治愈」（自愈假象）。
//
// 修复：store 保存 resp.bindings；侧栏 isWfEditSession 改**并集**判定
// （localStorage 登记 ∪ 服务端 `wf_agentgen:*` 绑定）。以下用例锁定三个面：
// 服务端权威面单独成立 / 本地登记面单独成立 / 非 agentgen 前缀绑定不隐藏。

const listMock = vi.fn()
const listProjectsMock = vi.fn()

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn() }),
}))

vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({
    list: (...a: any[]) => listMock(...a),
    listProjects: (...a: any[]) => listProjectsMock(...a),
    create: vi.fn(),
    createProject: vi.fn(),
    removeProject: vi.fn(),
    renameProject: vi.fn(),
    openProjectDir: vi.fn(),
    rename: vi.fn(),
    clear: vi.fn(),
    export: vi.fn(),
    delete: vi.fn(),
    turns: vi.fn(),
    fork: vi.fn(),
  }),
}))

import SessionSidebar from '../SessionSidebar.vue'
import { useWfEditSessions } from '../../composables/wfEditSessions'

beforeEach(() => {
  setActivePinia(createPinia())
  listMock.mockReset()
  listProjectsMock.mockReset()
  listProjectsMock.mockResolvedValue({ projects: [], count: 0 })
  localStorage.clear()
})

function titles(w: any): string[] {
  return w.findAll('.session-item .session-title').map((t: any) => t.text().replace('📌', '').trim())
}

async function mountWith(sessions: any[], bindings: Record<string, string>) {
  listMock.mockResolvedValue({ sessions, bindings })
  const w = mount(SessionSidebar)
  await flushPromises()
  return w
}

const row = (id: string, first: string) => ({ id, firstMessage: first })

describe('SessionSidebar wf_edit 隐藏（服务端绑定 ∪ 本地登记）', () => {
  it('核心回归锁：localStorage 失忆（零登记），服务端 wf_agentgen:* 绑定命中 → 仍隐藏', async () => {
    const w = await mountWith(
      [row('main', '普通对话'), row('wfe', '工作流编辑对话')],
      { 'wf_agentgen:demo': 'wfe' },
    )
    expect(titles(w)).toEqual(['普通对话'])
    w.unmount()
  })

  it('并集保守方向：本地登记在场而服务端 bindings 未回带 → 仍隐藏', async () => {
    useWfEditSessions().register('wfe2', 'demo')
    const w = await mountWith(
      [row('main2', '普通对话'), row('wfe2', '工作流编辑对话')],
      {},
    )
    expect(titles(w)).toEqual(['普通对话'])
    w.unmount()
  })

  it('前缀限定：非 wf_agentgen 绑定键命中的会话不隐藏', async () => {
    const w = await mountWith(
      [row('main3', '普通对话'), row('wfe3', '其他绑定会话')],
      { 'other:binding': 'wfe3' },
    )
    expect(titles(w)).toEqual(['普通对话', '其他绑定会话'])
    w.unmount()
  })
})
