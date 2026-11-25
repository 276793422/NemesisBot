// B 线用户习惯面板契约（Forge 化重构 2026-11-24 更新；批 D 增补）：
// - profile.list → 开关态 + 条目表（类别/来源色点/内容/置信度/状态 +
//   steps 步骤清单 + ui_telemetry 开关投影）；
// - 批 D：五维分组（流程仪式/操作仪式/表达粒度/节奏分工/信任画像）+
//   profile.stats 统计卡（todo 完成度/教训分流/条目来源/近期强信号）；
// - veto/forget/pin 后门动作带 content 载荷；
// - telemetry_set 切换 UI 遥测（config user_profile.ui_telemetry 面）；
// - 未开启 → 空态文案诚实标注 + 显示开启按钮。
// - 接口二（2026-11-24）：待确认观察卡（profile.observations 列表 +
//   observation_respond 裁决载荷 {id, approved}）+ stats 卡 observations
//   三态计数 + source=observation 徽标「反思观察」。
// - 来源色点（2026-11-25 用户裁定，替代徽标链）：习惯列左侧独立列渲染
//   带色小点——蓝=内置触发（rule_id 在场，正交于话术来源：AI 改写认领
//   也是蓝点，tooltip 注明改写）/ 绿=AI 总结 / 灰=模板兜底（legacy 位）/
//   橙=反思观察 / 紫=外部回抛 / 青=手动条目；表头带色点图例 tooltip。
// - 溯源（evidence）不进正文，挂「命中」列 title 悬停可见。
// - 操作三键说明 tips（2026-11-25 用户要求）：钉住/否决/遗忘悬停可见
//   （钉住态切换「取消钉住」说明）。
// - 双 TAB 注入内容页（2026-11-25 用户拍板双 scope，批三）：TAB1「注入
//   内容」默认——对话/项目两大卡片左右并列，每卡片上半 = 手动习惯编辑器
//   （manual_get 回显 / manual_save {scope, lines} 整体替换保存）/ 下半 =
//   注入预览（profile.preview 后端 render_section 渲染产物，只读）；
//   TAB2「习惯条目」= 治理面（统计/观察/条目表）原样保留，默认隐藏。
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import ProfileView from '../ProfileView.vue'

function habit(over: Record<string, unknown> = {}) {
  return {
    category: 'style',
    content: '用户偏好简洁回复',
    confidence: 0.85,
    hits: 3,
    first_seen: '2026-10-01T00:00:00+08:00',
    last_seen: '2026-10-14T00:00:00+08:00',
    status: 'active',
    pinned: false,
    steps: [],
    evidence: [],
    auto: false,
    ...over,
  }
}

/// TAB2（习惯条目治理面）挂载：默认 TAB 是「注入内容」，治理面用例先切
/// 到「习惯条目」tab 再断言（2026-11-25 双 TAB 改版）。
async function mountOnEntries() {
  const w = mount(ProfileView)
  await flushPromises()
  await w.find('[data-testid="tab-entries"]').trigger('click')
  await flushPromises()
  return w
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation(async (_m: string, cmd: string) => {
    if (cmd === 'list') {
      return {
        enabled: true,
        enabled_at: '2026-10-01T08:00:00+08:00',
        ui_telemetry: true,
        habits: [
          habit(),
          habit({ category: 'correction', content: '先确认理解再动手', status: 'decaying', confidence: 0.72, pinned: true }),
        ],
      }
    }
    return {}
  })
})

