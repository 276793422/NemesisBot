<script setup lang="ts">
// 用户习惯画像面板（Forge 化重构，2026-11-24：后端迁 nemesis-forge::
// userprofile，条目增 steps 步骤清单 / evidence 溯源 / auto 固定产物标记；
// 新增 UI 遥测开关 profile.telemetry_get/set——config
// user_profile.ui_telemetry，存储即隐私闸）。
// 批 D（2026-11-24）：①五维分组展示（流程仪式/操作仪式/表达粒度/节奏
// 分工/信任画像——trust 类别随批 C 信任挖掘加入）；②挖掘统计卡
// profile.stats（todo 完成度/教训队列分流/条目来源/近期强信号）——
// 纯 Dashboard 展示，永不注入。
// 展示 profile.list（enabled/enabled_at/habits）；后门动作 veto/forget/pin；
// 开关 profile.enable/disable（真相源 config.json user_profile.enabled +
// user_profile.json enabled_at 仅向前锚点）。生成侧免确认自动生效——本页
// 只做展示与人工后门，不做审批闸。硬边界：权限/安全语义内容在渲染层
// 过滤（后端 store::violates_security_boundary），页面不重复判断。
// 接口二（2026-11-24）：「待确认观察」卡——反思通道低于置信度闸的边缘
// 结论人工裁决（profile.observations / observation_respond：确认 =
// ingest source=observation 入画像；忽略 = 负样本回流）；stats 卡补
// observations 三态计数；条目来源徽标 observation → 「反思观察」。
// 双 scope 注入内容页（2026-11-25 用户拍板，双 TAB 改版）：TAB1「注入
// 内容」（默认）= 对话/项目两大卡片左右并列，每卡片上半 = 手动习惯编辑器
// （用户自己设置习惯，必须可编辑，保存后一并注入）、下半 = 系统总结预览
// （后端 profile.preview 渲染产物，只读——用户看这里就知道实际注入提示词
// 的内容）；TAB2「习惯条目」= 现有治理表格原样保留。手动条目行语法
// `[类别] 内容`（类别可选缺省 manual），manual_get/manual_save 服务端
// 按行 diff 整体替换（新增 upsert / 消失删除，只动 source=manual）。
// 手动触发 + 语义分型治理（2026-11-26 批四）：①页头「立即总结」按钮 =
// profile.reflect_now（force sweep 绕过触发闸，空窗判定不放宽——空窗诚实
// 返回 note 注记）；②条目表双 scope 合并展示（list 按 scope 拉两份，
// 条目带 _scope 标签 + 「归属」列 badge），veto/forget/pin 载荷带 scope
// （条目从哪个画像来就操作哪个画像——此前缺省对话会把项目条目错路由）；
// ③搬移按钮 = profile.move_scope {content, to_scope}（位置分型判错的
// 人工修正通道，moved=false 幂等诚实提示）。
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const activeTab = ref<'inject' | 'entries'>('inject')

const enabled = ref(false)
const enabledAt = ref<string | null>(null)
const uiTelemetry = ref(true)
const habits = ref<any[]>([])
const stats = ref<any>(null)
const observations = ref<any[]>([])
const obsCounts = ref<any>(null)
const loading = ref(true)
const telemetryBusy = ref(false)

// 双 scope 注入内容状态（TAB1）：每 scope 一份手动编辑草稿 + 注入预览。
const injectState = ref<Record<'dialogue' | 'project', {
  draft: string
  preview: string
  manualCount: number
  saving: boolean
}>>({
  dialogue: { draft: '', preview: '', manualCount: 0, saving: false },
  project: { draft: '', preview: '', manualCount: 0, saving: false },
})

const SCOPES: Array<{ key: 'dialogue' | 'project'; title: string; hint: string }> = [
  { key: 'dialogue', title: '对话', hint: '对话组会话（主对话 / 集群 / 命令行）注入这份' },
  { key: 'project', title: '项目', hint: '项目组会话（每项目独立执行引擎）注入这份' },
]

