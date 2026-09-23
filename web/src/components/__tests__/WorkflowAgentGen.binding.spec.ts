import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// B/C（2026-09-23 会话绑定注册表）：WorkflowAgentGen 的服务端绑定契约。
//
// 钉住四件事：
// 1. 对话生成面板**不碰全局 currentId**——ChatPanel 经 `:session-id` prop
//    钉死目标会话（跨会话串扰的视图层根源）；
// 2. 每次进入目标都带 `binding_key` 走服务端 get-or-create（幂等复用，
//    复制机器在服务端终结）；
// 3. 旧 localStorage 映射只作一次性迁移源：会话存活 → set_binding 收编
//    （历史对话不丢）后清除条目；已死 → 直接新建，绝不 set_binding；
// 4. draft_apply 成功 → applied 事件触发原地重绑（demo 键指向当前会话 +
//    释放 __new__ 键 + 目标切到正式工作流名）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('../../composables/useWebSocket', () => ({
  addMessageHandler: vi.fn(),
  removeMessageHandler: vi.fn(),
}))

import WorkflowAgentGen from '../workflow/WorkflowAgentGen.vue'
import agentGenSource from '../workflow/WorkflowAgentGen.vue?raw'
import { useSessionStore } from '../../stores/session'

// localStorage shim（node26/jsdom 无 --localstorage-file 时全局缺失）。
function ensureLocalStorage(): void {
  try {
    globalThis.localStorage.getItem('__probe__')
    return
  } catch {
    /* 落到 shim */
  }
  const store = new Map<string, string>()
  ;(globalThis as any).localStorage = {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => {
      store.set(k, String(v))
    },
    removeItem: (k: string) => {
      store.delete(k)
    },
    clear: () => store.clear(),
  }
}

const TARGET_KEY = 'nemesisbot_wf_agentGen_target'
const LEGACY_KEY = 'nemesisbot_wf_agentGen_sids'

let listRows: any[]
let createResponse: { session_id: string; title: string; reused: boolean }
let createCalls: any[]
let setBindingCalls: any[]
let removeBindingCalls: any[]

function stubRequest(): void {
  createCalls = []
  setBindingCalls = []
  removeBindingCalls = []
  requestMock.mockImplementation((module: string, cmd: string, data: any) => {
    if (module === 'sessions') {
      if (cmd === 'list') return Promise.resolve({ sessions: listRows, bindings: {} })
      if (cmd === 'create') {
        createCalls.push(data)
        return Promise.resolve({ ...createResponse })
      }
      if (cmd === 'set_binding') {
        setBindingCalls.push(data)
        return Promise.resolve({ ok: true })
      }
      if (cmd === 'remove_binding') {
        removeBindingCalls.push(data)
        return Promise.resolve({ ok: true, removed: true })
      }
    }
    if (module === 'workflow') {
      if (cmd === 'list')
        return Promise.resolve({
          workflows: [{ name: 'demo_wf', description: '', version: '1.0.0', triggers: [], nodes: [], edges: [] }],
          trigger_driver_status: {},
        })
      if (cmd === 'draft_list') return Promise.resolve({ drafts: [] })
    }
    return Promise.resolve({})
  })
}

function row(id: string): any {
  return { id, channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: id, model: '' }
}

async function mountGen() {
  const wrapper = mount(WorkflowAgentGen, {
    global: {
      stubs: {
        ChatPanel: { name: 'ChatPanelStub', template: '<div class="cp-stub"/>' },
        WorkflowDraftPanel: { name: 'WfDraftStub', template: '<div class="draft-stub"/>', emits: ['applied'] },
      },
    },
  })
  await flushPromises()
  return wrapper
}

function chatPanelOf(wrapper: ReturnType<typeof mount>) {
  return wrapper.findComponent({ name: 'ChatPanelStub' })
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  stubRequest()
  ensureLocalStorage()
  try {
    localStorage.clear()
  } catch {
    /* shim 兜底 */
  }
  listRows = []
  createResponse = { session_id: 'gen-1', title: '对话生成：新建工作流', reused: false }
  useSessionStore().currentId = 'main-sess'
})

