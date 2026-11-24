import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'
import { ref } from 'vue'

// SessionResourcesPanel（2026-11-17 会话资源盘点看板 B3 前端主体）：
// - 渲染分区：identity 头部 / 执行态大卡 / 在途区（只展示）/ 资源卡流 /
//   空徽标流 / Unavailable ⚠ 行 / CTA 矩阵；
// - B4 活态：WS tool_event 帧会话过滤 → debounce 重拉；SSE approval/
//   question 四事件 session_key 匹配 → 重拉；revision 乱序丢弃；
// - 会话切换清态（旧回包丢弃）；
// - durable_loops 空态内联表单（loops.create 初始排程入参透传）；
// - session_allow 卡内开关。
//
// 红线回归钉：在途区只展示清单，绝不渲染审批交互按钮（审批走既有弹窗
// 链路——面板里出现「批准/拒绝」按钮 = 红线破）。

// vi.mock 工厂被提升到 import 之前执行——工厂内引用的句柄必须走
// vi.hoisted（直接 const 声明会 TDZ ReferenceError）。
const { requestMock, pushMock, setAllowMock, sseHandlers } = vi.hoisted(() => ({
  requestMock: vi.fn(),
  pushMock: vi.fn(),
  setAllowMock: vi.fn(),
  sseHandlers: {} as Record<string, ((d: any) => void)[]>,
}))

vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('../../../composables/useWebSocket', async () => {
  const vue = await import('vue')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn(),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: vue.ref('connected'),
  }
})

vi.mock('../../../composables/useSSE', () => ({
  on: (type: string, h: (d: any) => void) => {
    ;(sseHandlers[type] ??= []).push(h)
  },
  off: (type: string, h?: (d: any) => void) => {
    if (!h) { delete sseHandlers[type]; return }
    sseHandlers[type] = (sseHandlers[type] ?? []).filter(x => x !== h)
  },
}))

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: pushMock }),
}))

// 九键全迁（2026-11-19）后面板新增消费 allowAllBySession/sessionAllowAvailable
//（会话工具行「全部放行」键的激活态）——mock 必须补齐两个 ref，缺键 =
// sessionAllowActive computed 解引用 undefined 崩渲染。
vi.mock('../../../composables/useSessionAllowAll', async () => {
  const { ref } = await import('vue')
  return {
    useSessionAllowAll: () => ({
      allowAllBySession: ref<Record<string, unknown>>({}),
      sessionAllowAvailable: ref(true),
      setSessionAllow: setAllowMock,
      refreshSessionAllow: vi.fn(),
    }),
  }
})

import { addMessageHandler, wsStatus } from '../../../composables/useWebSocket'
import { useChatStore } from '../../../stores/chat'
import SessionResourcesPanel from '../SessionResourcesPanel.vue'

function wsHandler(): (frame: any) => void {
  const calls = vi.mocked(addMessageHandler).mock.calls
  expect(calls.length, 'addMessageHandler must have been called on mount').toBeGreaterThan(0)
  return calls[calls.length - 1][0] as (frame: any) => void
}

/** 全 16 类目形状的一次响应（按测试需要微调）。 */
function resourcesPayload(revision: number): any {
  return {
    session_id: 's1',
    session_key: 'agent:main:session:s1',
    revision,
    categories: {
      execution: { label: '执行态', order: 0, transient: true, status: 'ok', total: 2, summary: { running: true, turn: { turn_started_ms: Date.now() - 125_000, current_tool: { name: 'exec', started_ms: 2 } }, steer_pending: 1, followup_pending: 0 } },
      approvals_pending: { label: '待审批', order: 10, transient: true, status: 'ok', total: 1, summary: { rows: [{ session_key: 'agent:main:session:s1', pattern: 'git push *' }], omitted: 0 } },
      questions_pending: { label: '待提问', order: 20, transient: true, status: 'empty', total: 0 },
      hooks_session: { label: '会话钩子', order: 30, transient: false, status: 'empty', total: 0 },
      hooks_global: { label: '全局钩子', order: 40, transient: false, status: 'empty', total: 0 },
      cron: { label: '定时任务', order: 50, transient: false, status: 'empty', total: 0 },
      background: { label: '后台进程', order: 60, transient: false, status: 'empty', total: 0 },
      subagents: { label: '子代理', order: 70, transient: false, status: 'empty', total: 0 },
      durable_loops: { label: '常驻目标', order: 80, transient: false, status: 'ok', total: 0, summary: { loops: [], omitted: 0, active: 0 } },
      todos: { label: 'Todo', order: 90, transient: false, status: 'empty', total: 0 },
      file_changes: { label: '文件变更', order: 100, transient: false, status: 'ok', total: 2, summary: { rows: [{ path: 'a.rs', kind: 'write' }, { path: 'b.rs', kind: 'edit' }], omitted: 0 } },
      checkpoint: { label: '检查点', order: 110, transient: false, status: 'empty', total: 0 },
      context: { label: '上下文', order: 120, transient: false, status: 'unavailable', reason: '上下文快照不可用' },
      usage: { label: '用量', order: 130, transient: false, status: 'ok', total: 1, summary: { requests: 3, input_tokens: 100, output_tokens: 50, total_cost_usd: 0.02 } },
      session_allow: { label: '全部放行', order: 140, transient: false, status: 'ok', total: 1, summary: { enabled: true, since: null, auto_approved: 3 } },
      identity: { label: '会话信息', order: 200, transient: false, status: 'ok', total: 1, summary: { title: '修 bug 会话', project_path: 'C:/proj/a' } },
    },
  }
}

