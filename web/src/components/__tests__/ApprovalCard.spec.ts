import { mount } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// ApprovalCard（M7 审批卡模态）：
// - 无 pending 时不渲染；
// - SSE 事件后渲染卡（操作/目标/理由/风险徽标/倒计时）；
// - 批准/拒绝按钮 → respondTo → WSAPI approval.respond；
// - respond 成功后卡片消失。

const sseHandlers = new Map<string, (data?: unknown) => void>()
const wsapiRequest = vi.fn()

vi.mock('../../composables/useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    sseHandlers.set(type, handler)
  }),
  off: vi.fn(),
}))

vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: wsapiRequest }),
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

import { useApprovals, _resetApprovalsForTest } from '../../composables/useApprovals'
import ApprovalCard from '../ApprovalCard.vue'

function fireApproval(data: Record<string, unknown>) {
  sseHandlers.get('approval-requested')!(data)
}

function makePayload(requestId: string, overrides: Record<string, unknown> = {}) {
  return {
    request_id: requestId,
    operation: 'process_exec',
    target: 'cargo publish',
    risk_level: 'HIGH',
    reason: 'rule: exec-publish',
    timeout_secs: 60,
    age_secs: 0,
    ...overrides,
  }
}

beforeEach(() => {
  _resetApprovalsForTest()
  sseHandlers.clear()
  wsapiRequest.mockReset().mockResolvedValue({ delivered: true })
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

async function mounted() {
  const { initApprovals } = useApprovals()
  const w = mount(ApprovalCard)
  initApprovals()
  return w
}

describe('ApprovalCard', () => {
  it('无 pending 时不渲染模态', async () => {
    const w = await mounted()
    expect(w.find('.approval-backdrop').exists()).toBe(false)
    w.unmount()
  })

  it('SSE 事件后渲染卡：操作/目标/理由/风险徽标齐全', async () => {
    const w = await mounted()
    fireApproval(makePayload('r-view'))
    await vi.advanceTimersByTimeAsync(0)

    expect(w.find('.approval-backdrop').exists()).toBe(true)
    expect(w.find('.approval-card').classes()).toContain('high')
    expect(w.text()).toContain('process_exec')
    expect(w.text()).toContain('cargo publish')
    expect(w.text()).toContain('rule: exec-publish')
    expect(w.find('.badge-error').exists()).toBe(true)
    w.unmount()
  })

  it('倒计时显示剩余秒数并逐秒递减', async () => {
    const w = await mounted()
    // now ref 初值是模块收集时刻的真实时钟；先走一跳 interval 对齐 fake 时钟。
    vi.advanceTimersByTime(1000)
    await w.vm.$nextTick()
    fireApproval(makePayload('r-cd', { timeout_secs: 30 }))
    await vi.advanceTimersByTimeAsync(0)
    expect(w.find('.approval-countdown').text()).toContain('30s')

    vi.advanceTimersByTime(5000)
    await w.vm.$nextTick()
    expect(w.find('.approval-countdown').text()).toContain('25s')
    w.unmount()
  })

  it('CRITICAL 风险走 critical 边框 + 错误徽标', async () => {
    const w = await mounted()
    fireApproval(makePayload('r-crit', { risk_level: 'CRITICAL' }))
    await vi.advanceTimersByTimeAsync(0)

    expect(w.find('.approval-card').classes()).toContain('critical')
    expect(w.text()).toContain('严重')
    w.unmount()
  })

  it('点批准 → WSAPI respond approved=true → 卡片消失', async () => {
    const w = await mounted()
    fireApproval(makePayload('r-btn', { pattern: 'cargo publish *' }))
    await vi.advanceTimersByTimeAsync(0)

    const buttons = w.findAll('button')
    const approve = buttons.find(b => b.text() === '批准')!
    const deny = buttons.find(b => b.text() === '拒绝')!
    expect(approve).toBeTruthy()
    expect(deny).toBeTruthy()

    await approve.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('approval', 'respond', {
      request_id: 'r-btn',
      approved: true,
      always: false,
    })
    expect(w.find('.approval-backdrop').exists()).toBe(false)
    w.unmount()
  })

  it('点拒绝 → WSAPI respond approved=false', async () => {
    const w = await mounted()
    fireApproval(makePayload('r-deny'))
    await vi.advanceTimersByTimeAsync(0)

    const deny = w.findAll('button').find(b => b.text() === '拒绝')!
    await deny.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('approval', 'respond', {
      request_id: 'r-deny',
      approved: false,
      always: false,
    })
    w.unmount()
  })

  // ---- F3（总是允许：pattern 回显 + 层级安全门 UX 镜像）----

  it('F3: exec + pattern → 「总是允许」可见，回显 pattern，点击 respond always=true', async () => {
    const w = await mounted()
    fireApproval(makePayload('r-always', { pattern: 'cargo publish *' }))
    await vi.advanceTimersByTimeAsync(0)

    // 确认串回显：用户看到的就是将写入规则的 pattern。
    expect(w.find('.approval-always-scope').text()).toContain('cargo publish *')
    const always = w.findAll('button').find(b => b.text() === '总是允许')
    expect(always).toBeTruthy()

    await always!.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('approval', 'respond', {
      request_id: 'r-always',
      approved: true,
      always: true,
    })
    w.unmount()
  })

  it('F3: CRITICAL 非 exec → 「总是允许」隐藏（层级安全门）', async () => {
    const w = await mounted()
    fireApproval(
      makePayload('r-crit-w', {
        operation: 'file_write',
        target: '/etc/passwd',
        risk_level: 'CRITICAL',
        pattern: '/etc/passwd',
      }),
    )
    await vi.advanceTimersByTimeAsync(0)

    expect(
      w.findAll('button').find(b => b.text() === '总是允许'),
    ).toBeUndefined()
    expect(w.find('.approval-always-scope').exists()).toBe(false)
    w.unmount()
  })

  it('F3: CRITICAL exec → 「总是允许」可见（exec 豁免）', async () => {
    const w = await mounted()
    fireApproval(
      makePayload('r-crit-exec', { risk_level: 'CRITICAL', pattern: 'cargo build *' }),
    )
    await vi.advanceTimersByTimeAsync(0)
    expect(
      w.findAll('button').find(b => b.text() === '总是允许'),
    ).toBeTruthy()
    w.unmount()
  })

  it('F3: 无 pattern（空/缺省）→ 「总是允许」隐藏', async () => {
    const w = await mounted()
    fireApproval(makePayload('r-nopat'))
    await vi.advanceTimersByTimeAsync(0)
    expect(
      w.findAll('button').find(b => b.text() === '总是允许'),
    ).toBeUndefined()
    expect(w.find('.approval-always-scope').exists()).toBe(false)
    w.unmount()
  })

  // ---- F6（拒绝备注：输入框 + deny 携带 note + 竞速摘卡）----

  it('F6: 备注输入后点拒绝 → respond 带 note；无输入 → 不带 note 键', async () => {
    const w = await mounted()
    fireApproval(makePayload('r-note', { target: 'kubectl delete pod x' }))
    await vi.advanceTimersByTimeAsync(0)

    const input = w.find('.approval-note-input')
    expect(input.exists()).toBe(true)
    await input.setValue('这是生产集群，用 staging 试')

    const deny = w.findAll('button').find(b => b.text() === '拒绝')!
    await deny.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('approval', 'respond', {
      request_id: 'r-note',
      approved: false,
      always: false,
      note: '这是生产集群，用 staging 试',
    })

    // 第二张卡：不填备注直接拒绝 → payload 无 note 键。
    fireApproval(makePayload('r-nonote'))
    await vi.advanceTimersByTimeAsync(0)
    const deny2 = w.findAll('button').find(b => b.text() === '拒绝')!
    await deny2.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenLastCalledWith('approval', 'respond', {
      request_id: 'r-nonote',
      approved: false,
      always: false,
    })
    w.unmount()
  })

  it('F6: 点批准不携带备注（note 仅拒绝语义）', async () => {
    const w = await mounted()
    fireApproval(makePayload('r-appr'))
    await vi.advanceTimersByTimeAsync(0)
    await w.find('.approval-note-input').setValue('随手批的')

    const approve = w.findAll('button').find(b => b.text() === '批准')!
    await approve.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('approval', 'respond', {
      request_id: 'r-appr',
      approved: true,
      always: false,
    })
    w.unmount()
  })

  it('F6: approval-resolved 广播 → 卡片静默摘除（竞速败方无感关闭）', async () => {
    const w = await mounted()
    fireApproval(makePayload('r-loser'))
    await vi.advanceTimersByTimeAsync(0)
    expect(w.find('.approval-backdrop').exists()).toBe(true)

    sseHandlers.get('approval-resolved')!({ request_id: 'r-loser', decision: 'denied' })
    await vi.advanceTimersByTimeAsync(0)
    expect(w.find('.approval-backdrop').exists()).toBe(false)
    w.unmount()
  })

  // ---- M5（2026-10-10 集群无人值守硬化：远程审批分区）----

  function fireRemoteApproval(data: Record<string, unknown>) {
    sseHandlers.get('approval-remote-requested')!(data)
  }

  function makeRemotePayload(requestId: string, overrides: Record<string, unknown> = {}) {
    return {
      request_id: requestId,
      short_id: requestId.slice(0, 8),
      worker: 'node-b',
      operation: 'process_exec',
      target: 'rm -rf build/',
      risk_level: 'CRITICAL',
      reason: '看板 #7 子任务执行',
      issue_number: 7,
      timeout_secs: 300,
      age_secs: 0,
      ...overrides,
    }
  }

  it('M5: SSE approval-remote-requested → 远程卡渲染（节点徽标/短号/单号），无「总是允许」', async () => {
    const w = await mounted()
    fireRemoteApproval(makeRemotePayload('11111111-aaaa-bbbb-cccc-dddddddddddd'))
    await vi.advanceTimersByTimeAsync(0)

    expect(w.find('.approval-backdrop').exists()).toBe(true)
    expect(w.find('.approval-card.remote').exists()).toBe(true)
    expect(w.text()).toContain('远程审批')
    expect(w.text()).toContain('节点：node-b')
    expect(w.text()).toContain('11111111')
    expect(w.text()).toContain('看板 #7')
    expect(w.text()).toContain('rm -rf build/')
    expect(w.find('.badge-error').exists()).toBe(true)
    // 远程批复永远不记忆规则：无 pattern 回显、无「总是允许」按钮。
    expect(w.find('.approval-always-scope').exists()).toBe(false)
    expect(
      w.findAll('button').find(b => b.text() === '总是允许'),
    ).toBeUndefined()
    w.unmount()
  })

  it('M5: 点批准 → WSAPI approval.remote_respond approved=true → 卡片消失', async () => {
    const w = await mounted()
    fireRemoteApproval(makeRemotePayload('22222222-aaaa-bbbb-cccc-dddddddddddd'))
    await vi.advanceTimersByTimeAsync(0)

    const approve = w.findAll('button').find(b => b.text() === '批准')!
    await approve.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('approval', 'remote_respond', {
      request_id: '22222222-aaaa-bbbb-cccc-dddddddddddd',
      approved: true,
    })
    expect(w.find('.approval-backdrop').exists()).toBe(false)
    w.unmount()
  })

  it('M5: 远程拒绝带备注 → remote_respond 携带 note', async () => {
    const w = await mounted()
    fireRemoteApproval(makeRemotePayload('33333333-aaaa-bbbb-cccc-dddddddddddd'))
    await vi.advanceTimersByTimeAsync(0)

    await w.find('.approval-note-input').setValue('目标路径不对，先确认')
    const deny = w.findAll('button').find(b => b.text() === '拒绝')!
    await deny.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('approval', 'remote_respond', {
      request_id: '33333333-aaaa-bbbb-cccc-dddddddddddd',
      approved: false,
      note: '目标路径不对，先确认',
    })
    w.unmount()
  })

  it('M5: approval-remote-resolved 广播 → 远程卡静默摘除', async () => {
    const w = await mounted()
    fireRemoteApproval(makeRemotePayload('44444444-aaaa-bbbb-cccc-dddddddddddd'))
    await vi.advanceTimersByTimeAsync(0)
    expect(w.find('.approval-backdrop').exists()).toBe(true)

    sseHandlers.get('approval-remote-resolved')!({
      request_id: '44444444-aaaa-bbbb-cccc-dddddddddddd',
      decision: 'approved',
      source: 'board_comment',
    })
    await vi.advanceTimersByTimeAsync(0)
    expect(w.find('.approval-backdrop').exists()).toBe(false)
    w.unmount()
  })

  it('M5: initApprovals 拉 approval.remote_list 兜底补齐断线漏掉的卡', async () => {
    wsapiRequest.mockImplementation((_mod: string, cmd: string) =>
      cmd === 'remote_list'
        ? Promise.resolve({
            pending: [makeRemotePayload('55555555-aaaa-bbbb-cccc-dddddddddddd')],
          })
        : Promise.resolve({}),
    )
    const w = await mounted()
    await vi.advanceTimersByTimeAsync(0)

    expect(wsapiRequest).toHaveBeenCalledWith('approval', 'remote_list', {}, 5000)
    expect(w.find('.approval-card.remote').exists()).toBe(true)
    expect(w.text()).toContain('55555555')
    w.unmount()
  })

  it('M5: 本地卡与远程卡并存（同一模态两张卡）', async () => {
    const w = await mounted()
    fireApproval(makePayload('local-1', { pattern: 'cargo publish *' }))
    fireRemoteApproval(makeRemotePayload('66666666-aaaa-bbbb-cccc-dddddddddddd'))
    await vi.advanceTimersByTimeAsync(0)

    expect(w.findAll('.approval-card').length).toBe(2)
    expect(w.find('.approval-card.remote').exists()).toBe(true)
    // 本地卡有「总是允许」资格、远程卡没有——按钮只出现在本地卡。
    expect(
      w.findAll('button').filter(b => b.text() === '总是允许').length,
    ).toBe(1)
    w.unmount()
  })
})
