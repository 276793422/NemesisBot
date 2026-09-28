import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// P4（能力扩展 WS8）：发送区三态选择器（立即 / ⚡插队 / ⏭排队）与
// chat.queue_status 徽标。选择器是纯发送侧路由归约（steer 补 `!` 前缀 /
// queue 剥 `!` 标记 / now 原样）；徽标数据源是 chat.queue_status 的
// steer/followUp 分队列计数（与 U7 的 agent.inbox_status chip 刻意分源）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn(),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: ref('connected'),
  }
})

import { send, wsStatus } from '../../composables/useWebSocket'
import type { InboxStatusData } from '../../composables/useInboxStatus'
import type { QueueStatusData } from '../../composables/useQueueStatus'
import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

function inboxSnap(over: Partial<InboxStatusData>): InboxStatusData {
  return {
    available: true,
    session_key: 'agent:main:session:s1',
    next_turn: 0,
    next_step: 0,
    capacity: 8,
    busy: false,
    mode: 'steer',
    ...over,
  }
}

// `over` 缺省 = 全默认快照（调用方多处 queueSnap() 零参直调——此前
// 缺默认值是 vue-tsc 全量 typecheck 的 4 处 TS2554 根因）。
function queueSnap(over: Partial<QueueStatusData> = {}): QueueStatusData {
  return {
    available: true,
    session_key: 'agent:main:session:s1',
    steer: 0,
    followUp: 0,
    capacity: 8,
    busy: false,
    mode: 'steer',
    ...over,
  }
}

/** 按 (module, cmd) 路由 mock：chat.queue_status 走队列快照，其余走 inbox。 */
function routeMock(inbox: InboxStatusData, queue: QueueStatusData) {
  requestMock.mockImplementation((module: string, cmd: string) => {
    if (module === 'chat' && cmd === 'queue_status') return Promise.resolve(queue)
    return Promise.resolve(inbox)
  })
}

async function mountPanel() {
  // 与 ChatPanel.inbox.spec 同纪律：先锚当前会话，streaming 投影才有落点。
  useSessionStore().currentId = 's1'
  const wrapper = mount(ChatPanel)
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  vi.mocked(send).mockReset()
  wsStatus.value = 'connected'
})

describe('ChatPanel P4 三态选择器', () => {
  it('steer 后端：三选项齐全，默认选中「立即」', async () => {
    routeMock(inboxSnap({ mode: 'steer' }), queueSnap())
    const wrapper = await mountPanel()

    const sel = wrapper.find('.send-mode-sel')
    expect(sel.exists()).toBe(true)
    const opts = sel.findAll('.sm-opt')
    expect(opts.map(o => o.text())).toEqual(['立即', '⚡插队', '⏭排队'])
    const active = sel.find('.sm-opt.active')
    expect(active.exists()).toBe(true)
    expect(active.text()).toBe('立即')
    wrapper.unmount()
  })

  it('reject 后端：queue 未启用 → 选择器整体隐藏（不提供假入口）', async () => {
    routeMock(inboxSnap({ mode: 'reject' }), queueSnap({ mode: 'reject' }))
    const wrapper = await mountPanel()

    expect(wrapper.find('.send-mode-sel').exists()).toBe(false)
    wrapper.unmount()
  })

  it('queue 后端（非 steer）：选择器在但「⚡插队」选项隐藏', async () => {
    routeMock(inboxSnap({ mode: 'queue' }), queueSnap({ mode: 'queue' }))
    const wrapper = await mountPanel()

    const opts = wrapper.findAll('.send-mode-sel .sm-opt')
    expect(opts.map(o => o.text())).toEqual(['立即', '⏭排队'])
    wrapper.unmount()
  })
})

describe('ChatPanel P4 发送路由归约', () => {
  async function sendWithMode(mode: 'steer' | 'queue' | 'now', input: string) {
    routeMock(inboxSnap({ mode: 'steer' }), queueSnap())
    const wrapper = await mountPanel()
    const chat = useChatStore()
    if (mode !== 'now') {
      const btnCls = mode === 'steer' ? '.sm-steer' : '.sm-queue'
      await wrapper.find(btnCls).trigger('click')
    }
    chat.input = input
    await wrapper.vm.$nextTick()
    const sendBtn = wrapper.findAll('button').find(b => b.text() === '发送')
    vi.mocked(send).mockClear() // 只看本次点击的上行（同测试多次发送不串）
    await sendBtn!.trigger('click')
    await flushPromises()
    wrapper.unmount()
    return vi.mocked(send).mock.calls[0][0] as string
  }

  it('选「⚡插队」：上行文本自动补 ! 前缀', async () => {
    expect(await sendWithMode('steer', '停一下，先别删')).toBe('! 停一下，先别删')
  })

  it('选「⚡插队」：已带 ! 前缀不重复加', async () => {
    expect(await sendWithMode('steer', '! 已带标记')).toBe('! 已带标记')
  })

  it('选「⏭排队」：剥掉行首 ! 标记（显式排队压过文本路由标记）', async () => {
    expect(await sendWithMode('queue', '! 本想插队，改排队')).toBe('本想插队，改排队')
  })

  it('默认「立即」：文本原样上行（! 前缀语义交后端裁决）', async () => {
    expect(await sendWithMode('now', '普通消息')).toBe('普通消息')
    expect(await sendWithMode('now', '! 手写插队')).toBe('! 手写插队')
  })
})

describe('ChatPanel P4 queue_status 徽标', () => {
  it('streaming 中任一队列非空 → 徽标分队列显示计数；清空后消失', async () => {
    routeMock(inboxSnap({ mode: 'steer', busy: true }), queueSnap({ steer: 1, followUp: 2 }))
    const wrapper = await mountPanel()
    const chat = useChatStore()

    // 初始快照已在挂载时拉取（syncInboxMode 双链刷新）；进入 streaming 徽标出现。
    chat.streaming = true
    await wrapper.vm.$nextTick()
    const badge = wrapper.find('.queue-badge')
    expect(badge.exists()).toBe(true)
    expect(badge.find('.qb-steer').text()).toBe('⚡插队 1')
    expect(badge.find('.qb-followup').text()).toBe('⏭排队 2')

    // 队列清空：换快照后经 streaming=true→false 的 watch（stopQueuePolling +
    // syncInboxMode 重拉）刷新，徽标随之消失；再回 streaming 仍不出现。
    routeMock(inboxSnap({ mode: 'steer', busy: false }), queueSnap())
    chat.streaming = false
    await flushPromises()
    chat.streaming = true
    await wrapper.vm.$nextTick()
    expect(wrapper.find('.queue-badge').exists()).toBe(false)
    wrapper.unmount()
  })

  it('双队列皆空：streaming 中也不显示徽标', async () => {
    routeMock(inboxSnap({ mode: 'steer' }), queueSnap())
    const wrapper = await mountPanel()
    const chat = useChatStore()
    chat.streaming = true
    await wrapper.vm.$nextTick()
    expect(wrapper.find('.queue-badge').exists()).toBe(false)
    wrapper.unmount()
  })
})
