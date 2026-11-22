import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 系统通知条（2026-11-16 系统注入消息渲染成用户气泡 BUG）：role=user 的
// 内部注入行（`system_notice` 键 = msg_class class 投影 kind slug；旧章
// "system" 兼容读 + cron tick 行既有 `cron_job_id` 键）经历史装载三映射位
// 透传后，渲染折叠单行徽标条而非用户气泡。真人 user 行零变化（用户气泡
// 旧行为）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

/** 捕获槽（vi.hoisted——mock 工厂在 import 期执行，普通顶层 let 会 TDZ）。 */
const h = vi.hoisted(() => ({
  wsHandler: null as ((data: any) => void) | null,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  wsStatus: null as any,
}))

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  h.wsStatus = ref<'connected' | 'disconnected'>('connected')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn((fn: (data: any) => void) => { h.wsHandler = fn }),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: h.wsStatus,
  }
})

import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  useSessionStore().currentId = 's1'
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  h.wsHandler = null
  h.wsStatus.value = 'connected'
})

async function mountPanel() {
  const w = mount(ChatPanel)
  await flushPromises()
  return w
}

/** 装载历史行（走首连 loadHistory 的 history_response 路径——真实链路）。 */
async function loadHistory(w: Awaited<ReturnType<typeof mountPanel>>, messages: any[]) {
  const { sendHistoryRequest } = await import('../../composables/useWebSocket')
  const calls = (sendHistoryRequest as ReturnType<typeof vi.fn>).mock.calls
  const requestId = calls[calls.length - 1][0] as string
  h.wsHandler!({
    type: 'message',
    module: 'chat',
    cmd: 'history_response',
    data: {
      request_id: requestId,
      session_id: 's1',
      oldest_index: 0,
      has_more: false,
      messages,
    },
  })
  await flushPromises()
}

describe('ChatPanel 系统通知条', () => {
  it('历史行 system_notice → 折叠通知条（替代用户气泡，⚙ + 系统通知）', async () => {
    const w = await mountPanel()
    await loadHistory(w, [
      { role: 'user', content: '帮我把构建跑起来' },
      {
        role: 'user',
        content: 'Background job #4 finished: exit_code=Some(0), success=true, killed=false.\n--- 最近输出（尾部，全文可用 background_output 分页读）---\nok',
        system_notice: 'system',
      },
      { role: 'assistant', content: '收到完成通知，构建成功。' },
    ])
    const notices = w.findAll('.system-notice')
    expect(notices).toHaveLength(1)
    expect(notices[0].find('.system-notice-glyph').text()).toBe('⚙')
    expect(notices[0].find('.system-notice-tag').text()).toBe('系统通知')
    expect(notices[0].attributes('title')).toContain('Background job #4 finished')
    // 通知条替代气泡——该行不再渲染用户气泡（真人行仍渲染）。
    expect(w.findAll('.message-bubble')).toHaveLength(2)
    // 折叠态单行截断（非 expanded）。
    expect(notices[0].classes()).not.toContain('expanded')
    // 不误入压缩注记家族（role=user 的通知条 ≠ compact-note）。
    expect(w.findAll('.compact-note')).toHaveLength(0)
    // A4（2026-11-19 头像语义）：通知条是过程行——不渲染头像（头像只属于
    // 答案：头像没出现 = 还在跑；头像出现 = 答案到了）。
    const rows = w.findAll('.message')
    expect(rows[1].find('.message-avatar').exists()).toBe(false)
    // 真人行头像照常。
    expect(rows[0].find('.message-avatar').text()).toBe('U')
    expect(rows[2].find('.message-avatar').text()).toBe('NB')
    w.unmount()
  })

  it('历史行 cron_job_id → 同一通知条分支（⏰ + 定时任务）', async () => {
    const w = await mountPanel()
    await loadHistory(w, [
      {
        role: 'user',
        content: '每日站会提醒',
        cron_job_id: 'job-7',
        cron_job_name: 'daily-standup',
      },
    ])
    const notices = w.findAll('.system-notice')
    expect(notices).toHaveLength(1)
    expect(notices[0].find('.system-notice-glyph').text()).toBe('⏰')
    expect(notices[0].find('.system-notice-tag').text()).toBe('定时任务')
    // 通知行本身无气泡（末行 user 行的「处理中」typing 指示器同用
    // .message-bubble 类——那是 pendingTurn 占位，属真实轮次语义，不在断言内）。
    const rows = w.findAll('.message')
    expect(rows[0].find('.message-bubble').exists()).toBe(false)
    w.unmount()
  })

  it('点击展开全文 / 再点收起（与压缩注记同款 expandedCompactNotes）', async () => {
    const w = await mountPanel()
    await loadHistory(w, [
      {
        role: 'user',
        content: 'Background job #2 finished: exit_code=Some(101), success=false, killed=true.\n--- 最近输出 ---\nbuild failed',
        system_notice: 'system',
      },
    ])
    const notice = w.find('.system-notice')
    expect(notice.classes()).not.toContain('expanded')
    await notice.trigger('click')
    await flushPromises()
    expect(w.find('.system-notice').classes()).toContain('expanded')
    await w.find('.system-notice').trigger('click')
    await flushPromises()
    expect(w.find('.system-notice').classes()).not.toContain('expanded')
    w.unmount()
  })

  it('真人 user 行零变化：无标记 → 用户气泡旧行为（缺字段兼容）', async () => {
    const w = await mountPanel()
    await loadHistory(w, [
      { role: 'user', content: '普通真人消息（无任何标记键）' },
      { role: 'assistant', content: '回复' },
    ])
    expect(w.findAll('.system-notice')).toHaveLength(0)
    expect(w.findAll('.message-bubble')).toHaveLength(2)
    // avatar 照常 'U'。
    const rows = w.findAll('.message')
    expect(rows[0].find('.message-avatar').text()).toBe('U')
    w.unmount()
  })
})