describe('WorkflowAgentGen 服务端会话绑定', () => {
  it('不碰全局 currentId；ChatPanel 收到钉死的 session-id；create 带 binding_key', async () => {
    const sessionStore = useSessionStore()
    const wrapper = await mountGen()

    // 服务端 get-or-create 被调用（幂等键 = wf_agentgen:__new__）。
    expect(createCalls).toHaveLength(1)
    expect(createCalls[0].binding_key).toBe('wf_agentgen:__new__')
    expect(createCalls[0].title).toBe('对话生成：新建工作流')

    // 视图层：面板钉死目标会话，全局选中分毫未动（跨会话串扰回归钉）。
    expect(chatPanelOf(wrapper).attributes('session-id')).toBe('gen-1')
    expect(sessionStore.currentId).toBe('main-sess')

    wrapper.unmount()
  })

  it('localStorage 播种迁移：会话存活 → set_binding 收编 + create 复用 + 条目清除', async () => {
    listRows = [row('old-1')]
    createResponse = { session_id: 'old-1', title: 'x', reused: true }
    localStorage.setItem(LEGACY_KEY, JSON.stringify({ __new__: 'old-1' }))

    const wrapper = await mountGen()

    // 先收编（历史对话不丢），再 get-or-create 幂等命中。
    expect(setBindingCalls).toHaveLength(1)
    expect(setBindingCalls[0]).toEqual({ binding_key: 'wf_agentgen:__new__', session_id: 'old-1' })
    expect(createCalls).toHaveLength(1)
    expect(chatPanelOf(wrapper).attributes('session-id')).toBe('old-1')
    // 迁移完成：旧条目清除（零本地状态）。
    expect(JSON.parse(localStorage.getItem(LEGACY_KEY) || '{}')).not.toHaveProperty('__new__')

    wrapper.unmount()
  })

  it('localStorage 条目已死（不在列表）→ 不 set_binding，直接新建', async () => {
    listRows = []
    localStorage.setItem(LEGACY_KEY, JSON.stringify({ __new__: 'dead-1' }))

    const wrapper = await mountGen()

    expect(setBindingCalls).toHaveLength(0)
    expect(createCalls).toHaveLength(1)
    expect(chatPanelOf(wrapper).attributes('session-id')).toBe('gen-1')
    expect(JSON.parse(localStorage.getItem(LEGACY_KEY) || '{}')).not.toHaveProperty('__new__')

    wrapper.unmount()
  })

  it('draft_apply 成功 → applied 事件触发原地重绑：demo 键接管 + __new__ 释放 + 目标切换', async () => {
    const wrapper = await mountGen()
    await flushPromises()

    const draft = wrapper.findComponent({ name: 'WfDraftStub' })
    draft.vm.$emit('applied', 'demo_wf')
    await flushPromises()
    await flushPromises()

    // 重绑：当前会话 → demo_wf 专用键；__new__ 引导键释放。
    expect(setBindingCalls).toContainEqual({ binding_key: 'wf_agentgen:demo_wf', session_id: 'gen-1' })
    expect(removeBindingCalls).toContainEqual({ binding_key: 'wf_agentgen:__new__' })

    // 目标切换跟进：watch(selectedTarget) → ensureSessionFor('demo_wf') →
    // 第二次 create 幂等命中刚重绑的会话；面板会话不变（不重载）。
    const demoCreates = createCalls.filter(c => c.binding_key === 'wf_agentgen:demo_wf')
    expect(demoCreates.length).toBeGreaterThanOrEqual(1)
    expect(chatPanelOf(wrapper).attributes('session-id')).toBe('gen-1')

    wrapper.unmount()
  })

  it('E 高度链契约：宿主给 .page-chat 有界高度 + 纵向 flex（jsdom 无布局引擎，源码钉）', () => {
    // 滚动条根因（E）：.gen-chat 自身可收缩 + .page-chat 宿主约束。
    // 三者缺一，.chat-messages 的 flex:1 + overflow-y:auto 失去上界。
    expect(agentGenSource).toMatch(/\.gen-chat \{[\s\S]*?min-height:\s*0/)
    expect(agentGenSource).toMatch(
      /\.gen-chat > :deep\(\.page-chat\) \{[\s\S]*?min-height:\s*0[\s\S]*?flex-direction:\s*column/,
    )
  })
})