async function mountPanel(sessionId: string | null, payload = resourcesPayload(1)) {
  requestMock.mockImplementation(async (mod: string, cmd: string) => {
    if (mod === 'session' && cmd === 'resources') return payload
    if (mod === 'system' && cmd === 'status') return { workspace: 'C:/ws' }
    return {}
  })
  const wrapper = mount(SessionResourcesPanel, { props: { sessionId } })
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  pushMock.mockReset()
  setAllowMock.mockReset().mockResolvedValue(undefined)
  for (const k of Object.keys(sseHandlers)) delete sseHandlers[k]
})

afterEach(() => {
  vi.useRealTimers()
})

describe('渲染分区', () => {
  it('identity 头部：标题 + 项目 + 工作目录（system.status）', async () => {
    const w = await mountPanel('s1')
    expect(w.find('.srp-title').text()).toBe('修 bug 会话')
    expect(w.text()).toContain('C:/proj/a')
    expect(w.text()).toContain('C:/ws')
    expect(requestMock).toHaveBeenCalledWith('system', 'status')
    w.unmount()
  })

  it('#7 面板级 ✕ 住 header 行（与 ＋项目 同排），会话工具卡头无 ✕', async () => {
    const w = await mountPanel('s1')
    // ✕ 是面板级动作：必须在 .srp-header 内、与 ＋项目 同排。
    const header = w.find('.srp-header')
    expect(header.exists()).toBe(true)
    const closeInHeader = header.findAll('.srp-tools-close')
    expect(closeInHeader.length, 'header 行必须恰好一个 ✕').toBe(1)
    expect(header.text()).toContain('＋项目')
    // 卡头（.srp-card-head）不再是关闭按钮宿主。
    const closeInCards = w.findAll('.srp-card-head .srp-tools-close')
    expect(closeInCards.length, '业务卡头不得再有面板级 ✕').toBe(0)
    closeInHeader[0].trigger('click')
    await flushPromises()
    expect(w.emitted('close')).toHaveLength(1)
    w.unmount()
  })

  it('执行态大卡：running 点 + 当前工具名 + 插队徽标 + ⏱ 计时徽标', async () => {
    const w = await mountPanel('s1')
    expect(w.find('.srp-exec').exists()).toBe(true)
    expect(w.find('.srp-dot').classes()).toContain('running')
    expect(w.text()).toContain('执行中：exec')
    expect(w.text()).toContain('插队 1')
    // ⏱ 计时徽标（2026-11-24）：锚 turn_started_ms = now-125s → 2m05s。
    const elapsed = w.find('.srp-exec-elapsed')
    expect(elapsed.exists()).toBe(true)
    expect(elapsed.text()).toContain('2m05s')
    w.unmount()
  })

  it('执行态 ⏱ 计时：随时间走表；空闲/turn 缺失诚实不渲染', async () => {
    vi.useFakeTimers()
    const w = await mountPanel('s1')
    expect(w.find('.srp-exec-elapsed').text()).toContain('2m05s')
    await vi.advanceTimersByTimeAsync(60_000)
    expect(w.find('.srp-exec-elapsed').text()).toContain('3m05s')
    w.unmount()

    // 空闲（running=false，无 turn 数据）：无计时徽标。
    const idle = resourcesPayload(2)
    idle.categories.execution.summary = { running: false, steer_pending: 1, followup_pending: 0 }
    const w2 = await mountPanel('s1', idle)
    expect(w2.find('.srp-exec-elapsed').exists()).toBe(false)
    expect(w2.text()).toContain('空闲')
    w2.unmount()

    // turn_started_ms 缺失/非法（后端时钟异常锚 0 等）：诚实不渲染，不显示荒唐值。
    const bad = resourcesPayload(3)
    bad.categories.execution.summary = { running: true, turn: { turn_started_ms: 0 }, steer_pending: 0, followup_pending: 0 }
    const w3 = await mountPanel('s1', bad)
    expect(w3.find('.srp-exec-elapsed').exists()).toBe(false)
    w3.unmount()
  })

  it('在途区：待审批行渲染，红线——面板内无审批交互按钮', async () => {
    const w = await mountPanel('s1')
    expect(w.find('.srp-pending').exists()).toBe(true)
    expect(w.text()).toContain('git push *')
    // 红线：审批交互走既有弹窗链路，面板内绝不出现批准/拒绝按钮。
    const btnTexts = w.findAll('button').map(b => b.text())
    expect(btnTexts.some(t => t.includes('批准') || t.includes('拒绝'))).toBe(false)
    w.unmount()
  })

  it('资源卡流按 order 排 + file_changes/usage/session_allow 内容渲染', async () => {
    const w = await mountPanel('s1')
    // 卡流 = 排除会话工具卡 / 执行态大卡 / 在途区 / CTA 空态小卡后的正文卡。
    const cards = w.findAll('.srp-card').filter(c =>
      !c.classes().includes('srp-tools')
      && !c.classes().includes('srp-exec')
      && !c.classes().includes('srp-pending')
      && !c.text().includes('前往'))
    const titles = cards.map(c => c.find('.srp-card-title').text())
    // ok 类目 order：durable_loops(80) < file_changes(100) < usage(130) < session_allow(140)
    expect(titles).toEqual(['常驻目标', '文件变更', '用量', '全部放行'])
    expect(w.text()).toContain('a.rs')
    expect(w.text()).toContain('$0.0200')
    const cb = w.find('.srp-switch-row input[type="checkbox"]')
    expect((cb.element as HTMLInputElement).checked).toBe(true)
    w.unmount()
  })

  it('空徽标流（cron/hooks_global 走 CTA 空态卡，不进徽标）+ Unavailable ⚠', async () => {
    const w = await mountPanel('s1')
    // empty 且非 CTA 类目：questions/hooks_session/background/subagents/todos/checkpoint = 6
    expect(w.find('.srp-empty-row').text()).toContain('无资源 6 项')
    // CTA 空态卡两张（cron / hooks_global）。
    const ctaCards = w.findAll('.srp-card').filter(c => c.text().includes('前往'))
    expect(ctaCards.length).toBe(2)
    w.find('.srp-empty-row').trigger('click')
    await flushPromises()
    expect(w.findAll('.srp-empty-detail .srp-chip').length).toBe(6)
    // Unavailable ⚠ 行：reason 诚实透传。
    expect(w.find('.srp-unavail').text()).toContain('上下文快照不可用')
    w.unmount()
  })

  it('未装配诚实错误：request reject → 错误行渲染', async () => {
    requestMock.mockRejectedValue('资源盘点未装配（本网关形态未启用）')
    const w = mount(SessionResourcesPanel, { props: { sessionId: 's1' } })
    await flushPromises()
    expect(w.find('.srp-error').text()).toContain('资源盘点未装配')
    w.unmount()
  })

  // 明细行（2026-11-19「只有个数不知道是啥」补齐）：事件·类型 + 内容
  // 预览，长内容头截断，悬浮 title 带 matcher/timeout。
  it('hooks_session 明细行：事件·类型 + 内容（prompt/command 两形态 + 头截断）', async () => {
    const p = resourcesPayload(1)
    p.categories.hooks_session = {
      label: '会话钩子', order: 30, transient: false, status: 'ok', total: 2,
      summary: {
        count: 2,
        rows: [
          { event: 'Stop', kind: 'prompt', content: '现在几点了', matcher: '', timeout: null },
          { event: 'PreToolUse', kind: 'command', content: 'echo ' + 'x'.repeat(60), matcher: 'Bash', timeout: 30 },
        ],
      },
    }
    const w = await mountPanel('s1', p)
    expect(w.text()).toContain('本会话钩子 2 个')
    expect(w.text()).toContain('Stop·提示词')
    expect(w.text()).toContain('现在几点了')
    expect(w.text()).toContain('PreToolUse·命令')
    // 60 字符命令被头截到 48 + 省略号（全文在 title 悬浮可见）。
    const truncated = w.findAll('.srp-row').map(r => r.text()).join('\n')
    expect(truncated).toContain('…')
    const longRow = w.findAll('.srp-row').find(r => r.text().includes('PreToolUse'))
    const title = longRow?.find('span[title]')?.attributes('title')
    expect(title).toContain('matcher=Bash')
    expect(title).toContain('timeout=30s')
    w.unmount()
  })

  // 回归锁（2026-11-17 自查修复）：session_allow 是状态开关不是资源清单
  // ——empty（未开启=常态）也必须渲染开关卡，只 ok 可见 = 面板上没有
  // 开启入口（死锁）；且不进「无资源」徽标流。
  it('session_allow empty 态仍渲染开关（未勾选）且不进空徽标流', async () => {
    const p = resourcesPayload(1)
    p.categories.session_allow = { label: '全部放行', order: 140, transient: false, status: 'empty', total: 0 }
    const w = await mountPanel('s1', p)
    const cb = w.find('.srp-switch-row input[type="checkbox"]')
    expect(cb.exists()).toBe(true)
    expect((cb.element as HTMLInputElement).checked).toBe(false)
    expect(w.find('.srp-empty-row').text()).not.toContain('全部放行')
    // 开关可开：change → setSessionAllow(sid, true)。
    await cb.setValue(true)
    await flushPromises()
    expect(setAllowMock).toHaveBeenCalledWith('s1', true)
    w.unmount()
  })
})