const MANUAL_EDITOR_HINT =
  '每行一条：[类别] 习惯内容（类别可省略）。保存后这些习惯与下方系统总结一并注入。'
const MANUAL_PLACEHOLDER =
  '[风格] 提交信息用中文一行式\n回复保持简短，先给结论'

const CATEGORY_LABEL: Record<string, string> = {
  workflow: '工作流',
  correction: '纠正',
  tool_preference: '工具偏好',
  style: '风格',
  environment: '环境',
  quality_bar: '质量线',
  trust: '信任画像',
  manual: '手动',
}

function categoryLabel(c: string): string {
  return CATEGORY_LABEL[c] || c
}

// 来源徽标文案（外部抛入 source 标记 → 中文；未知标记原文展示）。
const SOURCE_LABEL: Record<string, string> = {
  observation: '反思观察',
}

function sourceLabel(s: string): string {
  return SOURCE_LABEL[s] || s
}

// 条目来源四态（批三 §4.5）：WSAPI list 逐条投影 source_kind（后端
// surface::source_kind 单一派生，与 stats 五桶同口径）；字段缺席时前端
// 按 auto/source 同口径兜底推导——两处逻辑保持对齐。
function kindOf(h: any): string {
  if (h.source_kind) return h.source_kind
  if (h.source) return h.source === 'observation' ? 'observation' : (h.source === 'manual' ? 'manual' : 'external')
  return h.auto ? 'template' : 'ai'
}

// 来源色点（2026-11-25 用户裁定，替代习惯单元格内的徽标链）：蓝=内置触发
// （rule_id 在场，触发信号来自确定性挖掘器）；绿=AI 总结；灰=模板兜底
// （legacy 色位——现行兜底产物恒带 rule_id 落蓝，灰点只覆盖历史无 rule_id
// 模板条目）；橙=反思观察；紫=外部回抛；青=手动条目（双 scope 批三，
// 用户在「注入内容」页手写）。rule_id 与话术来源正交（AI 改写
// 认领 = 蓝点 + tooltip 注明「话术已由 AI 原地改写」），明细悬停可见。
function dotClass(h: any): string {
  if (h.rule_id) return 'dot-rule'
  const k = kindOf(h)
  if (k === 'manual') return 'dot-manual'
  if (k === 'ai') return 'dot-ai'
  if (k === 'template') return 'dot-template'
  if (k === 'observation') return 'dot-observation'
  return 'dot-external'
}
function dotTitle(h: any): string {
  if (h.rule_id) {
    const writer = kindOf(h) === 'ai' ? '话术已由 AI 原地改写' : '话术 = 模板原文'
    return `内置触发 · ${h.rule_id}（触发信号来自确定性挖掘器；${writer}；命中按此 id 归并）`
  }
  const k = kindOf(h)
  if (k === 'manual') return '手动条目（你在「注入内容」页手写；不衰减、AI 不改写、超限截断时优先保留；编辑唯一入口 = 注入内容页）'
  if (k === 'ai') return 'AI 总结（反思通道自拟措辞）'
  if (k === 'template') return '模板兜底（确定性挖掘器固定产物）'
  if (k === 'observation') return '来自 反思观察（人工确认入画像）'
  return `来自 ${sourceLabel(h.source || '')}（外部学习子系统回抛）`
}

// 操作三键说明（2026-11-25 用户要求 tips 化，语义对齐 store.rs 后门动作）。
function pinTitle(h: any): string {
  return h.pinned
    ? '取消钉住：解除锚点保护，条目回到常规生命周期（可衰减、可被 AI 反思改写、可被合并吞并）'
    : '钉住：设为用户意志锚点——不衰减/不过期、不被 AI 反思改写、不被合并吞并，条目超限截断时优先保留，注入会话时段位置顶'
}
const VETO_TITLE =
  '否决：标记负样本——条目驻留但被压制（不再注入对话，同内容再命中也不会重新生成）；想永久屏蔽一条用它（遗忘后可能重生，否决不会）'
