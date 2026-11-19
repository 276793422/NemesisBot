import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 系统通知条（2026-11-16 系统注入消息渲染成用户气泡 BUG）：role=user 的
// 系统注入行（后台任务完成唤醒/审批裁决回灌/Turn WAL 续轮，后端按
// sender_id=="system" 推导落 jsonl `system_notice` 键）与 cron tick 行
// （既有 `cron_job_id` 键）经历史装载三映射位透传后，渲染折叠单行通知条
// 而非用户气泡。真人 user 行零变化（用户气泡旧行为）。

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