// msg_class 单轨 A2 kind-aware 渲染（2026-11-21 拍板 MUST）：`system_notice`
// 值 = 后端 system_notice_of 的 class 投影 kind slug（Notice 四族 +
// Instruction 四族），内部消息永不渲染成用户气泡；Notice = 虚线通知条 /
// Instruction = 实线指令徽标行；未知 slug 优雅兜底 ⚙ 系统通知（退役生产者
// 残留/未来新增不白屏不崩）。
describe('ChatPanel 通知条 kind 徽标（msg_class A2）', () => {
  const KIND_CASES: Array<{ slug: string; glyph: string; label: string; family: string }> = [
    { slug: 'events', glyph: '📡', label: '事件唤醒', family: 'notice' },
    { slug: 'loop', glyph: '🔁', label: '循环推进', family: 'notice' },
    { slug: 'approval_continuation', glyph: '📨', label: '审批续行', family: 'notice' },
    { slug: 'merged', glyph: '🔔', label: '合并通告', family: 'notice' },
    { slug: 'goal_round', glyph: '🎯', label: '目标轮次', family: 'instruction' },
    { slug: 'cron_tick', glyph: '⏰', label: '定时任务', family: 'instruction' },
    { slug: 'wal_resume', glyph: '↻', label: '续轮恢复', family: 'instruction' },
    { slug: 'board_dispatch', glyph: '📋', label: '看板派发', family: 'instruction' },
  ]

  for (const kc of KIND_CASES) {
    it(`kind slug ${kc.slug} → ${kc.glyph} ${kc.label}（${kc.family} 族，非用户气泡）`, async () => {
      const w = await mountPanel()
      await loadHistory(w, [
        { role: 'user', content: `[${kc.slug}] 内部注入行`, system_notice: kc.slug },
      ])
      const notice = w.find('.system-notice')
      expect(notice.exists()).toBe(true)
      expect(notice.find('.system-notice-glyph').text()).toBe(kc.glyph)
      expect(notice.find('.system-notice-tag').text()).toBe(kc.label)
      // 形态族变体类：Instruction 实线 / Notice 虚线。
      expect(notice.classes()).toContain(kc.family)
      // A2 MUST：内部消息永不渲染成用户气泡。
      const rows = w.findAll('.message')
      expect(rows[0].find('.message-bubble').exists()).toBe(false)
      expect(rows[0].find('.message-avatar').exists()).toBe(false)
      w.unmount()
    })
  }

  it('未知 slug → ⚙ 系统通知优雅兜底（退役生产者残留不白屏不崩）', async () => {
    const w = await mountPanel()
    await loadHistory(w, [
      { role: 'user', content: '退役生产者残留行', system_notice: 'background_wake' },
    ])
    const notice = w.find('.system-notice')
    expect(notice.exists()).toBe(true)
    expect(notice.find('.system-notice-glyph').text()).toBe('⚙')
    expect(notice.find('.system-notice-tag').text()).toBe('系统通知')
    expect(notice.classes()).toContain('notice')
    expect(notice.classes()).not.toContain('instruction')
    w.unmount()
  })
})
