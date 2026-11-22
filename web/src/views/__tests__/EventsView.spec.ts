import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { createPinia } from 'pinia'
import { useToast } from '../../composables/useToast'

// 事件规则表单化（2026-11-17，用户拍板「不能直接编辑脚本，必须给钩子样式的
// UI」）：字段级表单卡往返无字段丢失（显式形态深比对 / 缺省形态语义等价）、
// 未知 kind 降级原文编辑（不丢数据）、未知字段保真合并、新增默认模板、
// 必填/形态校验拦截（错误显示在保存钮旁、不发 save 请求）、编辑中轮询不覆盖、
// 会话 picker 落盘 preset 形态。Schema 真相源 = crates/nemesis-events/src/rules.rs
// （五族放行面 fs/webhook/proc/sys/probe；cron 已砍单）。后端行为由
// nemesis-events rules.rs tests 钉住。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import EventsView from '../EventsView.vue'

// 五族显式形态规则集（Optional 字段全部显式写出——深比对逐字节保真）。
const FULL_RULES = [
  {
    id: 'build-done',
    watch: { kind: 'fs', path: 'C:/logs/build', glob: '*.log', debounce_ms: 800, content_pattern: '(?i)fatal|panic' },
    deliver: { session_key: 'agent:main:session:s-1', prompt: '构建日志有更新：{paths}', cooldown_secs: 60, merge_window_secs: 5, max_per_hour: 10, enabled: true },
  },
  {
    id: 'ci-hook',
    watch: { kind: 'webhook', token: 's3cret' },
    deliver: { session_key: 'agent:main:session:s-1', prompt: '外部推送：{payload}', enabled: false },
  },
  {
    id: 'build-err',
    watch: { kind: 'proc', job_id: 7, pattern: '(?i)error|FAILED', notify_start: true },
    // enabled 恒输出（缺省语义扩展）：原文无此字段，保存后补 enabled:true（语义等价）。
    deliver: { session_key: 'agents:team:x', prompt: '后台任务 #{job_id} 命中：{line}', cooldown_secs: 0, enabled: true },
  },
  {
    id: 'cpu-hot',
    watch: { kind: 'sys', metric: 'cpu', above: 90.5, below: 80 },
    deliver: { session_key: 'agent:main:session:s-2', prompt: '{payload}', max_per_hour: 0, enabled: true },
  },
  {
    id: 'api-down',
    watch: { kind: 'probe', probe_type: 'http', target: 'https://api.example.com/health', interval_secs: 30, timeout_secs: 5 },
    deliver: { session_key: 'agent:main:session:s-2', prompt: '{payload}', enabled: true },
  },
]

// 缺省形态（无 enabled / notify_start / 全部数字字段缺省）——往返后语义等价
//（enabled 视为 true；Option 数字字段以 omit 表达缺省）。
const MINIMAL_RULES = [
  {
    id: 'mini-fs',
    watch: { kind: 'fs', path: 'C:/x' },
    deliver: { session_key: 'agent:main:session:s-1', prompt: 'p' },
  },
]

const UNKNOWN_KIND_RULES = [
  {
    id: 'future-kind',
    watch: { kind: 'quantum', spin: 'up', entangled: [1, 2] },
    deliver: { session_key: 'agent:main:session:s-1', prompt: 'q', custom_gate: 42 },
    topExtra: { keep: 'me' },
  },
]

const EXTRA_FIELDS_RULES = [
  {
    id: 'with-extra',
    topExtra: 'top-kept',
    watch: { kind: 'fs', path: 'C:/y', future_field: { a: 1 } },
    deliver: { session_key: 'agent:main:session:s-1', prompt: 'p', vendor_hint: 'v' },
  },
]

const SESSION_ROWS = [
  { id: 's-1', title: '运维会话', projectId: 'p1' },
  { id: 's-2', title: '日常监控' },
]

