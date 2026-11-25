// 洞察台账页（自学习铺路 · 洞察页与场景拆分 2026-11-24）：
// - 实效统计卡：候选/消化/状态计数 + 纠正率前后窗对照（双窗在场才渲染 delta）
// - 场景过滤：全部/对话/各项目（project_id → 项目注册表名字解析，缺席=原始 id）
// - 条目表：状态徽标 + 按 status 变化的操作面（晋升/退役/否决）
// - 触发学习对话：learn.run → 空原因/定稿计数 toast + 台账刷新
// 后端白名单面由 handlers/forge 钉住；本 spec 钉前端交互与载荷形态。
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { createPinia } from 'pinia'
import { useToast } from '../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

// session store 走 useChatApi.listProjects 拉项目注册表——mock 掉拿到
// 受控项目表（fetchProjects 失败静默是 store 纪律，但这里直接给数据）。
const listProjectsMock = vi.fn()
vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({ listProjects: (...args: any[]) => listProjectsMock(...args) }),
}))

import ForgeView from '../../views/ForgeView.vue'

const PROJECTS = [
  { id: 'prj-1', name: '网关重构', path: 'D:/w/a', created_at: '', running: true },
  { id: 'prj-2', name: '皮肤工坊', path: 'D:/w/b', created_at: '', running: false },
]

function entry(over: Record<string, unknown> = {}) {
  return {
    id: 'ins-20261124-1',
    fingerprint: 'fp1',
    statement: '停下询问前先自己尝试可行路径',
    category: 'behavior',
    evidence: '候选 cand-1：问授权被纠正',
    source: 'dialogue',
    status: 'Candidate',
    confidence: 0.9,
    occurrences: 1,
    first_seen: '2026-11-24T10:00:00+08:00',
    last_seen: '2026-11-24T10:00:00+08:00',
    // scope 序列化 skip_if_none：对话条目无此键。
    ...over,
  }
}

function statsImpl(entries: any[], stats: any = null) {
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'insights.list') return Promise.resolve({ entries, stats })
    return Promise.resolve({})
  })
}

beforeEach(() => {
  requestMock.mockReset()
  listProjectsMock.mockReset().mockResolvedValue({ projects: PROJECTS })
  useToast().toasts.splice(0)
})

async function openInsights(w: ReturnType<typeof mount>) {
  await w.findAll('button.tab').find((b) => b.text() === '洞察')!.trigger('click')
  await flushPromises()
}

function mountView() {
  return mount(ForgeView, { global: { plugins: [createPinia()] } })
}