const FORGET_TITLE =
  '遗忘：直接删除该条目；若同样的触发信号再次出现，会重新生成同语义条目——只想清除一次用它（想永久屏蔽用「否决」）'

// 五维分组（批 D）：类别 → 维度归属；未知类别落「其他」兜底组。
const GROUPS: Array<{ key: string; label: string; categories: string[] }> = [
  { key: 'ritual-flow', label: '流程仪式', categories: ['workflow'] },
  { key: 'ritual-op', label: '操作仪式', categories: ['tool_preference', 'environment'] },
  { key: 'granularity', label: '表达粒度', categories: ['style', 'correction'] },
  { key: 'cadence', label: '节奏分工', categories: ['quality_bar'] },
  { key: 'trust', label: '信任画像', categories: ['trust'] },
]

const grouped = computed(() => {
  const out: Array<{ label: string; items: Array<{ h: any; idx: number }> }> = []
  const used = new Set<number>()
  for (const g of GROUPS) {
    const items: Array<{ h: any; idx: number }> = []
    habits.value.forEach((h, idx) => {
      if (g.categories.includes(h.category)) {
        items.push({ h, idx })
        used.add(idx)
      }
    })
    if (items.length) out.push({ label: g.label, items })
  }
  const rest: Array<{ h: any; idx: number }> = []
  habits.value.forEach((h, idx) => {
    if (!used.has(idx)) rest.push({ h, idx })
  })
  if (rest.length) out.push({ label: '其他', items: rest })
  return out
})

function statusClass(s: string): string {
  const map: Record<string, string> = {
    active: 'badge-success',
    decaying: 'badge-warning',
    expired: 'badge-neutral',
    vetoed: 'badge-error',
  }
  return map[s] || 'badge-neutral'
}

function statusLabel(s: string): string {
  const map: Record<string, string> = {
    active: '活跃',
    decaying: '衰减中',
    expired: '已过期',
    vetoed: '已否决',
  }
  return map[s] || s
}

onMounted(load)

/// 单 scope 注入内容加载（TAB1）：manual_get（编辑器回显，行语法解析
/// 单一真相源在服务端）+ preview（注入预览，渲染规则单一真相源在服务端
/// render_section）。
async function loadScopeInject(scope: 'dialogue' | 'project') {
  const st = injectState.value[scope]
  try {
    const [mg, pv] = await Promise.all([
      request('profile', 'manual_get', { scope }),
      request('profile', 'preview', { scope }),
    ])
    st.draft = (mg?.lines || []).join('\n')
    st.preview = pv?.preview || ''
    st.manualCount = pv?.manual_count || 0
  } catch { /* 注入面加载失败静默（卡内空态自解释） */ }
}

async function load() {
  loading.value = true
  try {
    const [listData, projData, statsData, obsData] = await Promise.all([
      request('profile', 'list'),
      request('profile', 'list', { scope: 'project' }).catch(() => null),
      request('profile', 'stats').catch(() => null),
      request('profile', 'observations').catch(() => null),
    ])
    // 开关态/遥测取对话侧（config 单一真相源，两 scope 同值）。
    enabled.value = !!listData?.enabled
    enabledAt.value = listData?.enabled_at || null
    uiTelemetry.value = listData?.ui_telemetry !== false
    // 双 scope 合并（批三语义分型）：条目带 _scope 标签，治理动作
    // （veto/forget/pin/move）按归属画像路由载荷；项目侧拉取失败 =
    // 诚实只显对话条目（.catch(() => null) 兜底）。
    const dlg = (listData?.habits || []).map((h: any) => ({ ...h, _scope: 'dialogue' }))
    const proj = (projData?.habits || []).map((h: any) => ({ ...h, _scope: 'project' }))
    habits.value = [...dlg, ...proj]
    stats.value = statsData?.stats || null
    observations.value = obsData?.observations || []
    obsCounts.value = obsData?.counts || null
  } catch { /* ignore */ } finally {
    loading.value = false
  }
  await Promise.all([loadScopeInject('dialogue'), loadScopeInject('project')])
}