function baseImpl(): Record<string, any> {
  return {
    status: { stats: { delivered_events: 1 }, rules: [], pending: [] },
    'rules.get': { rules: [] },
    'rules.save': { warnings: [] },
    'cron.list': { jobs: [] },
  }
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  vi.useFakeTimers()
  requestMock.mockImplementation((m: string, cmd: string) => {
    if (m === 'sessions' && cmd === 'list') return Promise.resolve({ sessions: SESSION_ROWS, bindings: {} })
    if (m === 'projects' && cmd === 'list') return Promise.resolve({ projects: [{ id: 'p1', name: '运维项目', path: 'C:/ops' }] })
    const impl = baseImpl()
    return Promise.resolve(impl[cmd] ?? {})
  })
})

afterEach(() => {
  vi.useRealTimers()
})

/** events.status 的 rules 投影（后端返回只读行：id/kind/path/glob/session_key/
 * 三闸缺省值/enabled）——总览表数据源，与 rules.get 同源同序。 */
function projectStatus(rules: any[]) {
  return rules.map(r => ({
    id: r.id,
    kind: r.watch.kind,
    path: r.watch.path ?? '',
    glob: r.watch.glob,
    session_key: r.deliver.session_key,
    cooldown_secs: r.deliver.cooldown_secs ?? 60,
    merge_window_secs: r.deliver.merge_window_secs ?? 5,
    max_per_hour: r.deliver.max_per_hour ?? 10,
    enabled: r.deliver.enabled !== false,
  }))
}

function setRules(rules: any[]) {
  requestMock.mockImplementation((m: string, cmd: string) => {
    if (m === 'sessions' && cmd === 'list') return Promise.resolve({ sessions: SESSION_ROWS, bindings: {} })
    if (m === 'projects' && cmd === 'list') return Promise.resolve({ projects: [{ id: 'p1', name: '运维项目', path: 'C:/ops' }] })
    if (m === 'events' && cmd === 'status') return Promise.resolve({ stats: {}, rules: projectStatus(rules), pending: [] })
    if (m === 'events' && cmd === 'rules.get') return Promise.resolve({ rules })
    if (m === 'events' && cmd === 'rules.save') return Promise.resolve({ warnings: [] })
    if (m === 'tasks' && cmd === 'cron.list') return Promise.resolve({ jobs: [] })
    return Promise.resolve({})
  })
}

async function mountView() {
  // 双 TAB（2026-11-18）默认落总览；编辑类用例需要规则设置 TAB，这里统一切过去。
  // teleport stub：会话 picker Teleport 到 body 后，stub 让内容原地渲染可查。
  const w = mount(EventsView, {
    global: { plugins: [createPinia()], stubs: { teleport: true } },
  })
  await flushPromises()
  await w.find('[data-testid="events-tab-rules"]').trigger('click')
  return w
}

async function save(w: ReturnType<typeof mount>) {
  await w.find('[data-testid="save-rules-btn"]').trigger('click')
  await flushPromises()
}

function savePayload(): any[] {
  const call = requestMock.mock.calls.find(([m, cmd]: any[]) => m === 'events' && cmd === 'rules.save')
  return call?.[2]?.rules ?? null
}

describe('EventsView 表单化 · 往返保真', () => {
  it('五族显式形态：get → 不动 → save 与原文深比对相等', async () => {
    setRules(FULL_RULES)
    const w = await mountView()
    const cards = w.findAll('[data-testid^="rule-card-"]')
    expect(cards.length).toBe(5)
    await save(w)
    expect(savePayload()).toEqual(FULL_RULES)
  })

  it('缺省形态：无 enabled/数字字段 → 数字 omit 保真；enabled 恒输出（语义等价扩展）', async () => {
    setRules(MINIMAL_RULES)
    const w = await mountView()
    // enabled checkbox 显示勾选（缺省语义 = 启用）。
    const cb = w.find('input[data-testid="rule-enabled-0"]')
    expect((cb.element as HTMLInputElement).checked).toBe(true)
    await save(w)
    expect(savePayload()).toEqual([
      {
        id: 'mini-fs',
        watch: { kind: 'fs', path: 'C:/x' },
        deliver: { session_key: 'agent:main:session:s-1', prompt: 'p', enabled: true },
      },
    ])
  })

  it('停用缺省规则：取消勾选 enabled → 显式落盘 false（不得被缺省 true 吞掉）', async () => {
    setRules(MINIMAL_RULES)
    const w = await mountView()
    await w.find('input[data-testid="rule-enabled-0"]').setValue(false)
    await save(w)
    expect(savePayload()![0].deliver.enabled).toBe(false)
  })
})