describe('等待队列气泡（2026-11-24）', () => {
  /** 带 waiting_queue ok 类目的响应（插队 1 + 排队 2，末条长文本测预览截断）。 */
  function wqPayload(): any {
    const p = resourcesPayload(5)
    p.categories.waiting_queue = {
      label: '等待队列',
      order: 5,
      transient: true,
      status: 'ok',
      total: 3,
      summary: {
        steer: [
          { content: '!帮我把测试跑一遍然后把结果发给我', sender_id: 'web', timestamp: '14:32:05', priority: 'normal' },
        ],
        follow_up: [
          { content: '顺便看下 CI 为什么红了', sender_id: 'web', timestamp: '14:35:00', priority: 'normal' },
          { content: '再'.repeat(60), sender_id: 'web', timestamp: '14:36:00', priority: 'high' },
        ],
        capacity: 8,
      },
    }
    return p
  }

  it('有等待才渲染：插队在上、排队在下（组内序 = 真实执行序），徽标计数 + 行预览截断', async () => {
    const w = await mountPanel('s1', wqPayload())
    const card = w.find('.srp-queue')
    expect(card.exists()).toBe(true)
    expect(card.text()).toContain('插队 1')
    expect(card.text()).toContain('排队 2')
    const keys = card.findAll('.srp-key')
    expect(keys.length).toBe(3)
    expect(keys[0].text()).toBe('⚡')
    expect(keys[1].text()).toBe('⏳')
    expect(keys[2].text()).toBe('⏳')
    // 行预览 48 字符截断（第三条 60 字符 → 48 + 省略号）；全文只在弹层。
    const previews = card.findAll('.srp-wq-preview')
    expect(previews[2].text()).toBe(`${'再'.repeat(48)}…`)
    w.unmount()
  })

  it('hover 行出全文弹层（fixed 挂根级）：全文 + meta；离开延迟隐藏、悬入弹层取消', async () => {
    vi.useFakeTimers()
    const w = await mountPanel('s1', wqPayload())
    expect(w.find('.srp-wq-pop').exists()).toBe(false)
    await w.findAll('.srp-wq-row')[0].trigger('mouseenter')
    const pop = w.find('.srp-wq-pop')
    expect(pop.exists()).toBe(true)
    expect(pop.text()).toContain('!帮我把测试跑一遍然后把结果发给我')
    expect(pop.text()).toContain('插队 · 14:32:05')
    // 行离开：hover-intent 延迟 200ms 再藏（防指针移入弹层时闪烁）。
    await w.findAll('.srp-wq-row')[0].trigger('mouseleave')
    expect(w.find('.srp-wq-pop').exists()).toBe(true)
    await vi.advanceTimersByTimeAsync(250)
    expect(w.find('.srp-wq-pop').exists()).toBe(false)
    // 悬入弹层取消隐藏：行离开 → 弹层悬入 → 越过延迟窗仍不消失。
    await w.findAll('.srp-wq-row')[1].trigger('mouseenter')
    expect(w.find('.srp-wq-pop').exists()).toBe(true)
    await w.findAll('.srp-wq-row')[1].trigger('mouseleave')
    await w.find('.srp-wq-pop').trigger('mouseenter')
    await vi.advanceTimersByTimeAsync(250)
    expect(w.find('.srp-wq-pop').exists()).toBe(true)
    // 弹层自身离开即时藏。
    await w.find('.srp-wq-pop').trigger('mouseleave')
    expect(w.find('.srp-wq-pop').exists()).toBe(false)
    w.unmount()
  })

  it('无等待（empty）不渲染气泡且不进空徽标流', async () => {
    const p = resourcesPayload(5)
    p.categories.waiting_queue = { label: '等待队列', order: 5, transient: true, status: 'empty', total: 0 }
    const w = await mountPanel('s1', p)
    expect(w.find('.srp-queue').exists()).toBe(false)
    const chips = w.findAll('.srp-chip').map(c => c.text())
    expect(chips).not.toContain('等待队列')
    w.unmount()
  })

  it('ok 态无双渲染：等待队列不进通用资源卡流', async () => {
    const w = await mountPanel('s1', wqPayload())
    expect(w.findAll('.srp-queue').length).toBe(1)
    const genericTitles = w.findAll('.srp-card-head.clickable .srp-card-title').map(t => t.text())
    expect(genericTitles).not.toContain('等待队列')
    w.unmount()
  })
})