/// 手动习惯保存（TAB1 上半编辑器；整体替换语义——服务端按行 diff）。
/// 冲突（与系统条目撞文，不重复建）诚实 toast；保存后刷新本卡预览 +
/// TAB2 表格。
async function saveManual(scope: 'dialogue' | 'project') {
  const st = injectState.value[scope]
  st.saving = true
  try {
    const lines = st.draft.split('\n')
    const data = await request('profile', 'manual_save', { scope, lines })
    const added = data?.added || 0
    const removed = data?.removed || 0
    const conflicts: string[] = data?.conflicts || []
    let msg = `已保存：新增 ${added} 条，移除 ${removed} 条`
    if (conflicts.length) msg += `；${conflicts.length} 条与系统总结撞文（不重复生成）`
    toast.success(msg)
    await Promise.all([loadScopeInject(scope), load()])
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  } finally {
    st.saving = false
  }
}

/// 裁决观察提名（接口二·待确认观察）：确认入画像 / 忽略负样本回流；
/// 裁决后整页重载（画像表 + 统计 + 队列同步刷新）。
async function respondObservation(o: any, approved: boolean) {
  try {
    const data = await request('profile', 'observation_respond', { id: o.id, approved })
    if (data?.resolved) {
      toast.success(approved
        ? '已确认入画像（来源标记「反思观察」）'
        : '已忽略（负样本回流，不再重提同类）')
    } else {
      toast.info('该提名已被处理')
    }
    await load()
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

async function toggleTelemetry() {
  telemetryBusy.value = true
  const next = !uiTelemetry.value
  try {
    await request('profile', 'telemetry_set', { enabled: next })
    uiTelemetry.value = next
    toast.success(next
      ? 'UI 行为遥测已开启（页面足迹与 WSAPI 调用流落盘供习惯挖掘）'
      : 'UI 行为遥测已关闭（停止落盘；已有数据保留）')
  } catch (e: any) {
    toast.error('切换失败: ' + e)
  } finally {
    telemetryBusy.value = false
  }
}

async function enableProfile() {
  try {
    const data = await request('profile', 'enable')
    toast.success('用户习惯画像已开启（仅采集此后会话）')
    enabledAt.value = data?.enabled_at || enabledAt.value
    enabled.value = true
  } catch (e: any) {
    toast.error('开启失败: ' + e)
  }
}

async function disableProfile() {
  try {
    await request('profile', 'disable')
    toast.info('用户习惯画像已关闭（已有条目保留、不再采集）')
    enabled.value = false
  } catch (e: any) {
    toast.error('关闭失败: ' + e)
  }
}

async function act(cmd: string, h: any, tip: string) {
  try {
    // 载荷带 scope（双 scope 合并表：缺省对话会把项目条目错路由到对话
    // 画像——条目从哪个画像来就操作哪个画像）。
    const data = await request('profile', cmd, { content: h.content, scope: h._scope || 'dialogue' })
    if (!data?.vetoed && !data?.forgotten && !data?.updated) {
      toast.info('未命中任何条目')
    } else {
      toast.success(tip)
    }
    await load()
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

function veto(h: any) { act('veto', h, '已否决（负样本，不再生成）') }
function forget(h: any) { act('forget', h, '已遗忘（同语义可重新生成）') }
function pin(h: any) { act('pin', h, h.pinned ? '已取消钉住' : '已钉住（不衰减、置顶注入）') }

/// 条目搬移（批二 move_scope）：位置分型判错的修正通道——对话/项目画像
/// 互搬；moved=false（目标已持有同 rule_id 归并）= 幂等 no-op 诚实提示。
async function moveHabit(h: any) {
  const to = h._scope === 'project' ? 'dialogue' : 'project'
  const toLabel = to === 'project' ? '项目' : '对话'
  try {
    const data = await request('profile', 'move_scope', { content: h.content, to_scope: to })
    if (data?.moved) toast.success(`已搬到${toLabel}画像${data.content ? '：' + data.content : ''}`)
    else toast.info('目标画像已持有该条目（已归并，未重复搬移）')
    await load()
  } catch (e: any) {
    toast.error('搬移失败: ' + e)
  }
}

/// 立即总结（批一 reflect_now）：force sweep 绕过自动触发闸（空窗判定不
/// 放宽）；estop / 无可用 caller 由后端诚实报错，这里 toast 透传。
const reflectBusy = ref(false)
async function reflectNow() {
  reflectBusy.value = true
  try {
    const d = await request('profile', 'reflect_now')
    if (d?.note) toast.info(d.note)
    else toast.success(`总结完成：习惯 ${d?.habits || 0} 条、教训 ${d?.lessons || 0} 条、观察 ${d?.observations || 0} 条`)
    await load()
  } catch (e: any) {
    toast.error('总结失败: ' + e)
  } finally {
    reflectBusy.value = false
  }
}
</script>

<template>
  <div class="page-profile">
    <div class="page-header">
      <h2>用户习惯</h2>
      <div class="page-header-actions">
        <span v-if="enabled && enabledAt" style="font-size: var(--text-xs); color: var(--text-muted);" data-testid="enabled-at">
          采集起点：{{ new Date(enabledAt).toLocaleString('zh-CN') }}
        </span>
        <button
          v-if="enabled"
          class="btn"
          data-testid="reflect-now-btn"
          :disabled="reflectBusy"
          title="立即跑一轮反思总结：不等自动触发闸（消息门槛/周期到点）；24h 证据窗内的内容直接归并，空窗诚实返回「无新内容」"
          @click="reflectNow"
        >
          <span v-if="reflectBusy" class="spinner" style="width: 12px; height: 12px; border-width: 2px; margin-right: var(--space-1);"></span>
          {{ reflectBusy ? '总结中…' : '立即总结' }}
        </button>
        <button v-if="!enabled" class="btn btn-primary" data-testid="enable-btn" @click="enableProfile">开启画像</button>
        <button v-else class="btn" data-testid="disable-btn" @click="disableProfile">关闭画像</button>
      </div>
    </div>

    <div class="page-body">
      <div class="card" style="margin-bottom: var(--space-4);">
        <div class="card-body">
          <p style="font-size: var(--text-sm); color: var(--text-secondary); margin: 0 0 var(--space-2);">
            从会话中自动学习稳定偏好（工作流仪式 / todo 收尾习惯 / UI 操作足迹 / LLM 反思归纳），
            注入后续会话使回复更贴合。
            <strong>只调整风格与流程姿态，绝不触碰权限与安全语义</strong>；全本地采集，
            开启后只分析此后会话（仅向前）。生成免确认自动生效，下方提供否决 / 遗忘 / 钉住后门。
          </p>
          <label class="toggle-row" style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm); cursor: pointer;" data-testid="telemetry-toggle-row">
            <input
              type="checkbox"
              class="checkbox"
              :checked="uiTelemetry"
              :disabled="telemetryBusy"
              data-testid="telemetry-toggle"
              @change="toggleTelemetry"
            />
            <span>UI 行为遥测（页面足迹 + WSAPI 调用流落盘 <code>logs/wsapi_usage.jsonl</code>，供 UI 习惯挖掘；关闭 = 停止落盘）</span>
          </label>
        </div>
      </div>

      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-else>
        <!-- 双 TAB（2026-11-25 用户拍板）：TAB1 注入内容（默认）/ TAB2 习惯条目治理面 -->
        <div class="tabs" style="margin-top: calc(-1 * var(--space-2));">
          <button class="tab" :class="{ active: activeTab === 'inject' }" data-testid="tab-inject" @click="activeTab = 'inject'">注入内容</button>
          <button class="tab" :class="{ active: activeTab === 'entries' }" data-testid="tab-entries" @click="activeTab = 'entries'">习惯条目</button>
        </div>

        <!-- TAB1 注入内容：对话/项目两大卡片左右并列；每卡片上半手动编辑器
             （用户自设习惯，必须可编辑）、下半系统总结预览（实际注入内容） -->
        <div v-if="activeTab === 'inject'" class="inject-cards" data-testid="inject-cards">
          <div v-for="sc in SCOPES" :key="sc.key" class="card" :data-testid="'inject-card-' + sc.key">
            <div class="card-header">
              <h3>{{ sc.title }}习惯</h3>
              <span style="font-size: var(--text-xs); color: var(--text-muted);">{{ sc.hint }}</span>
            </div>
            <div class="card-body">
              <p class="inject-section-label">我的手动习惯 <span style="font-weight: 400; color: var(--text-muted);">（一并注入）</span></p>
              <textarea
                v-model="injectState[sc.key].draft"
                class="form-textarea"
                rows="5"
                :placeholder="MANUAL_PLACEHOLDER"
                :data-testid="'manual-editor-' + sc.key"
              ></textarea>
              <div style="display: flex; align-items: center; gap: var(--space-3);">
                <button
                  class="btn btn-primary"
                  :disabled="injectState[sc.key].saving"
                  :data-testid="'manual-save-' + sc.key"
                  @click="saveManual(sc.key)"
                >保存手动习惯</button>
                <span style="font-size: var(--text-xs); color: var(--text-muted);">{{ MANUAL_EDITOR_HINT }}</span>
              </div>
              <p class="inject-section-label" style="margin-top: var(--space-2);">
                系统总结 <span style="font-weight: 400; color: var(--text-muted);">（实际注入提示词的内容，只读）</span>
                <span v-if="injectState[sc.key].manualCount" style="font-weight: 400; color: var(--text-muted);">——已含你的手动 {{ injectState[sc.key].manualCount }} 条</span>
              </p>
              <pre class="inject-preview" :data-testid="'inject-preview-' + sc.key">{{ injectState[sc.key].preview || '暂无总结——正常使用若干会话后自动生成；上方手动习惯保存后立即生效' }}</pre>
            </div>
          </div>
        </div>

        <!-- TAB2 习惯条目：治理面（统计 / 待确认观察 / 条目表）原样保留 -->
        <div v-if="activeTab === 'entries'">
        <!-- 挖掘统计卡（批 D；纯展示不注入） -->
        <div v-if="stats" class="card" style="margin-bottom: var(--space-4);" data-testid="stats-card">
          <div class="card-header">
            <h3>挖掘统计</h3>
            <span style="font-size: var(--text-xs); color: var(--text-muted);">只展示，不注入会话</span>
          </div>
          <div class="card-body" style="display: flex; flex-wrap: wrap; gap: var(--space-5); font-size: var(--text-sm);">
            <span data-testid="stats-todos">
              Todo 完成度：<strong style="font-variant-numeric: tabular-nums;">{{ Math.round((stats.todos?.completion_ratio || 0) * 100) }}%</strong>
              <span style="color: var(--text-muted);">（{{ stats.todos?.completed_count || 0 }}/{{ stats.todos?.item_count || 0 }} 项 · {{ stats.todos?.snapshots_with_pending || 0 }} 快照有待办）</span>
            </span>
            <span data-testid="stats-lessons">
              会话教训分流：<strong style="font-variant-numeric: tabular-nums;">待消费 {{ stats.lessons?.pending || 0 }}</strong>
              <span style="color: var(--text-muted);">/ 已消费 {{ stats.lessons?.acked || 0 }}（转 Forge 反思输入）</span>
            </span>
            <span data-testid="stats-sources">
              条目来源：<strong style="font-variant-numeric: tabular-nums;">AI 总结 {{ stats.habits_by_source?.ai || 0 }}</strong>
              <span style="color: var(--text-muted);">/ 模板兜底 {{ stats.habits_by_source?.template || 0 }} / 观察转正 {{ stats.habits_by_source?.observation || 0 }} / 外部 {{ stats.habits_by_source?.external || 0 }} / 手动 {{ stats.habits_by_source?.manual || 0 }}</span>
            </span>
            <span v-if="stats.observations" data-testid="stats-observations">
              待确认观察：<strong style="font-variant-numeric: tabular-nums;">{{ stats.observations?.pending || 0 }}</strong>
              <span style="color: var(--text-muted);">/ 已确认 {{ stats.observations?.confirmed || 0 }} / 已忽略 {{ stats.observations?.dismissed || 0 }}</span>
            </span>
            <span data-testid="stats-signals">
              近期强信号：<strong style="font-variant-numeric: tabular-nums;">审批卡 {{ stats.signals_recent?.approval_cards || 0 }}</strong>
              <span style="color: var(--text-muted);"> · 急停 {{ stats.signals_recent?.estop || 0 }}</span>
            </span>
          </div>
        </div>

        <!-- 待确认观察卡（接口二：反思边缘结论人工裁决；确认入画像 / 忽略负样本） -->
        <div v-if="observations.length" class="card" style="margin-bottom: var(--space-4);" data-testid="observations-card">
          <div class="card-header">
            <h3>待确认观察</h3>
            <span style="font-size: var(--text-xs); color: var(--text-muted);">
              反思通道不够确定的结论（共 {{ obsCounts?.pending || observations.length }} 条待裁决）
            </span>
          </div>
          <div class="card-body">
            <div class="table-wrap">
              <table>
                <thead>
                  <tr><th style="width: 110px;">类别</th><th>观察</th><th>置信度</th><th style="width: 160px;">操作</th></tr>
                </thead>
                <tbody>
                  <tr v-for="(o, i) in observations" :key="o.id" :data-testid="'obs-row-' + i">
                    <td>
                      <span class="badge badge-info">{{ categoryLabel(o.category) }}</span>
                      <span
                        class="badge"
                        :class="o.scope === 'project' ? 'badge-warning' : 'badge-info'"
                        style="margin-left: var(--space-1);"
                        :data-testid="'obs-scope-' + i"
                        :title="o.scope === 'project' ? '确认后入项目画像' : '确认后入对话画像'"
                      >{{ o.scope === 'project' ? '项目' : '对话' }}</span>
                    </td>
                    <td style="max-width: 420px;">
                      {{ o.content }}
                      <ol v-if="o.steps && o.steps.length" style="margin: var(--space-1) 0 0; padding-left: var(--space-5); font-size: var(--text-xs); color: var(--text-secondary);">
                        <li v-for="(s, si) in o.steps" :key="si" :data-testid="'obs-step-' + i + '-' + si">{{ s }}</li>
                      </ol>
                    </td>
                    <td style="font-variant-numeric: tabular-nums;">{{ (o.confidence * 100).toFixed(0) }}%</td>
                    <td style="white-space: nowrap;">
                      <button class="btn btn-primary" style="font-size: var(--text-xs); margin-right: var(--space-1);" :data-testid="'obs-confirm-' + i" @click="respondObservation(o, true)">确认入画像</button>
                      <button class="btn" style="font-size: var(--text-xs);" :data-testid="'obs-dismiss-' + i" @click="respondObservation(o, false)">忽略</button>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <div class="card">
          <div class="card-header">
            <h3>习惯条目</h3>
            <span style="font-size: var(--text-sm); color: var(--text-secondary);">{{ habits.length }} 条</span>
          </div>
          <div class="card-body">
            <div v-if="habits.length === 0" class="empty-state" data-testid="empty-state">
              <p>{{ enabled ? '暂无习惯条目——正常使用若干会话后自动生成' : '画像未开启（config user_profile.enabled）' }}</p>
            </div>
            <div v-else class="table-wrap">
              <table>
                <thead>
                  <tr><th style="width: 110px;">维度 / 类别</th><th style="width: 56px;">归属</th><th style="width: 44px; white-space: nowrap;" data-testid="src-dot-col-head" title="蓝=内置触发；绿=AI 总结；灰=模板兜底；橙=反思观察；紫=外部；青=手动">来源</th><th>习惯</th><th>置信度</th><th>命中</th><th>状态</th><th>操作</th></tr>
                </thead>
                <tbody v-for="g in grouped" :key="g.label">
                  <tr class="group-row">
                    <td colspan="8" style="font-size: var(--text-xs); font-weight: 600; color: var(--text-secondary); background: var(--bg-secondary, transparent); padding-top: var(--space-3);">
                      {{ g.label }} <span style="font-weight: 400; color: var(--text-muted);">（{{ g.items.length }}）</span>
                    </td>
                  </tr>
                  <tr v-for="it in g.items" :key="it.idx" :data-testid="'habit-row-' + it.idx">
                    <td><span class="badge badge-info">{{ categoryLabel(it.h.category) }}</span></td>
                    <td><span
                      class="badge"
                      :class="it.h._scope === 'project' ? 'badge-warning' : 'badge-info'"
                      :data-testid="'scope-badge-' + it.idx"
                      :title="it.h._scope === 'project' ? '项目画像（项目组会话注入）' : '对话画像（对话组会话注入）'"
                    >{{ it.h._scope === 'project' ? '项目' : '对话' }}</span></td>
                    <td style="text-align: center;"><span class="src-dot" :class="dotClass(it.h)" :data-testid="'src-dot-' + it.idx" :title="dotTitle(it.h)"></span></td>
                    <td style="max-width: 420px;">
                      {{ it.h.content }}
                      <span v-if="it.h.pinned" class="badge badge-neutral" style="margin-left: var(--space-1);">📌 pinned</span>
                      <ol v-if="it.h.steps && it.h.steps.length" class="habit-steps" style="margin: var(--space-1) 0 0; padding-left: var(--space-5); font-size: var(--text-xs); color: var(--text-secondary);">
                        <li v-for="(s, si) in it.h.steps" :key="si" :data-testid="'habit-step-' + it.idx + '-' + si">{{ s }}</li>
                      </ol>
                    </td>
                    <td style="font-variant-numeric: tabular-nums;">{{ (it.h.confidence * 100).toFixed(0) }}%</td>
                    <td :title="it.h.evidence && it.h.evidence.length ? '溯源（内置规则命中凭据）：' + it.h.evidence.join('、') : undefined" :data-testid="'habit-hits-' + it.idx" :class="{ 'hits-with-evidence': it.h.evidence && it.h.evidence.length }">{{ it.h.hits }}</td>
                    <td><span class="badge" :class="statusClass(it.h.status)" :data-testid="'habit-status-' + it.idx">{{ statusLabel(it.h.status) }}</span></td>
                    <td style="white-space: nowrap;">
                      <button class="btn" style="font-size: var(--text-xs); margin-right: var(--space-1);" :data-testid="'move-' + it.idx" :title="'搬到' + (it.h._scope === 'project' ? '对话' : '项目') + '画像：位置分型判错的修正通道（AI 结论落错 scope / 想手动归组时用）'" @click="moveHabit(it.h)">搬到{{ it.h._scope === 'project' ? '对话' : '项目' }}</button>
                      <button class="btn" style="font-size: var(--text-xs); margin-right: var(--space-1);" :data-testid="'pin-' + it.idx" :title="pinTitle(it.h)" @click="pin(it.h)">{{ it.h.pinned ? '取消钉住' : '钉住' }}</button>
                      <button class="btn" style="font-size: var(--text-xs); margin-right: var(--space-1);" :data-testid="'veto-' + it.idx" :title="VETO_TITLE" @click="veto(it.h)">否决</button>
                      <button class="btn" style="font-size: var(--text-xs);" :data-testid="'forget-' + it.idx" :title="FORGET_TITLE" @click="forget(it.h)">遗忘</button>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>
        </div><!-- /TAB2 习惯条目 -->
      </div>
    </div>
  </div>
</template>