describe('EventsView 表单化 · 未知 kind 降级', () => {
  it('未知 kind 渲染警示条 + 原文编辑框，保存原样透传不丢数据', async () => {
    setRules(UNKNOWN_KIND_RULES)
    const w = await mountView()
    expect(w.find('[data-testid="unknown-kind-banner"]').exists()).toBe(true)
    const ta = w.find('[data-testid="unknown-rule-editor-0"]')
    expect(ta.exists()).toBe(true)
    // 原文含全部未知字段（spin/entangled/custom_gate/topExtra）。
    const raw = JSON.parse((ta.element as HTMLTextAreaElement).value)
    expect(raw).toEqual(UNKNOWN_KIND_RULES[0])
    await save(w)
    expect(savePayload()).toEqual(UNKNOWN_KIND_RULES)
  })

  it('未知 kind 原文改坏 JSON → 校验拦截不发 save', async () => {
    setRules(UNKNOWN_KIND_RULES)
    const w = await mountView()
    await w.find('[data-testid="unknown-rule-editor-0"]').setValue('{ broken')
    await save(w)
    expect(w.find('[data-testid="rules-error"]').text()).toContain('原文 JSON 非法')
    expect(savePayload()).toBeNull()
  })
})

describe('EventsView 表单化 · 未知字段保真', () => {
  it('顶层/watch/deliver 未知字段在编辑其他字段后原样合并回', async () => {
    setRules(EXTRA_FIELDS_RULES)
    const w = await mountView()
    // 改动已知字段（prompt 加字）。
    const prompt = w.find('[data-testid="rule-card-0"] textarea')
    await prompt.setValue('p2')
    await save(w)
    const out = savePayload()
    expect(out[0].topExtra).toBe('top-kept')
    expect(out[0].watch.future_field).toEqual({ a: 1 })
    expect(out[0].deliver.vendor_hint).toBe('v')
    expect(out[0].deliver.prompt).toBe('p2')
    expect(out[0].watch.path).toBe('C:/y')
  })
})

describe('EventsView 表单化 · 新增与编辑', () => {
  it('新增规则 → fs 模板卡（建议 id 顺延），填写后保存结构正确', async () => {
    setRules(MINIMAL_RULES)
    const w = await mountView()
    await w.find('[data-testid="add-rule-btn"]').trigger('click')
    await flushPromises()
    const idInput = w.find('[data-testid="rule-id-1"]')
    expect((idInput.element as HTMLInputElement).value).toBe('rule-2')
    // 填必填字段。
    await idInput.setValue('my-rule')
    await w.find('[data-testid="fs-path-1"]').setValue('C:/watch/dir')
    const cards = w.findAll('[data-testid^="rule-card-"]')
    // 目标会话 + prompt（卡内 input/textarea 定位）。
    await cards[1].find('input[placeholder^="agent:main:session"]').setValue('agent:main:session:s-9')
    await cards[1].find('textarea').setValue('触发：{paths}')
    await save(w)
    const out = savePayload()
    expect(out.length).toBe(2)
    expect(out[1]).toEqual({
      id: 'my-rule',
      watch: { kind: 'fs', path: 'C:/watch/dir' },
      deliver: { session_key: 'agent:main:session:s-9', prompt: '触发：{paths}', enabled: true },
    })
  })

  it('kind 切换：fs → probe 渲染 probe 字段组', async () => {
    setRules(MINIMAL_RULES)
    const w = await mountView()
    await w.find('[data-testid="rule-kind-0"]').setValue('probe')
    await flushPromises()
    expect(w.text()).toContain('探测形态')
    expect(w.text()).toContain('探测周期(秒)')
    // fs 的监视根字段组不再渲染。
    expect(w.find('[data-testid="fs-path-0"]').exists()).toBe(false)
  })
})