describe('C1 subagents 专用渲染', () => {
  function subagentsPayload(): any {
    const p = resourcesPayload(1)
    p.categories.subagents = {
      label: '子代理', order: 70, transient: false, status: 'ok', total: 3,
      summary: {
        running: 1,
        finished_total: 2,
        rows: [
          { id: 't1', task: '跑长任务 A', role: 'dev', status: 'running', background: true, bg_task_id: 'bg-9', started_at_ms: Date.now() - 5000, finished_at_ms: null, summary: null, error: null },
          { id: 't2', task: '分析模块 B', role: 'dev', status: 'done', background: false, bg_task_id: null, started_at_ms: Date.now() - 60000, finished_at_ms: Date.now() - 30000, summary: '模块 B 分析完成：3 处疑点', error: null },
          { id: 't3', task: '重构 C', role: 'dev', status: 'failed', background: false, bg_task_id: null, started_at_ms: Date.now() - 90000, finished_at_ms: Date.now() - 60000, summary: null, error: '编译错误：E0382' },
        ],
        omitted: 5,
      },
    }
    return p
  }

  function subagentsCard(w: ReturnType<typeof mount>): any {
    const card = w.findAll('.srp-card').find(c => c.find('.srp-card-title').text() === '子代理')
    expect(card, 'subagents 卡必须在资源卡流中').toBeTruthy()
    return card!
  }

  it('卡头徽标 = 运行中 N / 历史 M；行渲染 ⏳/✅/❌ + summary/error，无 raw dump', async () => {
    vi.useFakeTimers()
    const w = await mountPanel('s1', subagentsPayload())
    const card = subagentsCard(w)
    // 卡头计数（新 summary 契约：running + finished_total）。
    expect(card.find('.srp-badge').text()).toBe('运行中 1 / 历史 2')
    const rows = card.findAll('.subagent-row')
    expect(rows).toHaveLength(3)
    // running 行：⏳ + running 状态点 + live 用时。
    expect(rows[0].find('.subagent-icon').text()).toBe('⏳')
    expect(rows[0].find('.srp-dot').classes()).toContain('running')
    expect(rows[0].find('.subagent-meta.live').text()).toContain('运行中')
    expect(rows[0].find('.subagent-meta.live').text()).toContain('5s')
    // done 行：✅ + summary 摘录。
    expect(rows[1].find('.subagent-icon').text()).toBe('✅')
    expect(rows[1].find('.subagent-meta.ok').text()).toBe('模块 B 分析完成：3 处疑点')
    // failed 行：❌ + error 原文。
    expect(rows[2].find('.subagent-icon').text()).toBe('❌')
    expect(rows[2].find('.subagent-meta.err').text()).toBe('编译错误：E0382')
    // 红线（计划 C 批）：raw 字段 dump 禁令——行文本不得出现字段名=值形态。
    expect(card.text()).not.toContain('bg_task_id=')
    expect(card.text()).not.toContain('background=')
    expect(card.text()).not.toContain('started_at_ms=')
    // omitted 提示。
    expect(card.text()).toContain('…还有 5 条历史')
    w.unmount()
  })

  it('running 实时用时随时间走表（nowTick 驱动）', async () => {
    vi.useFakeTimers()
    const w = await mountPanel('s1', subagentsPayload())
    expect(w.find('.subagent-meta.live').text()).toContain('5s')
    await vi.advanceTimersByTimeAsync(60000)
    expect(w.find('.subagent-meta.live').text()).toContain('1m05s')
    w.unmount()
  })

  it('ok 态 rows 空 → 「暂无子代理」兜底（徽标 0/0）', async () => {
    const p = resourcesPayload(1)
    p.categories.subagents = { label: '子代理', order: 70, transient: false, status: 'ok', total: 0, summary: { running: 0, finished_total: 0, rows: [], omitted: 0 } }
    const w = await mountPanel('s1', p)
    const card = subagentsCard(w)
    expect(card.text()).toContain('暂无子代理')
    expect(card.find('.srp-badge').text()).toBe('运行中 0 / 历史 0')
    w.unmount()
  })

  // 回归钉：C1 不改空态分类——subagents empty 仍进「无资源」徽标流。
  it('empty 态不破空徽标计数（仍是 6 项）', async () => {
    const w = await mountPanel('s1')
    expect(w.find('.srp-empty-row').text()).toContain('无资源 6 项')
    w.unmount()
  })
})