describe('ForgeView 洞察页', () => {
  it('切页拉台账 + 项目注册表；空台账 = 空态文案', async () => {
    statsImpl([])
    const w = mountView()
    await flushPromises()
    await openInsights(w)
    expect(requestMock).toHaveBeenCalledWith('forge', 'insights.list', {})
    expect(listProjectsMock).toHaveBeenCalled()
    expect(w.text()).toContain('暂无洞察条目')
    w.unmount()
  })

  it('条目表渲染：状态徽标 / 场景徽标（对话 vs 项目·名字）/ 置信百分比', async () => {
    statsImpl([
      entry(),
      entry({
        id: 'ins-20261124-2',
        statement: '构建前先看 CI 配置',
        status: 'Active',
        scope: 'prj-1',
        confidence: 0.75,
        occurrences: 3,
      }),
      entry({ id: 'ins-20261124-3', statement: '孤儿项目 id', status: 'Retired', scope: 'prj-gone' }),
    ])
    const w = mountView()
    await flushPromises()
    await openInsights(w)
    const text = w.text()
    expect(text).toContain('停下询问前先自己尝试可行路径')
    expect(text).toContain('候选')
    expect(text).toContain('活跃')
    // 场景解析：注册表命中 → 项目名；缺席 → 原始 id。
    expect(text).toContain('项目·网关重构')
    expect(text).toContain('项目·prj-gone')
    expect(text).toContain('75.0%')
    w.unmount()
  })

  it('场景过滤：对话筛空 scope，项目筛精确 id', async () => {
    statsImpl([
      entry({ id: 'i1' }),
      entry({ id: 'i2', scope: 'prj-1' }),
      entry({ id: 'i3', scope: 'prj-2' }),
    ])
    const w = mountView()
    await flushPromises()
    await openInsights(w)
    const sel = w.find('select')
    const rows = () => w.findAll('tbody tr').length
    expect(rows()).toBe(3) // 全部
    await sel.setValue('dialogue')
    expect(rows()).toBe(1)
    await sel.setValue('prj-1')
    expect(rows()).toBe(1)
    expect(w.text()).toContain('停下询问前先自己尝试可行路径')
    w.unmount()
  })

  it('候选条目操作面：晋升 → insights.update {id, action:"promote"} → 台账刷新', async () => {
    statsImpl([entry()])
    const w = mountView()
    await flushPromises()
    await openInsights(w)
    const btn = w.findAll('button').find((b) => b.text() === '晋升')!
    await btn.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('forge', 'insights.update', {
      id: 'ins-20261124-1',
      action: 'promote',
    })
    expect(useToast().toasts.some((t) => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('活跃条目操作面：退役 / 否决在；候选条目无退役钮', async () => {
    statsImpl([
      entry({ id: 'i1', status: 'Candidate' }),
      entry({ id: 'i2', status: 'Active', statement: '活跃条目' }),
    ])
    const w = mountView()
    await flushPromises()
    await openInsights(w)
    const row = (stmt: string) => w.findAll('tbody tr').find((r) => r.text().includes(stmt))!
    expect(row('停下询问前先自己尝试可行路径').text()).not.toContain('退役')
    expect(row('停下询问前先自己尝试可行路径').text()).toContain('晋升')
    expect(row('活跃条目').text()).toContain('退役')
    expect(row('活跃条目').text()).not.toContain('晋升')
    // 退役载荷带 action=retire
    await row('活跃条目').findAll('button').find((b) => b.text() === '退役')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('forge', 'insights.update', {
      id: 'i2',
      action: 'retire',
    })
    w.unmount()
  })

  it('实效统计卡：双窗在场渲染纠正率 delta；无锚点渲染开窗引导', async () => {
    statsImpl([], {
      candidates_total: 12,
      candidates_consumed: 8,
      entries_candidate: 2,
      entries_active: 3,
      entries_retired: 1,
      window_days_before: 10,
      window_days_after: 5,
      corrections_before: 4,
      corrections_after: 1,
      correction_rate_before: 0.4,
      correction_rate_after: 0.2,
      generated_at: '2026-11-24T12:00:00+08:00',
    })
    const w = mountView()
    await flushPromises()
    await openInsights(w)
    const text = w.text()
    expect(text).toContain('候选总数')
    expect(text).toContain('纠正率对照')
    expect(text).toContain('-20.0%') // (0.2-0.4)*100，生效方向负值
    w.unmount()

    statsImpl([], {
      candidates_total: 0,
      entries_candidate: 0,
      entries_active: 0,
      entries_retired: 0,
      window_days_before: 0,
      window_days_after: 0,
    })
    const w2 = mountView()
    await flushPromises()
    await openInsights(w2)
    expect(w2.text()).toContain('暂无激活锚点')
    w2.unmount()
  })

  it('触发学习对话：learn.run 定稿计数 toast + 台账刷新', async () => {
    statsImpl([])
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'insights.list') return Promise.resolve({ entries: [], stats: null })
      if (cmd === 'learn.run')
        return Promise.resolve({
          triggered: true,
          phases_completed: 4,
          insights: [{ id: 'ins-x' }],
          skills: [],
          empty_reason: null,
        })
      return Promise.resolve({})
    })
    const w = mountView()
    await flushPromises()
    await openInsights(w)
    const learnBtns = w.findAll('button').filter((b) => b.text().includes('触发学习对话'))
    await learnBtns[0].trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('forge', 'learn.run', {})
    expect(
      useToast().toasts.some((t) => t.type === 'success' && t.message.includes('1 条洞察'))
    ).toBe(true)
    // learn.run 后台账刷新（第二次 insights.list 调用）
    const listCalls = requestMock.mock.calls.filter((c: any[]) => c[1] === 'insights.list')
    expect(listCalls.length).toBeGreaterThanOrEqual(2)
    w.unmount()
  })

  it('学习对话空收场（empty_reason）→ info toast 原文', async () => {
    statsImpl([])
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'insights.list') return Promise.resolve({ entries: [], stats: null })
      if (cmd === 'learn.run')
        return Promise.resolve({
          triggered: true,
          phases_completed: 1,
          insights: [],
          skills: [],
          empty_reason: '候选全是正常咨询',
        })
      return Promise.resolve({})
    })
    const w = mountView()
    await flushPromises()
    await openInsights(w)
    await w.findAll('button').filter((b) => b.text().includes('触发学习对话'))[0].trigger('click')
    await flushPromises()
    expect(
      useToast().toasts.some((t) => t.type === 'info' && t.message.includes('候选全是正常咨询'))
    ).toBe(true)
    w.unmount()
  })

  it('自动激活可见（BUG-4）：learn.run 带 activated → 成功 toast 带激活计数后缀', async () => {
    statsImpl([])
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'insights.list') return Promise.resolve({ entries: [], stats: null })
      if (cmd === 'learn.run')
        return Promise.resolve({
          triggered: true,
          phases_completed: 4,
          insights: [{ id: 'ins-a' }],
          skills: [],
          activated: ['ins-a', 'ins-b'],
          empty_reason: null,
        })
      return Promise.resolve({})
    })
    const w = mountView()
    await flushPromises()
    await openInsights(w)
    await w.findAll('button').filter((b) => b.text().includes('触发学习对话'))[0].trigger('click')
    await flushPromises()
    expect(
      useToast().toasts.some(
        (t) => t.type === 'success' && t.message.includes('自动激活 2 条')
      )
    ).toBe(true)
    w.unmount()
  })

  it('截断可见：learn.run 带 truncated → warn toast（未进包候选留待下一轮）', async () => {
    statsImpl([])
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'insights.list') return Promise.resolve({ entries: [], stats: null })
      if (cmd === 'learn.run')
        return Promise.resolve({
          triggered: true,
          phases_completed: 4,
          insights: [],
          skills: [],
          truncated: true,
          empty_reason: null,
        })
      return Promise.resolve({})
    })
    const w = mountView()
    await flushPromises()
    await openInsights(w)
    await w.findAll('button').filter((b) => b.text().includes('触发学习对话'))[0].trigger('click')
    await flushPromises()
    expect(
      useToast().toasts.some(
        (t) => t.type === 'warn' && t.message.includes('未进包的候选留待下一轮')
      )
    ).toBe(true)
    w.unmount()
  })
})