describe('EventsView 表单化 · 校验拦截', () => {
  it('清空 fs 必填 path → 错误显示在保存钮旁、不发 save', async () => {
    setRules(MINIMAL_RULES)
    const w = await mountView()
    await w.find('[data-testid="fs-path-0"]').setValue('')
    await save(w)
    expect(w.find('[data-testid="rules-error"]').text()).toContain('监视根 path 必填')
    expect(savePayload()).toBeNull()
  })

  it('id 重复拦截', async () => {
    setRules([
      MINIMAL_RULES[0],
      { ...MINIMAL_RULES[0], id: 'mini-fs' },
    ])
    const w = await mountView()
    await save(w)
    expect(w.find('[data-testid="rules-error"]').text()).toContain('id 重复')
    expect(savePayload()).toBeNull()
  })

  it('session_key 非 preset 形态拦截', async () => {
    setRules(MINIMAL_RULES)
    const w = await mountView()
    const cards = w.findAll('[data-testid^="rule-card-"]')
    await cards[0].find('input[placeholder^="agent:main:session"]').setValue('bare-session-id')
    await save(w)
    expect(w.find('[data-testid="rules-error"]').text()).toContain('agent:/agents: 预设形态')
    expect(savePayload()).toBeNull()
  })

  it('sys below >= above 拦截（滞回永不回 normal）', async () => {
    setRules([
      {
        id: 'cpu',
        watch: { kind: 'sys', metric: 'cpu', above: 80, below: 85 },
        deliver: { session_key: 'agent:main:session:s-1', prompt: 'p' },
      },
    ])
    const w = await mountView()
    await save(w)
    expect(w.find('[data-testid="rules-error"]').text()).toContain('below 须低于触发阈值')
    expect(savePayload()).toBeNull()
  })

  it('probe http 目标非 URL 拦截', async () => {
    setRules([
      {
        id: 'pr',
        watch: { kind: 'probe', probe_type: 'http', target: 'api.example.com', interval_secs: 2 },
        deliver: { session_key: 'agent:main:session:s-1', prompt: 'p' },
      },
    ])
    const w = await mountView()
    await save(w)
    const err = w.find('[data-testid="rules-error"]').text()
    expect(err).toContain('完整 URL')
    expect(err).toContain('最小 5 秒')
    expect(savePayload()).toBeNull()
  })
})