describe('CTA 矩阵', () => {
  it('durable_loops 空态：内联表单提交透传 goal + cron 到 loops.create', async () => {
    const w = await mountPanel('s1')
    await w.find('.srp-cta').trigger('click') // 「＋ 设置目标」
    expect(w.find('.srp-form').exists()).toBe(true)
    const inputs = w.findAll('.srp-input')
    await inputs[0].setValue('盯着 CI')
    await inputs[1].setValue('0 9 * * *')
    await w.findAll('.srp-cta').filter(b => b.text() === '创建')[0].trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('loops', 'create', {
      goal: '盯着 CI',
      session_id: 's1',
      cron: '0 9 * * *',
    })
    w.unmount()
  })

  it('durable_loops 空态：delay_secs 入参（无 cron 时）', async () => {
    const w = await mountPanel('s1')
    await w.find('.srp-cta').trigger('click')
    const inputs = w.findAll('.srp-input')
    await inputs[0].setValue('清理临时目录')
    await inputs[2].setValue('300')
    await w.findAll('.srp-cta').filter(b => b.text() === '创建')[0].trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('loops', 'create', {
      goal: '清理临时目录',
      session_id: 's1',
      delay_secs: 300,
    })
    w.unmount()
  })

  it('loops.create 失败诚实显示错误不关表单', async () => {
    const w = await mountPanel('s1')
    await w.find('.srp-cta').trigger('click')
    const inputs = w.findAll('.srp-input')
    await inputs[0].setValue('x')
    requestMock.mockRejectedValueOnce('排程无效：bad cron')
    await w.findAll('.srp-cta').filter(b => b.text() === '创建')[0].trigger('click')
    await flushPromises()
    expect(w.find('.srp-sub.err').exists()).toBe(true)
    expect(w.find('.srp-form').exists()).toBe(true)
    w.unmount()
  })

  it('session_allow 开关：change → setSessionAllow(sid, false)', async () => {
    const w = await mountPanel('s1')
    await w.find('.srp-switch-row input[type="checkbox"]').setValue(false)
    await flushPromises()
    expect(setAllowMock).toHaveBeenCalledWith('s1', false)
    w.unmount()
  })

  it('cron/hooks_global 空态卡跳转 /tasks、/hooks', async () => {
    const w = await mountPanel('s1')
    const ctaBtns = w.findAll('button').filter(b => b.text().includes('前往'))
    expect(ctaBtns.length).toBe(2)
    await ctaBtns.filter(b => b.text().includes('定时任务'))[0].trigger('click')
    await ctaBtns.filter(b => b.text().includes('Hooks'))[0].trigger('click')
    expect(pushMock).toHaveBeenCalledWith('/tasks')
    expect(pushMock).toHaveBeenCalledWith('/hooks')
    w.unmount()
  })
})

