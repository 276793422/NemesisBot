<script setup lang="ts">
import { ref, computed, onMounted, watch, defineAsyncComponent } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { useEditorMode } from '../composables/useEditorMode'
import { useSessionAllowAll } from '../composables/useSessionAllowAll'
import { useSessionStore } from '../stores/session'
import { useTheme } from '../composables/useTheme'

// 皮肤管理面板（skins feature / VITE_FEATURE_SKINS 门控）：`!== 'false'`
// = 默认开（与 router 门控同一约定）；env 为 'false' 时 Vite 把死分支
// tree-shake 掉，面板代码不进 bundle（IoT 裁剪面）。
const SkinsPanel = import.meta.env.VITE_FEATURE_SKINS !== 'false'
  ? defineAsyncComponent(() => import('../components/settings/SkinsPanel.vue'))
  : null

const { request } = useWSAPI()
const toast = useToast()

// 配色 TAB（2026-10-29）：亮暗模式切换从侧边栏迁入——后续 UI 配色自定义
// 的统一入口。状态与持久化仍在 useTheme 单例（localStorage），按钮挪位
// 零迁移。
const { theme, setTheme } = useTheme()

const activeTab = ref('agent')
const config = ref<any>({})
const loading = ref(true)
const editing = ref(false)
const editConfig = ref('')

// CORS state
const corsOrigins = ref<string[]>([])
const corsEnabled = ref(false)
const corsError = ref('')
const newOrigin = ref('')

const tabs = [
  { id: 'agent', label: 'Agent' },
  { id: 'gateway', label: 'Gateway' },
  { id: 'tools', label: '工具' },
  { id: 'services', label: '服务开关' },
  { id: 'editor', label: '编辑器' },
  { id: 'logging', label: '日志' },
  // 数据治理 tab（2026-11-08 配置页补全批次 2）：日志 16 族 TTL / checkpoint
  // gc / 磁盘水位 / 失败诊断捕获——全部 config.set_field（typed 段已有）。
  { id: 'data', label: '数据治理' },
  { id: 'cors', label: 'CORS' },
  // 配色 tab（2026-10-29）：亮暗模式切换（侧边栏原位换【打开主目录】），
  // 后续 UI 配色自定义统一入口；与皮肤 tab 相邻（外观类）。
  { id: 'appearance', label: '配色' },
  // 皮肤 tab（skins feature 门控；skins 关 = tab 不出现）
  ...(import.meta.env.VITE_FEATURE_SKINS !== 'false' ? [{ id: 'skins', label: '皮肤' }] : []),
  { id: 'raw', label: '原始 JSON' },
]

// --- Full Access 编辑器放行开关（2026-09-20 用户裁决）---
// 与聊天框旁按钮同一状态（useEditorMode 模块级单例——单一真相源）。
// 点击语义与 ChatPanel 同款：关 FA 随关 ext（服务端联动会把 (false, true)
// 折回 full=true，显式双关才能真关）；开 ext 确保 full 开。
const { fullAccess, externalWrite, editorAvailable, setEditorAccess } = useEditorMode()

async function toggleEditorFull() {
  if (fullAccess.value) {
    await setEditorAccess(false, false)
  } else {
    await setEditorAccess(true, externalWrite.value)
  }
}

async function toggleEditorExt() {
  if (!fullAccess.value) return // UI 联动：依赖 Full Access
  await setEditorAccess(true, !externalWrite.value)
}

// --- 会话级全部放行（2026-10-24 长任务无人值守计划 Feature A）---
// 与聊天框旁按钮同一状态（useSessionAllowAll 模块级单例）。作用域是
// 当前会话（持久 sidecar，无 TTL、重启自动恢复）；设置页只操作当前会话。
const sessionStore = useSessionStore()
const { allowAllBySession, sessionAllowAvailable, refreshSessionAllow, setSessionAllow } =
  useSessionAllowAll()

const currentSessionAllow = ref(false)
const currentSessionSince = ref<string | null>(null)
const currentSessionAllowCount = ref(0)

watch(
  () => sessionStore.currentId,
  async (sid) => {
    if (!sid) return
    await refreshSessionAllow(sid)
    syncAllowDisplay()
  },
  { immediate: true }
)

/** 单例分桶 → 本页展示字段投影。 */
function syncAllowDisplay() {
  const sid = sessionStore.currentId
  const st = sid ? allowAllBySession.value[sid] : undefined
  currentSessionAllow.value = !!st?.enabled
  currentSessionSince.value = st?.since ?? null
  currentSessionAllowCount.value = st?.count ?? 0
}

async function toggleSessionAllow() {
  const sid = sessionStore.currentId
  if (!sid) {
    toast.error('当前没有活动会话')
    return
  }
  await setSessionAllow(sid, !currentSessionAllow.value)
  syncAllowDisplay()
}

async function loadConfig() {
  try {
    const data = await request('config', 'get')
    config.value = data || {}
  } catch (e: any) {
    toast.error('加载配置失败: ' + e)
  }
  loading.value = false
}