describe('ProfileView（双 TAB 注入内容页，2026-11-25）', () => {
  it('TAB1 默认激活：双卡片渲染（对话/项目并列），manual_get/preview 按 scope 请求', async () => {
    const calls: any[] = []
    requestMock.mockImplementation(async (_m: string, cmd: string, data: any) => {
      calls.push([cmd, data])
      if (cmd === 'list') return { enabled: true, enabled_at: null, ui_telemetry: true, habits: [] }
      if (cmd === 'manual_get' && data?.scope === 'dialogue') {
        return { scope: 'dialogue', lines: ['[风格] 提交信息用中文一行式'] }
      }
      if (cmd === 'manual_get' && data?.scope === 'project') return { scope: 'project', lines: [] }
      if (cmd === 'preview' && data?.scope === 'dialogue') {
        return { scope: 'dialogue', preview: '- [风格] 回复保持简短\n（另有手动 1 条一并注入）', manual_count: 1 }
      }
      if (cmd === 'preview' && data?.scope === 'project') {
        return { scope: 'project', preview: '', manual_count: 0 }
      }
      return {}
    })
    const w = mount(ProfileView)
    await flushPromises()
    // 默认 TAB = 注入内容：双卡片在场、习惯条目表不渲染。
    expect(w.find('[data-testid="inject-cards"]').exists()).toBe(true)
    expect(w.find('[data-testid="inject-card-dialogue"]').exists()).toBe(true)
    expect(w.find('[data-testid="inject-card-project"]').exists()).toBe(true)
    expect(w.find('[data-testid="habit-row-0"]').exists()).toBe(false)
    // manual_get / preview 各按两 scope 请求（后端按 data.scope 选卡片数据源）。
    for (const scope of ['dialogue', 'project']) {
      expect(calls).toContainEqual(['manual_get', { scope }])
      expect(calls).toContainEqual(['preview', { scope }])
    }
    // 编辑器回显 manual_get 行 + 预览回显后端渲染产物。
    expect(
      (w.find('[data-testid="manual-editor-dialogue"]').element as HTMLTextAreaElement).value,
    ).toBe('[风格] 提交信息用中文一行式')
    expect(w.find('[data-testid="inject-preview-dialogue"]').text()).toContain('回复保持简短')
    expect(w.find('[data-testid="inject-preview-dialogue"]').text()).toContain('另有手动 1 条一并注入')
    // 项目卡预览空态诚实文案。
    expect(w.find('[data-testid="inject-preview-project"]').text()).toContain('暂无总结')
    w.unmount()
  })

  it('TAB1 保存：manual_save 带 {scope, lines} 载荷；冲突诚实提示；保存后刷新预览', async () => {
    const calls: any[] = []
    requestMock.mockImplementation(async (_m: string, cmd: string, data: any) => {
      calls.push([cmd, data])
      if (cmd === 'list') return { enabled: true, enabled_at: null, ui_telemetry: true, habits: [] }
      if (cmd === 'manual_get') return { scope: data?.scope, lines: [] }
      if (cmd === 'preview') return { scope: data?.scope, preview: '', manual_count: 0 }
      if (cmd === 'manual_save') {
        return { scope: data?.scope, added: 2, removed: 1, removed_contents: ['旧习惯'], conflicts: ['撞文条目'] }
      }
      return {}
    })
    const w = mount(ProfileView)
    await flushPromises()
    const editor = w.find('[data-testid="manual-editor-dialogue"]')
    await editor.setValue('[风格] 回复简短\n撞文条目\n')
    await w.find('[data-testid="manual-save-dialogue"]').trigger('click')
    await flushPromises()
    // 整体替换语义：编辑框全文按行进载荷（空行由服务端跳过）。
    expect(calls).toContainEqual([
      'manual_save',
      { scope: 'dialogue', lines: ['[风格] 回复简短', '撞文条目', ''] },
    ])
    // 结果 toast 诚实（新增/移除/撞文计数）。
    const toast = useToast()
    const last = toast.toasts[toast.toasts.length - 1]
    expect(last.message).toContain('新增 2 条')
    expect(last.message).toContain('移除 1 条')
    expect(last.message).toContain('撞文')
    // 保存后刷新本卡（manual_get/preview 重发）。
    const reloads = calls.filter(([cmd]) => cmd === 'preview' && cmd !== undefined)
    expect(reloads.length).toBeGreaterThanOrEqual(3) // 首载 2 + 保存后重发
    w.unmount()
  })

  it('TAB 切换：注入内容 → 习惯条目（治理面渲染），来回切换内容互斥', async () => {
    const w = mount(ProfileView)
    await flushPromises()
    expect(w.find('[data-testid="inject-cards"]').exists()).toBe(true)
    expect(w.find('[data-testid="stats-card"]').exists()).toBe(false)
    await w.find('[data-testid="tab-entries"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="inject-cards"]').exists()).toBe(false)
    expect(w.find('[data-testid="tab-inject"]').classes()).not.toContain('active')
    expect(w.find('[data-testid="tab-entries"]').classes()).toContain('active')
    await w.find('[data-testid="tab-inject"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="inject-cards"]').exists()).toBe(true)
    w.unmount()
  })
})