describe('B4 活态刷新', () => {
  beforeEach(() => { vi.useFakeTimers() })

  it('tool_event 帧本会话命中 → debounce 后重拉；异会话帧不重拉', async () => {
    const w = await mountPanel('s1')
    const calls0 = requestMock.mock.calls.filter((c: any[]) => c[0] === 'session').length
    const h = wsHandler()
    h({ type: 'push', cmd: 'tool_event', data: { kind: 'ToolStarted', data: { session_id: 's1', chat_id: 'web:s1' } } })
    await vi.advanceTimersByTimeAsync(399)
    expect(requestMock.mock.calls.filter((c: any[]) => c[0] === 'session').length).toBe(calls0)
    await vi.advanceTimersByTimeAsync(1)
    expect(requestMock.mock.calls.filter((c: any[]) => c[0] === 'session').length).toBeGreaterThan(calls0)

    // 异会话帧：不触发。
    const calls1 = requestMock.mock.calls.filter((c: any[]) => c[0] === 'session').length
    h({ type: 'push', cmd: 'tool_event', data: { kind: 'ToolStarted', data: { session_id: 'other' } } })
    await vi.advanceTimersByTimeAsync(1000)
    expect(requestMock.mock.calls.filter((c: any[]) => c[0] === 'session').length).toBe(calls1)
    w.unmount()
  })

  it('旧帧无 session_id：chat_id `web:{sid}` 兜底命中', async () => {
    const w = await mountPanel('s1')
    const calls0 = requestMock.mock.calls.filter((c: any[]) => c[0] === 'session').length
    wsHandler()({ type: 'push', cmd: 'tool_event', data: { kind: 'TodoUpdated', data: { chat_id: 'web:s1', todos: [] } } })
    await vi.advanceTimersByTimeAsync(500)
    expect(requestMock.mock.calls.filter((c: any[]) => c[0] === 'session').length).toBe(calls0 + 1)
    w.unmount()
  })

  it('SSE approval/question 四事件：session_key 匹配重拉、不匹配忽略', async () => {
    const w = await mountPanel('s1')
    const calls0 = requestMock.mock.calls.filter((c: any[]) => c[0] === 'session').length
    const evt = { session_key: 'agent:main:session:s1' }
    for (const type of ['approval-requested', 'approval-resolved', 'question-asked', 'question-resolved']) {
      expect(sseHandlers[type]?.length, `SSE ${type} 应有订阅`).toBeGreaterThan(0)
      sseHandlers[type][0](evt)
    }
    await vi.advanceTimersByTimeAsync(500)
    const calls1 = requestMock.mock.calls.filter((c: any[]) => c[0] === 'session').length
    expect(calls1).toBeGreaterThan(calls0)

    // 异会话 session_key：不触发。
    const calls2 = requestMock.mock.calls.filter((c: any[]) => c[0] === 'session').length
    for (const type of ['approval-requested', 'question-asked']) {
      sseHandlers[type][0]({ session_key: 'agent:main:session:zzz' })
    }
    await vi.advanceTimersByTimeAsync(1000)
    expect(requestMock.mock.calls.filter((c: any[]) => c[0] === 'session').length).toBe(calls2)
    w.unmount()
  })

  it('revision 乱序丢弃：旧 revision 回包不覆盖新视图', async () => {
    const w = await mountPanel('s1', resourcesPayload(5))
    // 视图当前是 revision 5 的内容。
    expect(w.text()).toContain('修 bug 会话')
    // WS 帧触发重拉，回包是旧 revision 3 → 应用层丢弃。
    requestMock.mockImplementation(async (mod: string, cmd: string) => {
      if (mod === 'session' && cmd === 'resources') return resourcesPayload(3)
      if (mod === 'system' && cmd === 'status') return { workspace: 'C:/ws' }
      return {}
    })
    wsHandler()({ type: 'push', cmd: 'tool_event', data: { kind: 'ToolStarted', data: { session_id: 's1' } } })
    await vi.advanceTimersByTimeAsync(500)
    await flushPromises()
    // 视图仍是 revision 5 数据（标题仍在）；后续 revision 6 正常应用。
    requestMock.mockImplementation(async (mod: string, cmd: string) => {
      if (mod === 'session' && cmd === 'resources') {
        const p = resourcesPayload(6)
        p.categories.identity.summary.title = '新标题'
        return p
      }
      if (mod === 'system' && cmd === 'status') return { workspace: 'C:/ws' }
      return {}
    })
    wsHandler()({ type: 'push', cmd: 'tool_event', data: { kind: 'ToolStarted', data: { session_id: 's1' } } })
    await vi.advanceTimersByTimeAsync(500)
    await flushPromises()
    expect(w.text()).toContain('新标题')
    w.unmount()
  })

  it('WS 重连归零 revision 高水位：跨 gateway 重启不冻结（2026-11-19 修「钩子设了面板不显示」）', async () => {
    // 旧进程代拿到高水位 revision=500；gateway 重启后新进程 seq 归零，
    // 回包 revision=3（带会话钩子变化）。无归零修复时 3 < 500 恒被当
    // 乱序旧包丢弃 = 面板永久冻结在重启前数据。
    const w = await mountPanel('s1', resourcesPayload(500))
    expect(w.text()).toContain('修 bug 会话')
    const p3 = resourcesPayload(3)
    p3.categories.hooks_session = { label: '会话钩子', order: 30, transient: false, status: 'ok', total: 1, summary: { count: 1 } }
    requestMock.mockImplementation(async (mod: string, cmd: string) => {
      if (mod === 'session' && cmd === 'resources') return p3
      if (mod === 'system' && cmd === 'status') return { workspace: 'C:/ws' }
      return {}
    })
    wsStatus.value = 'disconnected'
    await vi.advanceTimersByTimeAsync(10)
    wsStatus.value = 'connected'
    await vi.advanceTimersByTimeAsync(10)
    await flushPromises()
    expect(w.text()).toContain('本会话钩子 1 个')
    w.unmount()
  })

  it('SSE resync 归零 revision 高水位：缺口提示后旧代回包照常应用', async () => {
    const w = await mountPanel('s1', resourcesPayload(500))
    expect(w.text()).toContain('修 bug 会话')
    const p3 = resourcesPayload(3)
    p3.categories.hooks_session = { label: '会话钩子', order: 30, transient: false, status: 'ok', total: 1, summary: { count: 1 } }
    requestMock.mockImplementation(async (mod: string, cmd: string) => {
      if (mod === 'session' && cmd === 'resources') return p3
      if (mod === 'system' && cmd === 'status') return { workspace: 'C:/ws' }
      return {}
    })
    expect(sseHandlers['resync']?.length, 'SSE resync 应有订阅').toBeGreaterThan(0)
    sseHandlers['resync'][0](undefined)
    await flushPromises()
    expect(w.text()).toContain('本会话钩子 1 个')
    w.unmount()
  })

  it('30s 兜底轮询 + 会话切换清态（旧回包丢弃）', async () => {
    const w = await mountPanel('s1')
    const calls0 = requestMock.mock.calls.filter((c: any[]) => c[0] === 'session').length
    await vi.advanceTimersByTimeAsync(30000)
    expect(requestMock.mock.calls.filter((c: any[]) => c[0] === 'session').length)
      .toBeGreaterThan(calls0)

    // 切会话：清态重拉新 sid；挂起的旧回包落地不覆盖。
    requestMock.mockImplementation(() => new Promise(() => {}))
    await w.setProps({ sessionId: 's2' })
    await flushPromises()
    // 切换即发新拉取（fetchResources 先于 fetchWorkdir，两者都在场即可）。
    expect(requestMock).toHaveBeenCalledWith('session', 'resources', { session_id: 's2' })
    expect(requestMock).toHaveBeenCalledWith('system', 'status')
    expect(w.find('.srp-root').text()).not.toContain('修 bug 会话')
    w.unmount()
  })
})