async function saveField(path: string, value: any) {
  try {
    await request('config', 'set_field', { path, value })
    toast.success(`已更新 ${path}`)
    await loadConfig()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

async function saveFullConfig() {
  try {
    const parsed = JSON.parse(editConfig.value)
    await request('config', 'save', parsed)
    toast.success('配置已保存')
    editing.value = false
    await loadConfig()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

// CORS functions（C-1 接真：后端消费 gateway 注入的 CORSManager；
// CorsLayer 启动时静态构建 → 改动重启 gateway 后生效）
async function loadCors() {
  try {
    const data = await request('config', 'cors.list')
    corsOrigins.value = data?.origins || []
    corsEnabled.value = data?.development_mode === true
    corsError.value = ''
  } catch (e: any) {
    // manager 未注入（cors.json 缺席或解析失败，gateway 启动期装配）——
    // 诚实展示错误而非伪装成空列表。
    corsOrigins.value = []
    corsEnabled.value = false
    corsError.value = String(e)
  }
}

async function addCorsOrigin() {
  if (!newOrigin.value) return
  try {
    await request('config', 'cors.add', { origin: newOrigin.value })
    toast.success('已添加')
    newOrigin.value = ''
    await loadCors()
  } catch (e: any) {
    toast.error('添加失败: ' + e)
  }
}

async function removeCorsOrigin(origin: string) {
  try {
    await request('config', 'cors.remove', { origin })
    toast.success('已移除')
    await loadCors()
  } catch (e: any) {
    toast.error('移除失败: ' + e)
  }
}

async function toggleCors(enabled: boolean) {
  try {
    await request('config', 'cors.toggle', { enabled })
    corsEnabled.value = enabled
    toast.success(enabled ? '已启用' : '已禁用')
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

function toggleService(path: string, current: boolean) {
  saveField(path, !current)
}

// --- 批次 4 键补全（2026-11-08 配置页补全计划）：展示缺省兜底同源
// nemesis-config serde default（键缺席时 typed 段走 Default，前端展示
// 同一缺省值，与数据治理 TAB 同一纪律）。---

/** 通用数字保存（非法值回退展示缺省并不落盘）。 */
function saveNum(path: string, e: any, fallback: number, min = -Infinity, max = Infinity) {
  const n = parseFloat(e.target.value)
  if (Number.isNaN(n) || n < min || n > max) {
    e.target.value = String(fallback)
    toast.error(`须为 ${min === -Infinity ? '' : '≥' + min}${max !== Infinity ? '≤' + max : ''} 的数字（已回退 ${fallback}）`)
    return
  }
  saveField(path, n)
}

/** 密钥保存（写入端掩码纪律）：set_field 无掩码还原（整档 save 才有
 * restore_masked_named_fields）——输入框恒空、placeholder 显示配置态，
 * 输入非空才落盘；粘贴掩码串（含 ****）拒绝，防止掩码覆盖真值。 */
async function saveSecret(path: string, e: any) {
  const v = String(e.target.value).trim()
  e.target.value = ''
  if (!v) return // 空输入 = 未表态，不动存量
  if (v.includes('****')) {
    toast.error('检测到掩码串（****）——已拒绝写入；如需更换请输入完整新 Key')
    return
  }
  await saveField(path, v)
}

// hidden_tools（供给+dispatch 双闸标签表；逗号/空格/分号分隔）。
const hiddenToolsText = computed(() =>
  ((config.value.agents?.hidden_tools as string[] | undefined) ?? []).join(', '),
)
function saveHiddenTools(e: any) {
  const arr = String(e.target.value)
    .split(/[\s,，;；]+/)
    .map((s) => s.trim())
    .filter(Boolean)
  saveField('agents.hidden_tools', arr)
}

// exec deny patterns（tools.exec；装配期一次读——重启生效标注见模板）。
const execDenyPatternsText = computed(() =>
  ((config.value.tools?.exec?.custom_deny_patterns as string[] | undefined) ?? []).join('\n'),
)
function saveExecDenyPatterns(e: any) {
  const arr = String(e.target.value)
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter(Boolean)
  saveField('tools.exec.custom_deny_patterns', arr)
}

// hardware i2c 地址段（[[lo,hi],...]；一行一段 lo-hi 形态输入）。
function i2cRangesText(v: unknown): string {
  const arr = Array.isArray(v) ? (v as unknown[]) : []
  return arr
    .map((r: any) => (Array.isArray(r) && r.length === 2 ? `${r[0]}-${r[1]}` : ''))
    .filter(Boolean)
    .join('\n')
}
function saveI2cRanges(path: string, e: any) {
  const ranges: [number, number][] = []
  for (const line of String(e.target.value).split(/\r?\n/)) {
    const t = line.trim()
    if (!t) continue
    const m = t.match(/^(\d+)\s*-\s*(\d+)$/)
    const lo = m ? parseInt(m[1], 10) : NaN
    const hi = m ? parseInt(m[2], 10) : NaN
    if (!m || Number.isNaN(lo) || Number.isNaN(hi) || lo > 255 || hi > 255 || lo > hi) {
      toast.error(`地址段「${t}」格式须为 lo-hi（0-255 且 lo ≤ hi），未保存`)
      return
    }
    ranges.push([lo, hi])
  }
  saveField(path, ranges)
}

// gateway host/port 可编辑（改动需重启 gateway 进程——监听器装配期读）。
async function saveGatewayHost(e: any) {
  const s = String(e.target.value).trim()
  if (!s) {
    e.target.value = config.value.gateway?.host || '0.0.0.0'
    toast.error('host 不能为空（已回退）')
    return
  }
  await saveField('gateway.host', s)
  toast.info('⚠ host 改动需重启 gateway 进程后生效（当前连接不受影响）')
}
async function saveGatewayPort(e: any) {
  const n = parseInt(e.target.value, 10)
  if (Number.isNaN(n) || n < 1 || n > 65535) {
    e.target.value = String(config.value.gateway?.port ?? 18790)
    toast.error('端口须为 1-65535 整数（已回退）')
    return
  }
  await saveField('gateway.port', n)
  toast.info('⚠ 端口改动需重启 gateway 进程后生效（重启后用新端口访问 Dashboard）')
}

// 实验性高级折叠（agent TAB 底部）：行为面开关，标注风险与生效方式。
const expAdvOpen = ref(false)
// tools TAB 内两个高级折叠。
const hwAdvOpen = ref(false)
const denyAdvOpen = ref(false)
// services TAB 终端高级折叠。
const termAdvOpen = ref(false)
// services TAB 底部高级 JSON 只读折叠（批次 5）：bindings[]/session.*/

// executor.persistent.*——结构性低频键，只展示 + 手改指引，不铺表单。
const advJsonOpen = ref(false)
// 只读展示的 JSON 片段（config ref 派生；缺席段显示 typed 缺省注记）。
function prettyOr(v: unknown, fallbackNote: string): string {
  if (v === undefined || v === null) return `（未配置 — ${fallbackNote}）`
  return JSON.stringify(v, null, 2)
}
const advJsonBlocks = computed(() => [
  {
    title: 'bindings（agent 绑定表）',
    json: prettyOr(config.value.bindings, '空数组 = 无绑定'),
    hint: '条目 {agent_id, match:{channel, account_id, peer:{kind,id}, guild_id, team_id}}——按入站消息来源把会话绑定到指定 agent。改动重启生效。',
  },
  {
    title: 'session（会话身份）',
    json: prettyOr(config.value.session
      ? { dm_scope: config.value.session.dm_scope ?? '', identity_links: config.value.session.identity_links ?? {} }
      : undefined, 'dm_scope 空串 = 缺省；identity_links 空表 = 无链接'),
    hint: 'dm_scope：DM 会话键作用域（如 per-peer）；identity_links：跨渠道身份归并映射（渠道身份 → 统一身份）。改动重启生效。',
  },
  {
    title: 'executor.persistent（常驻执行体）',
    json: prettyOr(config.value.executor?.persistent, '全关缺省：enabled=false, max_slots=4, idle_shutdown_secs=300, call_timeout_secs=600'),
    hint: 'per-agent 长驻 executor 子进程组：enabled 总闸 / max_slots 并发上限 / idle_shutdown_secs 空闲自动退（0=不睡）/ call_timeout_secs 单调用超时。改动重启 Agent 生效。',
  },
])

// --- 数据治理 TAB（2026-11-08 配置页补全批次 2）---
// 全部走 config.set_field（config.json typed 段：logs/checkpoint/health/
// debug 缺席时 serde 走 Default，因此前端展示用同源缺省值兜底）。
const dataAdvOpen = ref(false)

// 日志 16 族（与 nemesis-config LogsTtlConfig 字段一一对应）。
const LOG_TTL_FAMILIES: { key: string; label: string; hint?: string }[] = [
  { key: 'session_logs', label: '对话历史' },
  { key: 'boundary', label: '轮次边界' },
  { key: 'request_logs', label: 'LLM 请求日志' },
  { key: 'terminal', label: '终端审计', hint: '含回显密码，敏感面' },
  { key: 'event_ledger', label: '对话账本' },
  { key: 'trash', label: '会话回收站' },
  { key: 'gateway', label: '网关日志' },
  { key: 'cluster_logs', label: '集群日志' },
  { key: 'eval', label: '评估报告' },
  { key: 'capture', label: '失败诊断' },
  { key: 'dreaming', label: 'Dreaming 报告' },
  { key: 'calibration', label: '校准报告' },
  { key: 'deliveries', label: '交付留底' },
  { key: 'heartbeat', label: '心跳日志' },
  { key: 'ziwei', label: '紫微产物' },
  { key: 'exec_checkpoints', label: '检查点孤儿', hint: '崩溃残留，默认 1 天快清' },
]

// 缺省值（nemesis-config LogsTtlConfig::default 同源）。
const LOG_TTL_DEFAULTS: Record<string, number> = {
  session_logs: 30, boundary: 30, request_logs: 14, terminal: 7,
  event_ledger: 30, trash: 90, gateway: 14, cluster_logs: 14,
  eval: 30, capture: 30, dreaming: 30, calibration: 30,
  deliveries: 30, heartbeat: 30, ziwei: 30, exec_checkpoints: 1,
}

function ttlOf(key: string): number {
  const v = config.value.logs?.[key]
  return typeof v === 'number' ? v : LOG_TTL_DEFAULTS[key]
}

// 全 16 族同值且落在预设档值域 → 该档高亮；否则显示「自定义」。
const ttlPresetValue = computed(() => {
  const vals = LOG_TTL_FAMILIES.map(f => ttlOf(f.key))
  const uniform = vals.every(v => v === vals[0]) ? vals[0] : null
  return uniform === 7 || uniform === 30 || uniform === 90 ? String(uniform) : ''
})

async function applyTtlPreset(days: number) {
  try {
    for (const f of LOG_TTL_FAMILIES) {
      await request('config', 'set_field', { path: `logs.${f.key}`, value: days })
    }
    toast.success(`已将全部 ${LOG_TTL_FAMILIES.length} 族日志保留期设为 ${days} 天`)
  } catch (e: any) {
    toast.error('预设失败: ' + e)
  }
  await loadConfig()
}

function saveTtlFamily(key: string, e: any) {
  const n = parseInt(e.target.value, 10)
  if (Number.isNaN(n) || n < 0) {
    e.target.value = String(ttlOf(key))
    toast.error('保留天数须为 ≥0 整数（0=关闭该族清扫）')
    return
  }
  saveField(`logs.${key}`, n)
}

// checkpoint.gc 缺省（enabled=false / keep_turns=200 / keep_days=7）。
function gcField(path: 'enabled' | 'keep_turns' | 'keep_days'): any {
  const gc = config.value.checkpoint?.gc
  if (path === 'enabled') return gc?.enabled === true
  if (path === 'keep_turns') return typeof gc?.keep_turns === 'number' ? gc.keep_turns : 200
  return typeof gc?.keep_days === 'number' ? gc.keep_days : 7
}

// disk_watermark 缺省（enabled=true / warn 5GB / critical 1GB / 60s）。
function wmField(path: 'enabled' | 'warn_gb' | 'critical_gb' | 'interval_secs'): any {
  const wm = config.value.health?.disk_watermark
  if (path === 'enabled') return wm?.enabled !== false
  if (path === 'warn_gb') return typeof wm?.warn_gb === 'number' ? wm.warn_gb : 5
  if (path === 'critical_gb') return typeof wm?.critical_gb === 'number' ? wm.critical_gb : 1
  return typeof wm?.interval_secs === 'number' ? wm.interval_secs : 60
}

// --- WS9/P22（2026-09-26）：工作区写租约现状（Gateway 页签卡片）---
// `system.lease_status` 探针无副作用（试取锁立即释放 + 读持有者 sidecar）。
// 展示语义：supported=false = 未装配（headless/未知 workspace）；held =
// 当前持有者；free 但有 sidecar = 陈旧记录（后端 note 已诚实标注）。
const leaseStatus = ref<any>(null)

async function loadLease() {
  try {
    leaseStatus.value = await request('system', 'lease_status')
  } catch {
    leaseStatus.value = null
  }
}

function fmtLeaseTime(ts?: string): string {
  if (!ts) return ''
  const d = new Date(ts)
  return isNaN(d.getTime()) ? ts : d.toLocaleString('zh-CN', { hour12: false })
}

// Hooks 功能已于 2026-08-29 迁移至独立页面 views/HookView.vue（管理分组）。

onMounted(async () => {
  await Promise.all([loadConfig(), loadCors(), loadLease()])
})
</script>

<template>
  <div class="page-settings">
    <div class="page-header"><h2>设置</h2></div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <div class="tabs">
          <button v-for="t in tabs" :key="t.id" class="tab" :class="{ active: activeTab === t.id }" @click="activeTab = t.id">{{ t.label }}</button>
        </div>

        <!-- Agent config -->
        <div v-if="activeTab === 'agent'" class="card">
          <div class="card-header"><h3>Agent 配置</h3></div>
          <div class="card-body">
            <div class="form-group">
              <label class="form-label">默认模型</label>
              <input class="form-input" :value="config.agents?.defaults?.llm || '--'" disabled style="max-width: 300px;">
              <span class="form-hint">在模型页面修改</span>
            </div>
            <div class="form-group">
              <label class="form-label">温度</label>
              <input class="form-input" type="number" step="0.1" min="0" max="2" :value="config.agents?.defaults?.temperature ?? 0.7"
                @change="(e: any) => saveField('agents.defaults.temperature', parseFloat(e.target.value))" style="max-width: 200px;">
            </div>
            <div class="form-group">
              <label class="form-label">最大 Tokens</label>
              <input class="form-input" type="number" :value="config.agents?.defaults?.max_tokens ?? 4096"
                @change="(e: any) => saveField('agents.defaults.max_tokens', parseInt(e.target.value))" style="max-width: 200px;">
            </div>
            <div class="form-group">
              <label class="form-label">工作空间限制</label>
              <div class="toggle" :class="{ active: config.agents?.defaults?.restrict_to_workspace !== false }"
                @click="toggleService('agents.defaults.restrict_to_workspace', config.agents?.defaults?.restrict_to_workspace !== false)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.agents?.defaults?.restrict_to_workspace !== false ? '已启用' : '已禁用' }}</span>
            </div>
            <!-- 批次 4：agent TAB 高频键补全 -->
            <div class="form-group">
              <label class="form-label">最大工具轮数（max_tool_iterations）</label>
              <input class="form-input" type="number" min="0" style="max-width: 200px;" data-test="agent-max-tool-iterations"
                :value="config.agents?.defaults?.max_tool_iterations ?? 100"
                @change="(e: any) => saveNum('agents.defaults.max_tool_iterations', e, 100, 0)">
              <span class="form-hint">单任务工具调用轮数上限（0=不限）；到顶先给一轮收尾机会再暂停，不硬崩。</span>
            </div>
            <div class="form-group">
              <label class="form-label">限流重试次数（rate_limit_retries）</label>
              <input class="form-input" type="number" min="0" style="max-width: 200px;"
                :value="config.agents?.defaults?.rate_limit_retries ?? 10"
                @change="(e: any) => saveNum('agents.defaults.rate_limit_retries', e, 10, 0)">
              <span class="form-hint">上游 429/5xx 限流时的自动重试预算，耗尽后诚实报错。</span>
            </div>
            <div class="form-group">
              <label class="form-label">并发轮上限（max_concurrent_turns）</label>
              <input class="form-input" type="number" min="1" style="max-width: 200px;"
                :value="config.agents?.defaults?.max_concurrent_turns ?? 8"
                @change="(e: any) => saveNum('agents.defaults.max_concurrent_turns', e, 8, 1)">
              <span class="form-hint">同时运行的 LLM 轮数上限；超出走并发请求模式（代码开发页）。</span>
            </div>
            <div class="form-group">
              <label class="form-label">压缩尾巴保留（compact_keep_recent_tokens）</label>
              <input class="form-input" type="number" min="1" style="max-width: 200px;"
                :value="config.agents?.defaults?.compact_keep_recent_tokens ?? 20000"
                @change="(e: any) => saveNum('agents.defaults.compact_keep_recent_tokens', e, 20000, 1)">
              <span class="form-hint">上下文压缩时保留最近 N token 原文；须小于模型压缩点。</span>
            </div>
            <div class="form-group">
              <label class="form-label">会话 token 预算（session_token_budget）</label>
              <input class="form-input" type="number" min="0" style="max-width: 200px;"
                :value="config.agents?.defaults?.session_token_budget ?? 0"
                @change="(e: any) => saveNum('agents.defaults.session_token_budget', e, 0, 0)">
              <span class="form-hint">单会话累计 token 上限（0=关，默认）。</span>
            </div>
            <div class="form-group">
              <label class="form-label">单轮输出上限（max_output_ceiling）</label>
              <input class="form-input" type="number" min="1024" style="max-width: 200px;"
                :value="config.agents?.defaults?.max_output_ceiling ?? 262144"
                @change="(e: any) => saveNum('agents.defaults.max_output_ceiling', e, 262144, 1024)">
              <span class="form-hint">max_tokens 硬顶钳制（默认 262144）。</span>
            </div>
            <div class="form-group">
              <label class="form-label">溢出保留天数（spill_retention_days）</label>
              <input class="form-input" type="number" min="1" style="max-width: 200px;"
                :value="config.agents?.defaults?.spill_retention_days ?? 7"
                @change="(e: any) => saveNum('agents.defaults.spill_retention_days', e, 7, 1)">
              <span class="form-hint">上下文溢出落盘文件的保留天数。</span>
            </div>
            <div class="form-group">
              <label class="form-label">保存后格式化（format_on_save）</label>
              <div class="toggle" :class="{ active: config.agents?.defaults?.format_on_save?.enabled === true }"
                @click="toggleService('agents.defaults.format_on_save.enabled', config.agents?.defaults?.format_on_save?.enabled === true)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.agents?.defaults?.format_on_save?.enabled === true ? '已启用' : '已禁用（默认）' }}</span>
              <div class="form-hint">write/edit 落盘后跑格式化钩子（内置表按扩展名映射；单次墙钟预算默认 3s，慢机可在原始 JSON 调大 timeout_secs）。</div>
            </div>
            <div class="form-group">
              <label class="form-label">图片自动降采样（image_downscale）</label>
              <div class="toggle" :class="{ active: config.agents?.image_downscale !== false }"
                @click="toggleService('agents.image_downscale', config.agents?.image_downscale !== false)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.agents?.image_downscale !== false ? '已启用（默认）' : '已禁用' }}</span>
              <div class="form-hint">超限图片自动降分辨率重编码（4096→2048→1024 阶梯 + JPEG），不支持视觉的模型照常剥图。</div>
            </div>
            <div class="form-group">
              <label class="form-label">工具结果截断（tool_result_cap）</label>
              <input class="form-input" type="number" min="100" style="max-width: 200px;"
                :value="config.agents?.tool_result_cap ?? 8000"
                @change="(e: any) => saveNum('agents.tool_result_cap', e, 8000, 100)">
              <span class="form-hint">单次工具结果注入上下文的字符上限（默认 8000；超出截断留痕）。</span>
            </div>
            <div class="form-group">
              <label class="form-label">子代理嵌套深度（subagent.max_depth）</label>
              <input class="form-input" type="number" min="0" max="8" style="max-width: 200px;"
                :value="config.agents?.subagent?.max_depth ?? 1"
                @change="(e: any) => saveNum('agents.subagent.max_depth', e, 1, 0, 8)">
              <span class="form-hint">子代理还能再生子代理的层数（默认 1 = 不可再生）。</span>
            </div>
            <div class="form-group">
              <label class="form-label">工具隐藏清单（hidden_tools）</label>
              <input class="form-input" data-test="agent-hidden-tools" :value="hiddenToolsText"
                placeholder="如 exec, registry_write（逗号/空格分隔；mcp_ 前缀通配、裸 * 全隐）"
                style="max-width: 520px;" @change="saveHiddenTools">
              <span class="form-hint">隐藏的工具不进模型工具面、也不可被调用（供给+dispatch 双闸，热生效）。</span>
            </div>
            <div class="form-group">
              <label class="form-label">Guardian 审计模型（guardian_model）</label>
              <input class="form-input" :value="config.agents?.guardian_model || ''"
                placeholder="留空 = small_model → 主模型 回落阶梯"
                style="max-width: 360px;"
                @change="(e: any) => saveField('agents.guardian_model', String(e.target.value).trim() || null)">
              <span class="form-hint">LLM 命令审计（guardian judge）专用模型通道；重启 Agent 生效。</span>
            </div>
            <div class="form-group">
              <label class="form-label">Prompt 缓存预热（cache_warmer）</label>
              <div class="toggle" :class="{ active: config.agents?.cache_warmer?.enabled === true }"
                @click="toggleService('agents.cache_warmer.enabled', config.agents?.cache_warmer?.enabled === true)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.agents?.cache_warmer?.enabled === true ? '已启用' : '已禁用（默认，成本敏感 opt-in）' }}</span>
              <input class="form-input" type="number" step="0.01" min="0" style="max-width: 140px; margin-left: var(--space-4);"
                :value="config.agents?.cache_warmer?.cost_limit_usd ?? 0.05"
                @change="(e: any) => saveNum('agents.cache_warmer.cost_limit_usd', e, 0.05, 0)">
              <span class="form-hint" style="margin-left: var(--space-2);">单次 warm 成本上限（USD）</span>
              <div class="form-hint">idle 会话在 prompt cache TTL 到期前用 1-token 重放刷新缓存；启动后才开的预热任务要重启 gateway 才注册。</div>
            </div>
            <div class="form-group">
              <label class="form-label">项目指令信任策略（projects.instruction_trust）</label>
              <select class="form-select" style="max-width: 320px;"
                :value="config.projects?.instruction_trust || ''"
                @change="(e: any) => saveField('projects.instruction_trust', e.target.value)">
                <option value="">不可信项目注入横幅标注（默认）</option>
                <option value="annotate">同默认（显式 annotate）</option>
                <option value="isolated">不可信项目指令整段跳过</option>
              </select>
              <span class="form-hint">未受信项目（逐项目未开信任）的工作区指令文件（CLAUDE.md/AGENTS.md）处理策略；受信条目恒原文注入。</span>
            </div>
            <!-- 实验性高级区（批次 4）：行为面开关，默认全关/内置默认。 -->
            <div class="form-group" style="margin-top: var(--space-4); border-top: 1px dashed var(--border-light); padding-top: var(--space-3);">
              <label class="form-label" style="cursor: pointer; user-select: none;" data-test="exp-adv-toggle" @click="expAdvOpen = !expAdvOpen">
                ⚗ 实验性高级选项 {{ expAdvOpen ? '▲' : '▼' }}
              </label>
              <div v-if="expAdvOpen" style="margin-top: var(--space-2);">
                <div style="padding: var(--space-3); margin-bottom: var(--space-3); background: var(--warning-bg, #fef3cd); border: 1px solid var(--warning, #e5a00d); border-radius: var(--radius-md); font-size: var(--text-sm); color: var(--text-secondary);">
                  实验面：改动影响提示词形态与工具供给行为，不建议日常调整。出问题时回到这里切回默认即可。
                </div>
                <div class="form-group">
                  <label class="form-label">提示词体系（prompt_system）</label>
                  <select class="form-select" style="max-width: 240px;" data-test="prompt-system"
                    :value="config.agents?.prompt_system || 'pro'"
                    @change="(e: any) => saveField('agents.prompt_system', e.target.value)">
                    <option value="pro">pro（现行默认）</option>
                    <option value="classic">classic（旧版精简提示词）</option>
                  </select>
                  <span class="form-hint">AgentLoop 装配期读取——重启 Agent 生效。换体系会改变工具说明与行为约束的组织方式。</span>
                </div>
                <div class="form-group">
                  <label class="form-label">纪律闸（discipline.enabled）</label>
                  <div class="toggle" :class="{ active: config.agents?.discipline?.enabled === true }"
                    @click="toggleService('agents.discipline.enabled', config.agents?.discipline?.enabled === true)"></div>
                  <span class="form-hint" style="margin-left: var(--space-2);">{{ config.agents?.discipline?.enabled === true ? '已启用' : '已禁用（默认）' }}</span>
                  <div class="form-hint">开 = 任务描述含 [discipline:bugfix] 或 /discipline on 的会话进入参与态（声明闸 + 证伪预算）。关 = 全链路不注册。</div>
                </div>
                <div class="form-group">
                  <label class="form-label">工具 schema 惰性供给（mcp_lazy_supply）</label>
                  <div class="toggle" :class="{ active: config.agents?.mcp_lazy_supply === true }"
                    @click="toggleService('agents.mcp_lazy_supply', config.agents?.mcp_lazy_supply === true)"></div>
                  <span class="form-hint" style="margin-left: var(--space-2);">{{ config.agents?.mcp_lazy_supply === true ? '已启用' : '已禁用（默认，随 tier 矩阵）' }}</span>
                  <div class="form-hint">开 = 全档强制惰性供给（schema 首次用到才注入）；小模型场景可降 token，但首轮工具发现多一步。重启 Agent 生效。</div>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Gateway config -->
        <div v-if="activeTab === 'gateway'" class="card">
          <div class="card-header"><h3>Gateway 配置</h3></div>
          <div class="card-body">
            <div class="form-group">
              <label class="form-label">主机（gateway.host）</label>
              <input class="form-input" data-test="gateway-host" :value="config.gateway?.host || '0.0.0.0'"
                style="max-width: 300px;" @change="saveGatewayHost">
              <span class="form-hint">监听地址（0.0.0.0 = 所有网卡）；改动需重启 gateway 进程生效。</span>
            </div>
            <div class="form-group">
              <label class="form-label">端口（gateway.port）</label>
              <input class="form-input" type="number" min="1" max="65535" data-test="gateway-port"
                :value="config.gateway?.port ?? 18790" style="max-width: 200px;" @change="saveGatewayPort">
              <span class="form-hint">Web 服务 + 健康探针共用端口（默认 18790）；改动需重启 gateway 进程生效，重启后用新端口访问 Dashboard。</span>
            </div>
            <!-- WS9/P22：工作区写租约现状（只读探针 + 手动刷新） -->
            <div class="form-group">
              <label class="form-label">工作区写租约</label>
              <div class="lease-line">
                <template v-if="leaseStatus?.supported">
                  <span v-if="leaseStatus.held" class="lease-held">🔒 被占用：{{ leaseStatus.holder || '未知持有者' }}<template v-if="leaseStatus.acquired_at">（自 {{ fmtLeaseTime(leaseStatus.acquired_at) }}）</template></span>
                  <span v-else class="lease-free">🔓 空闲</span>
                </template>
                <span v-else class="lease-free">{{ leaseStatus ? '租约未装配（' + (leaseStatus.note || 'workspace 未知') + '）' : '状态未知' }}</span>
                <button class="lease-refresh" @click="loadLease" title="重新探测">刷新</button>
              </div>
              <div v-if="leaseStatus?.supported && leaseStatus.note" class="form-hint">{{ leaseStatus.note }}</div>
              <div class="form-hint">写类工具执行前需获取工作区写租约；持有进程退出后由 OS 自动释放。</div>
            </div>
          </div>
        </div>

        <!-- Tools config -->
        <div v-if="activeTab === 'tools'" class="card">
          <div class="card-header"><h3>工具配置</h3></div>
          <div class="card-body">
            <div class="form-group">
              <label class="form-label">Brave 搜索</label>
              <div class="toggle" :class="{ active: config.tools?.web?.brave?.enabled === true }"
                @click="toggleService('tools.web.brave.enabled', config.tools?.web?.brave?.enabled === true)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.tools?.web?.brave?.enabled === true ? '已启用' : '已禁用' }}</span>
              <input class="form-input" type="password" data-test="brave-api-key" placeholder="API Key：已配置（输入新值更换）"
                style="max-width: 320px; margin-left: var(--space-4);" @change="(e: any) => saveSecret('tools.web.brave.api_key', e)">
              <span class="form-hint" style="margin-left: var(--space-2);">Key 写入端掩码：输入非空才落盘，空/掩码串不动存量。</span>
            </div>
            <div class="form-group">
              <label class="form-label">Brave 结果条数</label>
              <input class="form-input" type="number" min="1" max="20" style="max-width: 140px;"
                :value="config.tools?.web?.brave?.max_results ?? 5"
                @change="(e: any) => saveNum('tools.web.brave.max_results', e, 5, 1, 20)">
              <span class="form-hint">默认 5。</span>
            </div>
            <div class="form-group">
              <label class="form-label">DuckDuckGo 搜索</label>
              <div class="toggle" :class="{ active: config.tools?.web?.duckduckgo?.enabled === true }"
                @click="toggleService('tools.web.duckduckgo.enabled', config.tools?.web?.duckduckgo?.enabled === true)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.tools?.web?.duckduckgo?.enabled === true ? '已启用' : '已禁用' }}</span>
              <input class="form-input" type="number" min="1" max="20" style="max-width: 140px; margin-left: var(--space-4);"
                :value="config.tools?.web?.duckduckgo?.max_results ?? 5"
                @change="(e: any) => saveNum('tools.web.duckduckgo.max_results', e, 5, 1, 20)">
              <span class="form-hint" style="margin-left: var(--space-2);">结果条数（默认 5）</span>
            </div>
            <div class="form-group">
              <label class="form-label">Perplexity 搜索</label>
              <div class="toggle" :class="{ active: config.tools?.web?.perplexity?.enabled === true }"
                @click="toggleService('tools.web.perplexity.enabled', config.tools?.web?.perplexity?.enabled === true)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.tools?.web?.perplexity?.enabled === true ? '已启用' : '已禁用' }}</span>
              <input class="form-input" type="password" placeholder="API Key：已配置（输入新值更换）"
                style="max-width: 320px; margin-left: var(--space-4);" @change="(e: any) => saveSecret('tools.web.perplexity.api_key', e)">
              <input class="form-input" type="number" min="1" max="20" style="max-width: 140px; margin-left: var(--space-4);"
                :value="config.tools?.web?.perplexity?.max_results ?? 5"
                @change="(e: any) => saveNum('tools.web.perplexity.max_results', e, 5, 1, 20)">
              <span class="form-hint" style="margin-left: var(--space-2);">条数</span>
            </div>
            <div class="form-group">
              <label class="form-label">图像生成（image_gen）</label>
              <input class="form-input" :value="config.tools?.image_gen?.model || ''"
                placeholder="模型别名（留空 = 唯一 images-openai 条目自动解析）"
                style="max-width: 320px;"
                @change="(e: any) => saveField('tools.image_gen.model', String(e.target.value).trim() || null)">
              <input class="form-input" type="number" min="0" style="max-width: 140px; margin-left: var(--space-4);"
                :value="config.tools?.image_gen?.timeout_secs ?? 120"
                @change="(e: any) => saveNum('tools.image_gen.timeout_secs', e, 120, 0)">
              <span class="form-hint" style="margin-left: var(--space-2);">单次生成超时（秒，0/缺省=120）</span>
            </div>
            <div class="form-group">
              <label class="form-label">硬件总线工具（hardware）</label>
              <div class="toggle" :class="{ active: config.tools?.hardware?.enabled === true }"
                @click="toggleService('tools.hardware.enabled', config.tools?.hardware?.enabled === true)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.tools?.hardware?.enabled === true ? '已启用（i2c/spi 注册）' : '已禁用（默认；IoT/嵌入式显式开）' }}</span>
              <span class="form-hint" style="margin-left: var(--space-4); cursor: pointer; user-select: none;" @click="hwAdvOpen = !hwAdvOpen">
                I2C 地址段高级 {{ hwAdvOpen ? '▲' : '▼' }}
              </span>
              <div v-if="hwAdvOpen" style="display: flex; gap: var(--space-4); margin-top: var(--space-2); flex-wrap: wrap;">
                <div>
                  <label class="form-label">允许地址段（allow，闭区间）</label>
                  <textarea class="form-textarea" rows="3" style="min-width: 220px; font-family: var(--font-mono); font-size: var(--text-xs);"
                    placeholder="8-119&#10;每行一段 lo-hi；空 = 内置默认 0x08-0x77"
                    :value="i2cRangesText(config.tools?.hardware?.i2c_allow_ranges)"
                    @change="(e: any) => saveI2cRanges('tools.hardware.i2c_allow_ranges', e)"></textarea>
                </div>
                <div>
                  <label class="form-label">拒绝地址段（deny，优先于 allow）</label>
                  <textarea class="form-textarea" rows="3" style="min-width: 220px; font-family: var(--font-mono); font-size: var(--text-xs);"
                    placeholder="每行一段 lo-hi（如 0-7 屏蔽保留地址）"
                    :value="i2cRangesText(config.tools?.hardware?.i2c_deny_ranges)"
                    @change="(e: any) => saveI2cRanges('tools.hardware.i2c_deny_ranges', e)"></textarea>
                </div>
              </div>
            </div>
            <!-- exec deny patterns（批次 4 接线激活的死键）：开关 + 自定义词表
                 （替换语义）。装配期一次读——重启 gateway（Agent 重建）生效；
                 executor 分离形态下经 env 透传进子进程同源生效。 -->
            <div class="form-group" style="border-top: 1px dashed var(--border-light); padding-top: var(--space-3);">
              <label class="form-label">Exec 危险命令词表（tools.exec）</label>
              <div class="toggle" :class="{ active: config.tools?.exec?.enable_deny_patterns === true }"
                data-test="exec-deny-toggle"
                @click="toggleService('tools.exec.enable_deny_patterns', config.tools?.exec?.enable_deny_patterns === true)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">
                {{ config.tools?.exec?.enable_deny_patterns === true ? '自定义词表已启用' : '内置默认词表（42 条：sudo/rm -rf/git push/shutdown/管道外传…）' }}
              </span>
              <span class="form-hint" style="margin-left: var(--space-4); cursor: pointer; user-select: none;" data-test="deny-adv-toggle" @click="denyAdvOpen = !denyAdvOpen">
                自定义词表 {{ denyAdvOpen ? '▲' : '▼' }}
              </span>
              <div v-if="denyAdvOpen" style="margin-top: var(--space-2);">
                <textarea class="form-textarea" rows="6" data-test="exec-deny-patterns" style="min-width: 460px; font-family: var(--font-mono); font-size: var(--text-xs);"
                  placeholder="每行一条正则（RE2 语法，小写匹配）。注意：启用自定义词表 = 整体替换内置默认（非叠加）；全部条目无效时自动回落内置默认词表，永不全放行。&#10;示例：&#10;forbidden_tool\.exe&#10;drop\s+database"
                  :value="execDenyPatternsText" @change="saveExecDenyPatterns"></textarea>
                <div class="form-hint">
                  ⚠ <strong>重启 gateway（Agent 重建）后生效</strong>——词表在工具装配期编译一次；执行体分离模式下经环境变量透传进子进程，同源生效。命中词表的命令直接拒绝执行（审计照常）。
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Services toggles -->
        <div v-if="activeTab === 'services'" class="card">
          <div class="card-header"><h3>系统服务开关</h3></div>
          <div class="card-body">
            <div v-for="svc in [
              { label: 'Heartbeat', path: 'heartbeat.enabled', value: config.heartbeat?.enabled },
              { label: 'USB 监控', path: 'devices.monitor_usb', value: config.devices?.monitor_usb },
              { label: '设备管理（devices）', path: 'devices.enabled', value: config.devices?.enabled },
              { label: 'Security', path: 'security.enabled', value: config.security?.enabled },
              { label: 'Forge', path: 'forge.enabled', value: config.forge?.enabled },
              { label: 'MCP', path: 'mcp.enabled', value: config.mcp?.enabled },
            ]" :key="svc.path"
              style="display: flex; align-items: center; justify-content: space-between; padding: var(--space-3) 0; border-bottom: 1px solid var(--border-light);">
              <span style="font-size: var(--text-sm); font-weight: 500;">{{ svc.label }}</span>
              <div class="toggle" :class="{ active: svc.value !== false }" @click="toggleService(svc.path, svc.value !== false)"></div>
            </div>
            <!-- 批次 4：services 参数补全 -->
            <div class="form-group" style="margin-top: var(--space-4);">
              <label class="form-label">心跳间隔（heartbeat.interval，分钟）</label>
              <input class="form-input" type="number" min="5" style="max-width: 160px;" data-test="heartbeat-interval"
                :value="config.heartbeat?.interval ?? 30"
                @change="(e: any) => saveNum('heartbeat.interval', e, 30, 5)">
              <span class="form-hint">默认 30 分钟；&lt;5 分钟按 5 分钟下限钳制，0 = 回落缺省 30。</span>
            </div>
            <div class="form-group">
              <label class="form-label">PTY 终端并发上限（terminal.max_sessions）</label>
              <input class="form-input" type="number" min="1" max="32" style="max-width: 160px;"
                :value="config.terminal?.max_sessions ?? 4"
                @change="(e: any) => saveNum('terminal.max_sessions', e, 4, 1, 32)">
              <span class="form-hint">满员拒绝新连接（默认 4）；总开关在代码开发页 terminal 段（默认关）。改动重启生效。</span>
            </div>
            <div class="form-group">
              <span class="form-hint" style="cursor: pointer; user-select: none;" @click="termAdvOpen = !termAdvOpen">
                终端 shell 覆盖高级 {{ termAdvOpen ? '▲' : '▼' }}
              </span>
              <div v-if="termAdvOpen">
                <input class="form-input" style="max-width: 420px; font-family: var(--font-mono); font-size: var(--text-sm);"
                  :value="config.terminal?.shell || ''"
                  placeholder="留空 = 平台默认（Windows powershell / Unix $SHELL）"
                  @change="(e: any) => saveField('terminal.shell', String(e.target.value).trim() || null)">
                <div class="form-hint">PTY 会话启动的 shell 可执行文件路径/名称；重启生效。⚠ 交互 shell 无法逐命令审批——信任级 = dashboard 登录用户。</div>
              </div>
            </div>

            <!-- 批次 5：高级 JSON 只读折叠区（结构性低频键不铺表单——只展示
                 现值 + 手改指引；「原始 JSON」TAB 可整档编辑）。 -->
            <div class="form-group" style="margin-top: var(--space-4); border-top: 1px solid var(--border); padding-top: var(--space-3);">
              <span class="form-hint" style="cursor: pointer; user-select: none; font-size: var(--text-sm); font-weight: 500;"
                data-test="adv-json-toggle" @click="advJsonOpen = !advJsonOpen">
                高级 JSON 配置（只读展示）{{ advJsonOpen ? '▲' : '▼' }}
              </span>
              <div v-if="advJsonOpen" style="display: flex; flex-direction: column; gap: var(--space-3); margin-top: var(--space-2);" data-test="adv-json-blocks">
                <div v-for="b in advJsonBlocks" :key="b.title"
                  style="border: 1px solid var(--border-light); border-radius: var(--radius-md, 6px); padding: var(--space-2) var(--space-3);">
                  <div style="font-size: var(--text-sm); font-weight: 500;">{{ b.title }}</div>
                  <pre class="form-hint" style="margin: var(--space-1) 0; font-family: var(--font-mono); font-size: var(--text-xs); white-space: pre-wrap; word-break: break-all; max-height: 220px; overflow: auto;">{{ b.json }}</pre>
                  <div class="form-hint">{{ b.hint }} 手改：下方「原始 JSON」TAB 编辑保存，或直接改 config.json。</div>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Logging -->
        <div v-if="activeTab === 'logging'" class="card">
          <div class="card-header"><h3>日志配置</h3></div>
          <div class="card-body">
            <div class="form-group">
              <label class="form-label">通用日志</label>
              <div class="toggle" :class="{ active: config.logging?.general?.enabled !== false }"
                @click="toggleService('logging.general.enabled', config.logging?.general?.enabled !== false)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.logging?.general?.enabled !== false ? '已启用' : '已禁用' }}</span>
            </div>
            <div class="form-group">
              <label class="form-label">控制台输出</label>
              <div class="toggle" :class="{ active: config.logging?.general?.enable_console !== false }"
                @click="toggleService('logging.general.enable_console', config.logging?.general?.enable_console !== false)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.logging?.general?.enable_console !== false ? '已启用' : '已禁用' }}</span>
            </div>
            <div class="form-group">
              <label class="form-label">日志级别</label>
              <select class="form-select" style="max-width: 200px;"
                :value="config.logging?.general?.level || 'info'"
                @change="(e: any) => saveField('logging.general.level', e.target.value)">
                <option value="debug">DEBUG</option>
                <option value="info">INFO</option>
                <option value="warn">WARN</option>
                <option value="error">ERROR</option>
              </select>
            </div>
            <div class="form-group">
              <label class="form-label">LLM 通信日志</label>
              <div class="toggle" :class="{ active: config.logging?.llm?.enabled === true }"
                @click="toggleService('logging.llm.enabled', config.logging?.llm?.enabled === true)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.logging?.llm?.enabled === true ? '已启用' : '已禁用' }}</span>
            </div>
            <!-- 批次 4：logging 细节键补全 -->
            <div class="form-group">
              <label class="form-label">LLM 日志目录（logging.llm.log_dir）</label>
              <input class="form-input" :value="config.logging?.llm?.log_dir || ''"
                placeholder="留空 = 默认 logs/llm/" style="max-width: 360px;"
                @change="(e: any) => saveField('logging.llm.log_dir', String(e.target.value).trim())">
              <span class="form-hint">相对 workspace 或绝对路径；重启生效。</span>
            </div>
            <div class="form-group">
              <label class="form-label">LLM 日志详细度（logging.llm.detail_level）</label>
              <select class="form-select" style="max-width: 220px;"
                :value="config.logging?.llm?.detail_level || 'full'"
                @change="(e: any) => saveField('logging.llm.detail_level', e.target.value)">
                <option value="full">full（完整请求/响应原文）</option>
                <option value="truncated">truncated（参数与长文截断预览）</option>
              </select>
            </div>
            <div class="form-group">
              <label class="form-label">保存原始字节（logging.llm.save_raw）</label>
              <div class="toggle" :class="{ active: config.logging?.llm?.save_raw === true }"
                @click="toggleService('logging.llm.save_raw', config.logging?.llm?.save_raw === true)"></div>
              <span class="form-hint" style="margin-left: var(--space-2);">{{ config.logging?.llm?.save_raw === true ? '已启用（磁盘开销更大）' : '已禁用（默认）' }}</span>
            </div>
            <div class="form-group">
              <label class="form-label">通用日志文件（logging.general.file）</label>
              <input class="form-input" :value="config.logging?.general?.file || ''"
                placeholder="留空 = 只落默认位置" style="max-width: 360px;"
                @change="(e: any) => saveField('logging.general.file', String(e.target.value).trim())">
              <span class="form-hint">通用日志额外落盘文件路径；重启生效。</span>
            </div>
          </div>
        </div>

        <!-- 数据治理（2026-11-08 批次 2）：日志 16 族 TTL / checkpoint gc /
             磁盘水位 / 失败诊断捕获。清扫器启动扫一次 + 每 6h 一拍 →
             TTL 改动最迟 6h 生效（重启 gateway 立即触发一次清扫）。 -->
        <div v-if="activeTab === 'data'">
          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header"><h3>日志保留期（16 族 TTL）</h3></div>
            <div class="card-body">
              <div style="padding: var(--space-3); margin-bottom: var(--space-4); background: var(--bg-secondary); border: 1px solid var(--border-light); border-radius: var(--radius-md); font-size: var(--text-sm); color: var(--text-secondary);">
                清扫器 gateway 启动扫一次、之后每 6 小时一拍（每族每拍限量 200 件防 IO 尖峰）——<strong>改动最迟 6 小时后生效</strong>，重启 gateway 立即触发一次清扫。0 = 该族关闭清扫。清理按日志目录内条目的最后修改时间判定。
              </div>
              <div class="form-group">
                <label class="form-label">预设档（一次设全 16 族）</label>
                <select class="form-select" style="max-width: 260px;" :value="ttlPresetValue" data-test="ttl-preset"
                  @change="(e: any) => { if (e.target.value) applyTtlPreset(parseInt(e.target.value)) }">
                  <option value="" disabled>自定义（用下方高级逐族设置）</option>
                  <option value="7">保守（7 天）</option>
                  <option value="30">标准（30 天）</option>
                  <option value="90">放宽（90 天）</option>
                </select>
                <span class="form-hint">预设档统一设全部 16 族（含「检查点孤儿」——如需保持其短闸快清，可在高级里单独改回）。</span>
              </div>
              <div class="form-group">
                <label class="form-label" style="cursor: pointer; user-select: none;" data-test="data-adv-toggle" @click="dataAdvOpen = !dataAdvOpen">
                  高级：逐族设置 {{ dataAdvOpen ? '▲' : '▼' }}
                </label>
                <div v-if="dataAdvOpen" style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-2) var(--space-4); margin-top: var(--space-2);">
                  <div v-for="f in LOG_TTL_FAMILIES" :key="f.key" style="display: flex; align-items: center; gap: var(--space-2);">
                    <span style="flex: 1; font-size: var(--text-sm); color: var(--text-secondary);">{{ f.label }}<span v-if="f.hint" style="font-size: var(--text-xs); color: var(--text-muted);">（{{ f.hint }}）</span></span>
                    <input class="form-input" type="number" min="0" style="max-width: 90px;" data-test="ttl-family-input"
                      :value="ttlOf(f.key)" @change="(e: any) => saveTtlFamily(f.key, e)">
                    <span style="font-size: var(--text-xs); color: var(--text-muted);">天</span>
                  </div>
                </div>
              </div>
            </div>
          </div>

          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header"><h3>检查点清理（checkpoint.gc）</h3></div>
            <div class="card-body">
              <div class="form-group">
                <label class="form-label">启用清理</label>
                <div class="toggle" :class="{ active: gcField('enabled') }" data-test="gc-enable-toggle"
                  @click="toggleService('checkpoint.gc.enabled', gcField('enabled'))"></div>
                <span class="form-hint" style="margin-left: var(--space-2);">{{ gcField('enabled') ? '已启用' : '已禁用（默认）' }}</span>
                <div class="form-hint">默认关：清理删错 = 回退网失效，宁默认关、开关在用户手里。git 影子库走重建式 gc（自检后原子换名，不碰主仓对象库）。</div>
              </div>
              <div style="display: flex; gap: var(--space-4); flex-wrap: wrap;">
                <div class="form-group" :style="!gcField('enabled') ? 'opacity: 0.5;' : ''">
                  <label class="form-label">按轮数保留</label>
                  <input class="form-input" type="number" min="0" style="max-width: 140px;" :value="gcField('keep_turns')" data-test="gc-keep-turns"
                    :disabled="!gcField('enabled')"
                    @change="(e: any) => saveField('checkpoint.gc.keep_turns', parseInt(e.target.value) || 0)">
                  <span class="form-hint">最近 N 个 turn（0=关闭该维）</span>
                </div>
                <div class="form-group" :style="!gcField('enabled') ? 'opacity: 0.5;' : ''">
                  <label class="form-label">按天数保留</label>
                  <input class="form-input" type="number" min="0" style="max-width: 140px;" :value="gcField('keep_days')"
                    :disabled="!gcField('enabled')"
                    @change="(e: any) => saveField('checkpoint.gc.keep_days', parseInt(e.target.value) || 0)">
                  <span class="form-hint">最近 N 天（0=关闭该维）</span>
                </div>
              </div>
            </div>
          </div>

          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header"><h3>磁盘水位监测（health.disk_watermark）</h3></div>
            <div class="card-body">
              <div class="form-group">
                <label class="form-label">启用监测</label>
                <div class="toggle" :class="{ active: wmField('enabled') }"
                  @click="toggleService('health.disk_watermark.enabled', wmField('enabled'))"></div>
                <span class="form-hint" style="margin-left: var(--space-2);">{{ wmField('enabled') ? '已启用（默认）' : '已禁用' }}</span>
                <div class="form-hint">监测 workspace 所在卷剩余空间；critical 只升级告警（SSE system-alert + /ready 降级），不做自动急停。</div>
              </div>
              <div style="display: flex; gap: var(--space-4); flex-wrap: wrap;">
                <div class="form-group">
                  <label class="form-label">警告阈值（GB）</label>
                  <input class="form-input" type="number" min="0" style="max-width: 140px;" :value="wmField('warn_gb')" data-test="wm-warn-gb"
                    @change="(e: any) => saveField('health.disk_watermark.warn_gb', parseInt(e.target.value) || 0)">
                </div>
                <div class="form-group">
                  <label class="form-label">严重阈值（GB）</label>
                  <input class="form-input" type="number" min="0" style="max-width: 140px;" :value="wmField('critical_gb')"
                    @change="(e: any) => saveField('health.disk_watermark.critical_gb', parseInt(e.target.value) || 0)">
                  <span class="form-hint">低于此值 = 回退网/历史落盘即将失效</span>
                </div>
                <div class="form-group">
                  <label class="form-label">监测周期（秒）</label>
                  <input class="form-input" type="number" min="1" style="max-width: 140px;" :value="wmField('interval_secs')"
                    @change="(e: any) => saveField('health.disk_watermark.interval_secs', parseInt(e.target.value) || 0)">
                  <span class="form-hint">默认 60；0 视作缺省</span>
                </div>
              </div>
            </div>
          </div>

          <div class="card">
            <div class="card-header"><h3>失败诊断捕获（debug.capture）</h3></div>
            <div class="card-body">
              <div class="form-group">
                <label class="form-label">启用捕获</label>
                <div class="toggle" :class="{ active: config.debug?.capture?.enabled !== false }" data-test="capture-toggle"
                  @click="toggleService('debug.capture.enabled', config.debug?.capture?.enabled !== false)"></div>
                <span class="form-hint" style="margin-left: var(--space-2);">{{ config.debug?.capture?.enabled !== false ? '已启用（默认）' : '已禁用' }}</span>
                <div class="form-hint">只在失败信号（LLM 重试耗尽 / 上下文溢出 / 会话覆写 / agent 错误漏斗）时写盘到 logs/capture/——正常路径零磁盘开销。</div>
              </div>
            </div>
          </div>
        </div>

        <!-- CORS -->
        <div v-if="activeTab === 'cors'">
          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header">
              <h3>CORS 管理</h3>
              <div style="display: flex; align-items: center; gap: var(--space-2);">
                <span class="form-hint">开发模式（放行所有来源）</span>
                <div class="toggle" :class="{ active: corsEnabled }" @click="toggleCors(!corsEnabled)"></div>
              </div>
            </div>
            <div class="card-body">
              <div v-if="corsError" style="padding: var(--space-3); margin-bottom: var(--space-4); background: var(--warning-bg, #fef3cd); border: 1px solid var(--warning, #e5a00d); border-radius: var(--radius-md); font-size: var(--text-sm); color: var(--text-secondary);">
                CORS 配置未加载：{{ corsError }}。通常为 workspace/config/cors.json 缺席（gateway 启动期装配 manager，可用 CLI <code>nemesisbot cors add</code> 创建后重启）。
              </div>
              <div style="padding: var(--space-3); margin-bottom: var(--space-4); background: var(--accent-muted, #e8f0fe); border: 1px solid var(--border-light); border-radius: var(--radius-md); font-size: var(--text-sm); color: var(--text-secondary);">
                修改写入 workspace/config/cors.json；CORS 层在 gateway 启动时静态构建，<strong>改后需重启 gateway 生效</strong>（CLI <code>nemesisbot cors</code> 同语义）。开发模式仅限本机调试使用。
              </div>
              <div style="display: flex; gap: var(--space-2); margin-bottom: var(--space-4);">
                <input class="form-input" v-model="newOrigin" placeholder="例如: https://dash.example.com（需 http:// 或 https:// 前缀）" style="max-width: 400px;" @keyup.enter="addCorsOrigin">
                <button class="btn btn-primary" @click="addCorsOrigin">添加</button>
              </div>
              <div v-if="corsOrigins.length === 0" style="color: var(--text-muted); font-size: var(--text-sm);">暂无放行来源（localhost 默认放行，不受此列表影响）</div>
              <div v-for="origin in corsOrigins" :key="origin" style="display: flex; align-items: center; justify-content: space-between; padding: var(--space-2) var(--space-3); border: 1px solid var(--border-light); border-radius: var(--radius-md); margin-bottom: var(--space-2);">
                <code style="font-size: var(--text-sm);">{{ origin }}</code>
                <button class="btn btn-sm btn-danger" @click="removeCorsOrigin(origin)">移除</button>
              </div>
            </div>
          </div>
        </div>

        <!-- 编辑器：Full Access 放行开关（2026-09-20 用户裁决；
             与聊天框旁按钮同源，运行时态——Agent 重启后一律回关） -->
        <div v-if="activeTab === 'editor'" class="card">
          <div class="card-header"><h3>编辑器放行开关</h3></div>
          <div class="card-body">
            <div v-if="!editorAvailable" style="padding: var(--space-3); margin-bottom: var(--space-4); background: var(--warning-bg, #fef3cd); border: 1px solid var(--warning, #e5a00d); border-radius: var(--radius-md); font-size: var(--text-sm); color: var(--text-secondary);">
              编辑器放行未装配（本实例未启用 security 模块）——开关不可用。
            </div>
            <div class="form-group" style="display: flex; align-items: center; gap: var(--space-3);">
              <label class="form-label" style="margin: 0;">Full Access</label>
              <div class="toggle" :class="{ active: fullAccess }" @click="toggleEditorFull()"></div>
            </div>
            <div class="form-hint" style="margin-bottom: var(--space-4);">
              项目目录内文件操作全放行；项目目录外读 / 执行 / 网络 / 系统操作放行；项目目录外的写入与删除仍需审批；自杀形态硬拦不被绕过；执行类由真沙盒兜底（第九道防护）。
            </div>
            <div class="form-group" style="display: flex; align-items: center; gap: var(--space-3);">
              <label class="form-label" style="margin: 0;" :style="!fullAccess ? 'opacity: 0.4;' : ''">外部写删放行</label>
              <div class="toggle" :class="{ active: externalWrite }" :style="!fullAccess ? 'opacity: 0.4; pointer-events: none;' : ''" @click="toggleEditorExt()"></div>
            </div>
            <div class="form-hint">
              依赖 Full Access：项目目录外的写入 / 删除也放行。两个开关全开 = 真·全放（沙盒仍兜底）。
            </div>
            <!-- 会话级全部放行（2026-10-24 长任务无人值守计划）：作用域是
                 当前会话（持久 sidecar，重启自动恢复），与上面两个运行时
                 开关的「重启失效」刻意相反 -->
            <div class="form-group" style="display: flex; align-items: center; gap: var(--space-3); margin-top: var(--space-4);">
              <label class="form-label" style="margin: 0;">会话全部放行</label>
              <div
                class="toggle"
                :class="{ active: currentSessionAllow }"
                :style="!sessionAllowAvailable ? 'opacity: 0.4; pointer-events: none;' : ''"
                @click="toggleSessionAllow()"
              ></div>
            </div>
            <div class="form-hint">
              作用于<strong>当前会话</strong>{{ sessionStore.currentId ? `（${sessionStore.currentId}）` : '' }}：本会话所有安全审批自动放行（含 CRITICAL 与紫微 force）；硬拒绝与急停不受影响。无到期时间、Agent 重启后自动恢复——长任务跑完请手动关闭。<template v-if="currentSessionAllow">已开启{{ currentSessionSince ? `（${currentSessionSince} 起）` : '' }}，累计自动批准 {{ currentSessionAllowCount }} 条。</template>
            </div>
            <div style="padding: var(--space-3); margin-top: var(--space-4); background: var(--bg-secondary); border: 1px solid var(--border-light); border-radius: var(--radius-md); font-size: var(--text-sm); color: var(--text-secondary); line-height: 1.8;">
              <strong>注意</strong>：两个开关都是<strong>运行时态</strong>——Agent 进程重启后一律回到关闭，必须手动重新打开（每次重启重新授权）。开启后配置里的 deny / ask 规则与审批弹窗将被放行越过（审计照常记录，policy_rule=<code>editor_access:*</code> 可过滤）；guardian 二审（LLM 命令审计）不受影响仍可弹卡。<strong>「会话全部放行」例外</strong>：它随会话持久化，重启自动恢复。
            </div>
          </div>
        </div>

        <!-- 配色（2026-10-29）：亮暗模式切换从侧边栏迁入；后续 UI 配色
             自定义（主题色/强调色等）都在此 TAB 扩展。setTheme 即时生效，
             localStorage 持久化刷新保持。 -->
        <div v-if="activeTab === 'appearance'" class="card">
          <div class="card-header">
            <h3>配色</h3>
          </div>
          <p class="theme-hint">Dashboard 界面亮暗模式，选择即时生效并自动记住。后续 UI 配色自定义都在这里扩展。</p>
          <div class="theme-cards">
            <div class="theme-card" :class="{ active: theme === 'light' }" @click="setTheme('light')">
              <span class="theme-card-icon">☀️</span>
              <span class="theme-card-name">浅色模式</span>
              <span v-if="theme === 'light'" class="theme-card-check">✓ 当前</span>
            </div>
            <div class="theme-card" :class="{ active: theme === 'dark' }" @click="setTheme('dark')">
              <span class="theme-card-icon">🌙</span>
              <span class="theme-card-name">深色模式</span>
              <span v-if="theme === 'dark'" class="theme-card-check">✓ 当前</span>
            </div>
          </div>
        </div>

        <!-- 皮肤（skins feature 门控；异步组件，关 = 死分支被 tree-shake） -->
        <component :is="SkinsPanel" v-if="activeTab === 'skins' && SkinsPanel" />

        <!-- Raw JSON -->
        <div v-if="activeTab === 'raw'">
          <div class="card">
            <div class="card-header">
              <h3>原始配置 (config.json)</h3>
              <div style="display: flex; gap: var(--space-2);">
                <template v-if="!editing">
                  <button class="btn btn-sm" @click="editing = true; editConfig = JSON.stringify(config, null, 2)">编辑</button>
                </template>
                <template v-else>
                  <button class="btn btn-sm" @click="editing = false">取消</button>
                  <button class="btn btn-sm btn-primary" @click="saveFullConfig">保存</button>
                </template>
              </div>
            </div>
            <div class="card-body">
              <div v-if="editing">
                <div style="padding: var(--space-3); margin-bottom: var(--space-3); background: var(--warning-bg, #fef3cd); border: 1px solid var(--warning, #e5a00d); border-radius: var(--radius-md); font-size: var(--text-sm); color: var(--text-secondary);">
                  注意：敏感字段（如 API Key、Token）已被遮蔽显示（含 **** ）。如需修改，请将遮蔽值替换为真实值；如保持遮蔽值不变，保存后该字段将被覆盖为遮蔽值。
                </div>
                <textarea class="form-textarea" style="min-height: 60vh; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="editConfig"></textarea>
              </div>
              <div v-else>
                <div class="settings-section" v-for="(sectionData, section) in config" :key="section">
                  <h3>{{ section }}</h3>
                  <div class="settings-grid">
                    <template v-for="(value, key) in (sectionData as any)" :key="key">
                      <template v-if="typeof value !== 'object'">
                        <div class="settings-key">{{ key }}</div>
                        <div class="settings-value">{{ String(value) }}</div>
                      </template>
                    </template>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* WS9/P22：租约状态行（Gateway 页签） */
.lease-line {
  display: flex;
  align-items: center;
  gap: var(--space-2, 8px);
  font-size: var(--text-sm, 13px);
}
.lease-held {
  color: var(--warning, #e5a00d);
}
.lease-free {
  color: var(--text-secondary, #888);
}
.lease-refresh {
  padding: 2px 10px;
  font-size: 12px;
  border: 1px solid var(--border, #ddd);
  border-radius: 4px;
  background: transparent;
  color: var(--text-secondary, #888);
  cursor: pointer;
}
.lease-refresh:hover {
  color: var(--text);
  background: var(--bg-primary, #f5f5f5);
}

/* 配色 TAB（2026-10-29）：亮暗模式双卡片选择器 */
.theme-hint {
  margin: 0 0 var(--space-3, 12px);
  font-size: var(--text-sm, 13px);
  color: var(--text-secondary, #888);
}
.theme-cards {
  display: flex;
  gap: var(--space-3, 12px);
}
.theme-card {
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--space-2, 8px);
  min-width: 140px;
  padding: var(--space-4, 16px) var(--space-3, 12px);
  border: 1px solid var(--border, #ddd);
  border-radius: 8px;
  background: var(--bg-primary, transparent);
  cursor: pointer;
  transition: border-color 0.15s, box-shadow 0.15s;
}
.theme-card:hover {
  border-color: var(--accent, #4a9eff);
}
.theme-card.active {
  border-color: var(--accent, #4a9eff);
  box-shadow: 0 0 0 1px var(--accent, #4a9eff);
}
.theme-card-icon {
  font-size: 24px;
  line-height: 1;
}
.theme-card-name {
  font-size: var(--text-sm, 13px);
  color: var(--text, inherit);
}
.theme-card-check {
  position: absolute;
  top: 6px;
  right: 8px;
  font-size: 12px;
  color: var(--accent, #4a9eff);
}
</style>