describe('EventsView 表单化 · 编辑守护与会话 picker', () => {
  it('总览启停按 id 定位编辑卡：本地结构改动后不错位（回归锁：按下标会启停错卡）', async () => {
    setRules([
      MINIMAL_RULES[0],
      { id: 'second', watch: { kind: 'fs', path: 'C:/y' }, deliver: { session_key: 'agent:main:session:s-1', prompt: 'p' } },
    ])
    const w = await mountView()
    // 本地删除第一张卡（结构改动未保存）→ forms=[second]，总览仍 [mini-fs, second]。
    await w.find('[data-testid="rule-del-0"]').trigger('click')
    // 切总览 TAB（mountView 默认进规则设置），点第二行（second）的启停：
    // 按下标取会拿到 forms[1]（不存在）→ 不保存；按 id 定位应正确启停 second。
    await w.find('[data-testid="events-tab-overview"]').trigger('click')
    await w.find('[data-testid="toggle-1"]').trigger('click')
    await flushPromises()
    const payload = savePayload()
    expect(payload).not.toBeNull()
    expect(payload.length).toBe(1)
    expect(payload[0].id).toBe('second')
    expect(payload[0].deliver.enabled).toBe(false)
  })

  it('总览启停目标在编辑区已删 → 诚实提示不发 save', async () => {
    setRules([MINIMAL_RULES[0]])
    const w = await mountView()
    await w.find('[data-testid="rule-del-0"]').trigger('click')
    await w.find('[data-testid="events-tab-overview"]').trigger('click')
    await w.find('[data-testid="toggle-0"]').trigger('click')
    await flushPromises()
    expect(savePayload()).toBeNull()
    expect(useToast().toasts.some(t => t.type === 'warn' && t.message.includes('mini-fs'))).toBe(true)
  })

  it('会话 picker 选定置脏：5s 轮询不冲掉所选 session_key（回归锁）', async () => {
    setRules([{ id: 'r', watch: { kind: 'fs', path: 'C:/x' }, deliver: { session_key: 'agent:main:session:s-1', prompt: 'p' } }])
    const w = await mountView()
    await w.find('[data-testid="pick-session-0"]').trigger('click')
    await flushPromises()
    // 选第二行（s-2，与原值 s-1 不同）。
    await w.findAll('.ev-picker-item')[1].trigger('click')
    await flushPromises()
    // 轮询拉回原规则（session_key 仍是 s-1）——未置脏会被冲掉。
    vi.advanceTimersByTime(5000)
    await flushPromises()
    const cards = w.findAll('[data-testid^="rule-card-"]')
    const sk = cards[0].find('input[placeholder^="agent:main:session"]')
    expect((sk.element as HTMLInputElement).value).toBe('agent:main:session:s-2')
  })

  it('编辑后 5s 轮询不覆盖表单（rulesDirty 守卫）', async () => {
    setRules(MINIMAL_RULES)
    const w = await mountView()
    await w.find('[data-testid="fs-path-0"]').setValue('C:/edited')
    // 轮询拉到不同内容。
    setRules([{ id: 'pollution', watch: { kind: 'fs', path: 'C:/polluted' }, deliver: { session_key: 'agent:main:session:s-1', prompt: 'p' } }])
    vi.advanceTimersByTime(5000)
    await flushPromises()
    const cards = w.findAll('[data-testid^="rule-card-"]')
    expect((cards[0].find('[data-testid="fs-path-0"]').element as HTMLInputElement).value).toBe('C:/edited')
    await save(w)
    expect(savePayload()![0].watch.path).toBe('C:/edited')
  })

  it('会话 picker：搜索过滤 + 点击行落盘 agent:main:session: 形态', async () => {
    setRules([{ id: 'r', watch: { kind: 'fs', path: 'C:/x' }, deliver: { session_key: 'agent:main:session:s-1', prompt: 'p' } }])
    const w = await mountView()
    await w.find('[data-testid="pick-session-0"]').trigger('click')
    await flushPromises()
    // 搜索过滤。
    await w.find('.ev-picker input').setValue('日常')
    const items = w.findAll('.ev-picker-item')
    expect(items.length).toBe(1)
    expect(items[0].text()).toContain('日常监控')
    // 点击选定（单选）→ 关闭弹层 → session_key 更新。
    await items[0].trigger('click')
    await flushPromises()
    expect(w.find('.ev-picker').exists()).toBe(false)
    const cards = w.findAll('[data-testid^="rule-card-"]')
    const sk = cards[0].find('input[placeholder^="agent:main:session"]')
    expect((sk.element as HTMLInputElement).value).toBe('agent:main:session:s-2')
  })
})

// ---------------------------------------------------------------------------
// R2231 回归锁（BUG-20261122-004 / BUG-20261122-005）
// ---------------------------------------------------------------------------