// -------------------------------------------------------------------------
// 发送方式卡（2026-11-24 三键迁移资源窗）：立即/插队/排队三键，状态真值
// chat store sendMode（与 ChatPanel 发送链 applySendMode 同源——此处切换
// 即时生效）；「立即」忙碌字面降级（BUG-1 同源口径，busy 读 store
// busyBySid 实时）；reject（queueEnabled=false）卡片整体不渲染。
// 本 spec 未 mock useInboxStatus——requestMock 对 agent.inbox_status 返
// {} → 快照 available 缺席 → mode 兜底 'steer'（开放态），三键全渲染。
describe('发送方式卡（三键迁移）', () => {
  // wrapper 结构走 any（VueWrapper 泛型在此处无谓纠缠；DOMWrapper 数组
  // 上的 find/findAll/text 链直接消费即可，本文件 resourcesPayload 同款口径）。
  function findSendModeCard(w: any) {
    return w.findAll('.srp-card').find((c: any) => c.find('.srp-card-title').text() === '发送方式')
  }

  it('三键渲染：默认「立即」active；点击写 chat store（与 ChatPanel 发送链同源）', async () => {
    const w = await mountPanel('s1')
    const card = findSendModeCard(w)
    expect(card, '发送方式卡必须渲染（快照缺席 → steer 兜底 → queueEnabled）').toBeTruthy()
    const btns = card!.findAll('.srp-tool-btn')
    expect(btns.map((b: { text: () => string }) => b.text())).toEqual(['立即', '⚡插队', '⏭排队'])
    expect(btns[0].classes()).toContain('active')
    const chat = useChatStore()
    expect(chat.sendMode).toBe('now')
    await btns[1].trigger('click')
    expect(chat.sendMode).toBe('steer')
    expect(btns[1].classes()).toContain('active')
    expect(btns[0].classes()).not.toContain('active')
    await btns[2].trigger('click')
    expect(chat.sendMode).toBe('queue')
    await btns[0].trigger('click')
    expect(chat.sendMode).toBe('now')
    w.unmount()
  })

  it('busy 字面降级：「立即」↔「立即→插队」（快照缺席兜底 steer 模式）', async () => {
    const chat = useChatStore()
    const w = await mountPanel('s1')
    const nowBtn = findSendModeCard(w)!.findAll('.srp-tool-btn')[0]
    expect(nowBtn.text()).toBe('立即')
    chat.setBusy('s1', true)
    await w.vm.$nextTick()
    expect(nowBtn.text()).toBe('立即→插队')
    chat.setBusy('s1', false)
    await w.vm.$nextTick()
    expect(nowBtn.text()).toBe('立即')
    w.unmount()
  })

  it('reject 模式（显式快照 mode=reject）：queueEnabled=false → 卡片整体不渲染', async () => {
    requestMock.mockImplementation(async (mod: string, cmd: string) => {
      if (mod === 'session' && cmd === 'resources') return resourcesPayload(1)
      if (mod === 'system' && cmd === 'status') return { workspace: 'C:/ws' }
      if (mod === 'agent' && cmd === 'inbox_status') {
        return {
          available: true,
          session_key: 'agent:main:session:s1',
          next_turn: 0,
          next_step: 0,
          capacity: 8,
          busy: false,
          mode: 'reject',
        }
      }
      return {}
    })
    const w = mount(SessionResourcesPanel, { props: { sessionId: 's1' } })
    await flushPromises()
    expect(w.findAll('.srp-card').find((c) => c.find('.srp-card-title').text() === '发送方式'))
      .toBeUndefined()
    w.unmount()
  })
})