describe('ProfileView（B 线习惯条目治理面，TAB2）', () => {
  it('开启态：条目表渲染类别/内容/状态，展示采集起点', async () => {
    const w = await mountOnEntries()
    expect(w.find('[data-testid="habit-row-0"]').exists()).toBe(true)
    expect(w.text()).toContain('用户偏好简洁回复')
    expect(w.text()).toContain('先确认理解再动手')
    expect(w.find('[data-testid="habit-status-1"]').text()).toContain('衰减中')
    expect(w.find('[data-testid="enabled-at"]').exists()).toBe(true)
    w.unmount()
  })

  it('结构化条目：steps 编号清单 + auto 徽标 + 扩容类别', async () => {
    const w0 = mount(ProfileView)
    await flushPromises()
    // 重 mock：一条带 steps 的 workflow 挖掘条目 + 一条 quality_bar。
    requestMock.mockImplementation(async (_m: string, cmd: string, data: any) => {
      if (cmd === 'list') {
        return {
          enabled: true,
          enabled_at: '2026-10-01T08:00:00+08:00',
          ui_telemetry: true,
          habits: [
            habit({
              category: 'workflow',
              content: '计划式工作流',
              auto: true,
              source: 'forge-pattern',
              steps: ['先制定计划', '回顾确认', '转成步骤清单', '测试收尾'],
              evidence: ['session-a', 'session-b'],
            }),
            habit({ category: 'quality_bar', content: '交付前必测试' }),
          ],
        }
      }
      return {}
    })
    const w = await mountOnEntries()
    expect(w.text()).toContain('计划式工作流')
    expect(w.find('[data-testid="habit-step-0-0"]').text()).toContain('先制定计划')
    expect(w.find('[data-testid="habit-step-0-3"]').text()).toContain('测试收尾')
    // 批三来源色点：source 非空 → 外部/观察色点优先于 auto 位（该条目
    // source=forge-pattern → 紫点 external；纯 auto=true 无 source 才是
    // 灰点模板兜底，见 stats 用例）。
    expect(w.find('[data-testid="src-dot-0"]').classes()).toContain('dot-external')
    expect(w.find('[data-testid="src-dot-1"]').classes()).toContain('dot-ai')
    expect(w.text()).toContain('质量线')
    // 溯源不进正文（2026-11-25 用户裁定）：挂「命中」列 title tooltip 悬停可见。
    expect(w.find('[data-testid="habit-hits-0"]').attributes('title')).toContain('溯源')
    expect(w.find('[data-testid="habit-hits-0"]').attributes('title')).toContain('session-a、session-b')
    w.unmount()
    w0.unmount()
  })

  it('telemetry_set 切换 UI 遥测开关（带 enabled 布尔载荷）', async () => {
    const w = mount(ProfileView)
    await flushPromises()
    expect(w.find<HTMLInputElement>('[data-testid="telemetry-toggle"]').element.checked).toBe(true)
    const calls: any[] = []
    requestMock.mockImplementation(async (_m: string, cmd: string, data: any) => {
      calls.push([cmd, data])
      if (cmd === 'telemetry_set') return { ui_telemetry: false }
      if (cmd === 'list') return { enabled: true, enabled_at: null, ui_telemetry: false, habits: [] }
      return {}
    })
    await w.find('[data-testid="telemetry-toggle"]').setValue(false)
    await flushPromises()
    expect(calls.some(([cmd, data]) => cmd === 'telemetry_set' && data?.enabled === false)).toBe(true)
    w.unmount()
  })

  it('veto/forget/pin 各带 content 载荷', async () => {
    const w = await mountOnEntries()
    const calls: any[] = []
    requestMock.mockImplementation(async (_m: string, cmd: string, data: any) => {
      calls.push([cmd, data])
      if (cmd === 'veto' || cmd === 'forget' || cmd === 'pin') return { vetoed: true, forgotten: true, updated: true }
      if (cmd === 'list') return { enabled: true, enabled_at: null, ui_telemetry: true, habits: [habit()] }
      return {}
    })
    await w.find('[data-testid="veto-0"]').trigger('click')
    await flushPromises()
    await w.find('[data-testid="forget-0"]').trigger('click')
    await flushPromises()
    await w.find('[data-testid="pin-0"]').trigger('click')
    await flushPromises()
    const cmds = calls.map((c) => c[0])
    expect(cmds).toContain('veto')
    expect(cmds).toContain('forget')
    expect(cmds).toContain('pin')
    for (const [cmd, data] of calls.filter(([cmd]) => ['veto', 'forget', 'pin'].includes(cmd))) {
      expect(data).toEqual({ content: '用户偏好简洁回复' })
      expect(cmds.length).toBeGreaterThanOrEqual(3)
    }
    w.unmount()
  })

  it('未开启：空态诚实标注 + 显示开启按钮；enable 调用后按钮切换', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') return { enabled: false, enabled_at: null, ui_telemetry: true, habits: [] }
      if (cmd === 'enable') return { enabled: true, enabled_at: '2026-10-14T10:00:00+08:00' }
      return {}
    })
    const w = await mountOnEntries()
    expect(w.find('[data-testid="empty-state"]').text()).toContain('画像未开启')
    expect(w.find('[data-testid="enable-btn"]').exists()).toBe(true)
    await w.find('[data-testid="enable-btn"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="disable-btn"]').exists()).toBe(true)
    w.unmount()
  })

  it('批 D：stats 统计卡渲染读数（含手动桶）；trust 条目落信任画像分组', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') {
        return {
          enabled: true,
          enabled_at: '2026-10-01T08:00:00+08:00',
          ui_telemetry: true,
          habits: [
            habit({ category: 'workflow', content: '计划式工作流', auto: true }),
            habit({ category: 'trust', content: '审批卡平均 3 秒批准，信任度高', confidence: 0.9 }),
          ],
        }
      }
      if (cmd === 'stats') {
        return {
          enabled: true,
          stats: {
            todos: { snapshot_count: 5, item_count: 12, completed_count: 9, completion_ratio: 0.75, snapshots_with_pending: 1 },
            lessons: { pending: 3, acked: 7 },
            habits_by_category: { workflow: 1, trust: 1 },
            habits_by_source: { ai: 1, template: 1, observation: 0, external: 0, manual: 4 },
            signals_recent: { approval_cards: 14, estop: 2 },
          },
        }
      }
      return {}
    })
    const w = await mountOnEntries()
    // 统计卡读数。
    expect(w.find('[data-testid="stats-card"]').exists()).toBe(true)
    expect(w.find('[data-testid="stats-todos"]').text()).toContain('75%')
    expect(w.find('[data-testid="stats-todos"]').text()).toContain('9/12 项')
    expect(w.find('[data-testid="stats-lessons"]').text()).toContain('待消费 3')
    expect(w.find('[data-testid="stats-lessons"]').text()).toContain('已消费 7')
    expect(w.find('[data-testid="stats-sources"]').text()).toContain('AI 总结 1')
    expect(w.find('[data-testid="stats-sources"]').text()).toContain('模板兜底 1')
    // 双 scope 批三：手动桶入统计行（对话+项目两画像合并聚合）。
    expect(w.find('[data-testid="stats-sources"]').text()).toContain('手动 4')
    // 纯 auto=true 无 source 的 workflow 条目 → 模板兜底灰点（legacy 色位）。
    expect(w.find('[data-testid="src-dot-0"]').classes()).toContain('dot-template')
    expect(w.find('[data-testid="stats-signals"]').text()).toContain('审批卡 14')
    expect(w.find('[data-testid="stats-signals"]').text()).toContain('急停 2')
    // 分组头：流程仪式（workflow）与信任画像（trust）各自成组。
    expect(w.text()).toContain('流程仪式')
    expect(w.text()).toContain('信任画像')
    // trust 条目渲染在新类别标签下。
    expect(w.text()).toContain('审批卡平均 3 秒批准，信任度高')
    w.unmount()
  })

  it('批 D：stats 请求失败 → 统计卡隐藏不阻塞条目表', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') return { enabled: true, enabled_at: null, ui_telemetry: true, habits: [habit()] }
      if (cmd === 'stats') throw new Error('boom')
      return {}
    })
    const w = await mountOnEntries()
    expect(w.find('[data-testid="stats-card"]').exists()).toBe(false)
    expect(w.find('[data-testid="habit-row-0"]').exists()).toBe(true)
    w.unmount()
  })

  it('接口二：待确认观察卡渲染 pending 列表；确认/忽略带 {id, approved} 载荷', async () => {
    const calls: any[] = []
    requestMock.mockImplementation(async (_m: string, cmd: string, data: any) => {
      calls.push([cmd, data])
      if (cmd === 'list') {
        return { enabled: true, enabled_at: null, ui_telemetry: true, habits: [] }
      }
      if (cmd === 'stats') {
        return {
          stats: {
            todos: { completion_ratio: 0, item_count: 0, completed_count: 0, snapshots_with_pending: 0 },
            lessons: { pending: 0, acked: 0 },
            observations: { pending: 2, confirmed: 1, dismissed: 0 },
          },
        }
      }
      if (cmd === 'observations') {
        return {
          observations: [
            { id: 'obs-1', kind: 'observation', category: 'workflow', content: '边缘结论甲', confidence: 0.6, steps: ['步骤一'], status: 'pending' },
            { id: 'obs-2', kind: 'observation', category: 'style', content: '边缘结论乙', confidence: 0.55, steps: [], status: 'pending' },
          ],
          counts: { pending: 2, confirmed: 1, dismissed: 0 },
        }
      }
      if (cmd === 'observation_respond') return { resolved: true, approved: data?.approved }
      return {}
    })
    const w = await mountOnEntries()
    // 观察卡渲染两条 pending + 步骤清单 + 统计卡三态计数。
    expect(w.find('[data-testid="observations-card"]').exists()).toBe(true)
    expect(w.find('[data-testid="obs-row-0"]').text()).toContain('边缘结论甲')
    expect(w.find('[data-testid="obs-row-1"]').text()).toContain('边缘结论乙')
    expect(w.find('[data-testid="obs-step-0-0"]').text()).toContain('步骤一')
    expect(w.find('[data-testid="stats-observations"]').text()).toContain('已确认 1')
    // 确认第一条：载荷 {id, approved: true} → 重载后队列变化（此 mock 恒返两条）。
    await w.find('[data-testid="obs-confirm-0"]').trigger('click')
    await flushPromises()
    // 忽略第二条。
    await w.find('[data-testid="obs-dismiss-1"]').trigger('click')
    await flushPromises()
    const respondCalls = calls.filter(([cmd]) => cmd === 'observation_respond')
    expect(respondCalls).toContainEqual(['observation_respond', { id: 'obs-1', approved: true }])
    expect(respondCalls).toContainEqual(['observation_respond', { id: 'obs-2', approved: false }])
    w.unmount()
  })

  it('接口二：observations 请求失败 → 观察卡不渲染不阻塞；无 pending 不渲染卡', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') return { enabled: true, enabled_at: null, ui_telemetry: true, habits: [habit()] }
      if (cmd === 'observations') throw new Error('boom')
      return {}
    })
    const w = await mountOnEntries()
    expect(w.find('[data-testid="observations-card"]').exists()).toBe(false)
    expect(w.find('[data-testid="habit-row-0"]').exists()).toBe(true)
    w.unmount()
  })

  it('接口二：source=observation 条目落橙点（tooltip 注明反思观察）', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') {
        return {
          enabled: true, enabled_at: null, ui_telemetry: true,
          habits: [habit({ source: 'observation', content: '确认后的观察条目' })],
        }
      }
      return {}
    })
    const w = await mountOnEntries()
    expect(w.find('[data-testid="src-dot-0"]').classes()).toContain('dot-observation')
    expect(w.find('[data-testid="src-dot-0"]').attributes('title')).toContain('反思观察')
    w.unmount()
  })

  it('批三来源色点：source_kind 字段驱动（蓝=内置触发/绿=AI/灰=模板/橙=观察/紫=外部/青=手动）', async () => {
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') {
        return {
          enabled: true, enabled_at: null, ui_telemetry: true,
          habits: [
            // WSAPI 投影后的形态：逐条带 source_kind（后端 surface::source_kind）。
            habit({ content: 'AI 措辞条目', auto: false, source_kind: 'ai' }),
            habit({ content: '模板条目', auto: true, source_kind: 'template', rule_id: 'stat.brief' }),
            habit({ content: '观察转正条目', auto: false, source: 'observation', source_kind: 'observation' }),
            habit({ content: '外部回抛条目', auto: false, source: 'learn_dialogue', source_kind: 'external' }),
            habit({ content: '手写条目', auto: false, source: 'manual', source_kind: 'manual' }),
          ],
        }
      }
      return {}
    })
    const w = await mountOnEntries()
    expect(w.text()).toContain('AI 措辞条目')
    // 五态+手动色点：ai=绿 / observation=橙 / external=紫 / manual=青。
    expect(w.find('[data-testid="src-dot-0"]').classes()).toContain('dot-ai')
    expect(w.find('[data-testid="src-dot-2"]').classes()).toContain('dot-observation')
    expect(w.find('[data-testid="src-dot-3"]').classes()).toContain('dot-external')
    expect(w.find('[data-testid="src-dot-4"]').classes()).toContain('dot-manual')
    expect(w.find('[data-testid="src-dot-4"]').attributes('title')).toContain('手动条目')
    expect(w.find('[data-testid="src-dot-4"]').attributes('title')).toContain('注入内容')
    // rule_id 在场 = 蓝点优先（内置触发正交于话术来源：模板条目带 rule_id 落蓝）。
    expect(w.find('[data-testid="src-dot-1"]').classes()).toContain('dot-rule')
    expect(w.find('[data-testid="src-dot-1"]').attributes('title')).toContain('内置触发 · stat.brief')
    // 表头色点列带图例 tooltip（含手动色说明）。
    expect(w.find('[data-testid="src-dot-col-head"]').attributes('title')).toContain('手动')
    w.unmount()
  })

  it('来源色点：AI 改写认领条目（auto=false + rule_id）= 蓝点 + tooltip 注明改写', async () => {
    // 内置模板被 AI 原地改写后的形态：rule_id 在场 → 蓝点（内置触发），
    // tooltip 注明「话术已由 AI 原地改写」——UI 上可与纯 AI 自发条目区分。
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'list') {
        return {
          enabled: true, enabled_at: null, ui_telemetry: true,
          habits: [
            habit({ content: '改写认领条目', auto: false, source_kind: 'ai', rule_id: 'opener.task' }),
            habit({ content: '纯 AI 条目', auto: false, source_kind: 'ai' }),
          ],
        }
      }
      return {}
    })
    const w = await mountOnEntries()
    expect(w.find('[data-testid="src-dot-0"]').classes()).toContain('dot-rule')
    expect(w.find('[data-testid="src-dot-0"]').attributes('title')).toContain('opener.task')
    expect(w.find('[data-testid="src-dot-0"]').attributes('title')).toContain('话术已由 AI 原地改写')
    // 纯 AI 自发（无 rule_id）= 绿点。
    expect(w.find('[data-testid="src-dot-1"]').classes()).toContain('dot-ai')
    w.unmount()
  })

  it('操作三键说明 tips：钉住/否决/遗忘悬停可见，钉住态切换「取消钉住」文案', async () => {
    const w = await mountOnEntries()
    // 未钉住 → 钉住 = 用户意志锚点说明（不衰减/不被改写/优先保留）。
    expect(w.find('[data-testid="pin-0"]').attributes('title')).toContain('钉住：设为用户意志锚点')
    expect(w.find('[data-testid="pin-0"]').attributes('title')).toContain('不被 AI 反思改写')
    // fixture 第 2 条 pinned: true → 按钮变「取消钉住」，说明切解除语义。
    expect(w.find('[data-testid="pin-1"]').attributes('title')).toContain('取消钉住：解除锚点保护')
    // 否决 = 负样本永久压制（对比遗忘：可重生）。
    expect(w.find('[data-testid="veto-0"]').attributes('title')).toContain('负样本')
    expect(w.find('[data-testid="veto-0"]').attributes('title')).toContain('不会重新生成')
    // 遗忘 = 删除但可重生。
    expect(w.find('[data-testid="forget-0"]').attributes('title')).toContain('直接删除')
    expect(w.find('[data-testid="forget-0"]').attributes('title')).toContain('重新生成')
    w.unmount()
  })
})