describe('EventsView · picker 定位与编辑守护回归锁', () => {
  it('BUG-20261122-004：pickerRoot 单元素 ref + pickerPos 有限值（修复前 ref 在 v-for 内被数组化，watch 定位算式读 undefined → NaN 死码）', async () => {
    setRules([MINIMAL_RULES[0]])
    const w = await mountView()
    await w.find('[data-testid="pick-session-0"]').trigger('click')
    await flushPromises()
    const vm = w.vm as any
    // 修复（Teleport 提升出 v-for）前：Array.isArray(pickerRoot) === true
    // 且 pickerPos = {top: NaN, left: NaN}（实测探针证据）。
    expect(Array.isArray(vm.pickerRoot)).toBe(false)
    expect(Number.isNaN(vm.pickerPos?.top)).toBe(false)
    expect(Number.isNaN(vm.pickerPos?.left)).toBe(false)
  })

  it('BUG-20261122-005：rules.get 飞行窗口内开始编辑 → await 后复核放弃覆写', async () => {
    setRules(MINIMAL_RULES)
    const w = await mountView()
    // rules.get 换成手动放行版：轮询发出后挂起，编辑落在飞行窗口内
    // （守卫第一道检查已通过——检查在 await 前，正是原缺陷的洞）。
    let release!: (v: any) => void
    requestMock.mockImplementation((m: string, cmd: string) => {
      if (m === 'sessions' && cmd === 'list') return Promise.resolve({ sessions: SESSION_ROWS, bindings: {} })
      if (m === 'projects' && cmd === 'list') return Promise.resolve({ projects: [{ id: 'p1', name: '运维项目', path: 'C:/ops' }] })
      if (m === 'events' && cmd === 'status') return Promise.resolve({ stats: {}, rules: projectStatus(MINIMAL_RULES), pending: [] })
      if (m === 'events' && cmd === 'rules.get') return new Promise(r => { release = r })
      if (m === 'tasks' && cmd === 'cron.list') return Promise.resolve({ jobs: [] })
      return Promise.resolve({})
    })
    vi.advanceTimersByTime(5000)
    await flushPromises()
    await w.find('[data-testid="fs-path-0"]').setValue('C:/typed-in-flight')
    release({ rules: [{ id: 'polluted', watch: { kind: 'fs', path: 'C:/polluted' }, deliver: { session_key: 'agent:main:session:s-1', prompt: 'p' } }] })
    await flushPromises()
    const cards = w.findAll('[data-testid^="rule-card-"]')
    expect((cards[0].find('[data-testid="fs-path-0"]').element as HTMLInputElement).value).toBe('C:/typed-in-flight')
  })

  it('BUG-20261122-005 同族：保存飞行窗口内新编辑 → dirty 保留 + 轮询避让 + 提示再存', async () => {
    setRules(MINIMAL_RULES)
    const w = await mountView()
    let release!: (v: any) => void
    requestMock.mockImplementation((m: string, cmd: string) => {
      if (m === 'sessions' && cmd === 'list') return Promise.resolve({ sessions: SESSION_ROWS, bindings: {} })
      if (m === 'projects' && cmd === 'list') return Promise.resolve({ projects: [{ id: 'p1', name: '运维项目', path: 'C:/ops' }] })
      if (m === 'events' && cmd === 'status') return Promise.resolve({ stats: {}, rules: projectStatus(MINIMAL_RULES), pending: [] })
      if (m === 'events' && cmd === 'rules.get') return Promise.resolve({ rules: MINIMAL_RULES })
      if (m === 'events' && cmd === 'rules.save') return new Promise(r => { release = r })
      if (m === 'tasks' && cmd === 'cron.list') return Promise.resolve({ jobs: [] })
      return Promise.resolve({})
    })
    await w.find('[data-testid="save-rules-btn"]').trigger('click')
    await flushPromises()
    // 保存挂起期间落键。
    await w.find('[data-testid="fs-path-0"]').setValue('C:/during-save')
    release({ warnings: [] })
    await flushPromises()
    // 提示再存；dirty 保留 → 后续轮询不得冲掉飞行窗口内的编辑。
    expect(useToast().toasts.some(t => t.type === 'warn' && t.message.includes('再点一次'))).toBe(true)
    vi.advanceTimersByTime(5000)
    await flushPromises()
    const cards = w.findAll('[data-testid^="rule-card-"]')
    expect((cards[0].find('[data-testid="fs-path-0"]').element as HTMLInputElement).value).toBe('C:/during-save')
  })
})
