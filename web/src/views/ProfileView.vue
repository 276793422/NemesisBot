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
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const enabled = ref(false)
const enabledAt = ref<string | null>(null)
const uiTelemetry = ref(true)
const habits = ref<any[]>([])
const stats = ref<any>(null)
const observations = ref<any[]>([])
const obsCounts = ref<any>(null)
const loading = ref(true)
const telemetryBusy = ref(false)

const CATEGORY_LABEL: Record<string, string> = {
  workflow: '工作流',
  correction: '纠正',
  tool_preference: '工具偏好',
  style: '风格',
  environment: '环境',
  quality_bar: '质量线',
  trust: '信任画像',
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

async function load() {
  loading.value = true
  try {
    const [listData, statsData, obsData] = await Promise.all([
      request('profile', 'list'),
      request('profile', 'stats').catch(() => null),
      request('profile', 'observations').catch(() => null),
    ])
    enabled.value = !!listData?.enabled
    enabledAt.value = listData?.enabled_at || null
    habits.value = listData?.habits || []
    uiTelemetry.value = listData?.ui_telemetry !== false
    stats.value = statsData?.stats || null
    observations.value = obsData?.observations || []
    obsCounts.value = obsData?.counts || null
  } catch { /* ignore */ } finally {
    loading.value = false
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

async function act(cmd: string, content: string, tip: string) {
  try {
    const data = await request('profile', cmd, { content })
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

function veto(h: any) { act('veto', h.content, '已否决（负样本，不再生成）') }
function forget(h: any) { act('forget', h.content, '已遗忘（同语义可重新生成）') }
function pin(h: any) { act('pin', h.content, h.pinned ? '已取消钉住' : '已钉住（不衰减、置顶注入）') }
</script>

<template>
  <div class="page-profile">
    <div class="page-header">
      <h2>用户习惯</h2>
      <div class="page-header-actions">
        <span v-if="enabled && enabledAt" style="font-size: var(--text-xs); color: var(--text-muted);" data-testid="enabled-at">
          采集起点：{{ new Date(enabledAt).toLocaleString('zh-CN') }}
        </span>
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
              条目来源：<strong style="font-variant-numeric: tabular-nums;">自动 {{ stats.habits_by_source?.auto || 0 }}</strong>
              <span style="color: var(--text-muted);">/ 手动 {{ stats.habits_by_source?.manual || 0 }} / 外部 {{ stats.habits_by_source?.external || 0 }}</span>
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
                    <td><span class="badge badge-info">{{ categoryLabel(o.category) }}</span></td>
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
                  <tr><th style="width: 110px;">维度 / 类别</th><th>习惯</th><th>置信度</th><th>命中</th><th>状态</th><th>操作</th></tr>
                </thead>
                <tbody v-for="g in grouped" :key="g.label">
                  <tr class="group-row">
                    <td colspan="6" style="font-size: var(--text-xs); font-weight: 600; color: var(--text-secondary); background: var(--bg-secondary, transparent); padding-top: var(--space-3);">
                      {{ g.label }} <span style="font-weight: 400; color: var(--text-muted);">（{{ g.items.length }}）</span>
                    </td>
                  </tr>
                  <tr v-for="it in g.items" :key="it.idx" :data-testid="'habit-row-' + it.idx">
                    <td><span class="badge badge-info">{{ categoryLabel(it.h.category) }}</span></td>
                    <td style="max-width: 420px;">
                      {{ it.h.content }}
                      <span v-if="it.h.auto" class="badge badge-neutral" style="margin-left: var(--space-1);" data-testid="auto-badge">自动挖掘</span>
                      <span v-if="it.h.source" class="badge badge-info" style="margin-left: var(--space-1);" :data-testid="'source-badge-' + it.idx" title="外部经验（由其他学习子系统抛入，来源已标记）">来自 {{ sourceLabel(it.h.source) }}</span>
                      <span v-if="it.h.pinned" class="badge badge-neutral" style="margin-left: var(--space-1);">📌 pinned</span>
                      <ol v-if="it.h.steps && it.h.steps.length" class="habit-steps" style="margin: var(--space-1) 0 0; padding-left: var(--space-5); font-size: var(--text-xs); color: var(--text-secondary);">
                        <li v-for="(s, si) in it.h.steps" :key="si" :data-testid="'habit-step-' + it.idx + '-' + si">{{ s }}</li>
                      </ol>
                      <div v-if="it.h.evidence && it.h.evidence.length" style="margin-top: var(--space-1); font-size: var(--text-xs); color: var(--text-muted);">
                        溯源：{{ it.h.evidence.join('、') }}
                      </div>
                    </td>
                    <td style="font-variant-numeric: tabular-nums;">{{ (it.h.confidence * 100).toFixed(0) }}%</td>
                    <td>{{ it.h.hits }}</td>
                    <td><span class="badge" :class="statusClass(it.h.status)" :data-testid="'habit-status-' + it.idx">{{ statusLabel(it.h.status) }}</span></td>
                    <td style="white-space: nowrap;">
                      <button class="btn" style="font-size: var(--text-xs); margin-right: var(--space-1);" :data-testid="'pin-' + it.idx" @click="pin(it.h)">{{ it.h.pinned ? '取消钉住' : '钉住' }}</button>
                      <button class="btn" style="font-size: var(--text-xs); margin-right: var(--space-1);" :data-testid="'veto-' + it.idx" @click="veto(it.h)">否决</button>
                      <button class="btn" style="font-size: var(--text-xs);" :data-testid="'forget-' + it.idx" @click="forget(it.h)">遗忘</button>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
