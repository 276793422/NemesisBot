<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import SecurityLayerHeader from '../components/security/SecurityLayerHeader.vue'

// 十层页面化（2026-11-10）③：六层配置驱动通用模板（用户拍板 1——不手写
// 十页）。覆盖 ①注入检测 ②命令守卫 ④凭据扫描 ⑤DLP ⑥SSRF ⑧审计链；
// ③ABAC 规则编辑器最重走专页（SecurityAbacView），⑦⑨⑩ 独立子系统页
// 归组（ScannerView/SandboxView/VmView）。
//
// 数据双源：
// - `security.overview`（raw 语义投影）→ 状态徽标/开关现状（层开关缺席 =
//   启用，typed serde default false 与 runtime 相反——展示一律信 overview）；
// - `security.config.get`（typed 全档）→ 写回载体（改键后 security.config.save
//   整档保存，与旧 SecurityView 同模式）。
//
// 层开关/层参数全部「重启网关生效」（apply_security_layer_switches 构造期
// 消费，reload 假姿态——overview.effective="restart" 提示）；② 的
// exec_unknown_policy 与 ⑧ 的审计开关同为构造期键，生效时机一致。

const props = defineProps<{ layer: string }>()

const { request } = useWSAPI()
const toast = useToast()

interface LayerDef {
  num: string
  title: string
  duty: string
  /** 本层审计 policy_rule 前缀（拦截记录过滤；空 = 展示全部） */
  policies: string[]
  /** security.overview.layers 投影键 */
  overviewKey: string
}

const LAYER_DEFS: Record<string, LayerDef> = {
  injection: {
    num: '①',
    title: '注入检测',
    duty: '工具入参中的提示词注入形态评分（0~1），超阈值判定注入并拦截；阈值越高越宽松。',
    policies: ['injection_detector'],
    overviewKey: 'injection',
  },
  command: {
    num: '②',
    title: '命令守卫',
    duty: 'exec/spawn 危险命令内置黑名单硬拦（rm -rf、format、shutdown 等）；未命中规则的未知命令按下方策略处置。',
    policies: ['command_guard'],
    overviewKey: 'command_guard',
  },
  credential: {
    num: '④',
    title: '凭据扫描',
    duty: '工具入参中的明文凭据检测（内置 32 条规则：AWS/GitHub/Slack/私钥/JWT/数据库连接串等），命中按动作脱敏或拦截。',
    policies: ['credential_scanner'],
    overviewKey: 'credential',
  },
  dlp: {
    num: '⑤',
    title: 'DLP 防泄漏',
    duty: '敏感数据出站检测（32 条规则：信用卡/PII/金融/网络/凭据五类），高置信按动作处置，低置信与入站可单独配置。',
    policies: ['dlp_engine'],
    overviewKey: 'dlp',
  },
  ssrf: {
    num: '⑥',
    title: 'SSRF 防护',
    duty: 'URL 目标内网/环回/链路本地地址拦截（SSRF 供给面收口），白名单域名显式放行。',
    policies: ['ssrf_guard'],
    overviewKey: 'ssrf',
  },
  audit: {
    num: '⑧',
    title: '审计链',
    duty: '全部安全事件 JSONL 审计日志 + Merkle append-only 防篡改链（链校验可发现历史改写）。',
    policies: [],
    overviewKey: 'audit',
  },
}

const def = computed(() => LAYER_DEFS[props.layer] ?? null)

// ── 状态 ─────────────────────────────────────────────────────────────
const loading = ref(true)
const overview = ref<any>(null)
const config = ref<any>({})
const entries = ref<any[]>([])
const saving = ref(false)
const savingKey = ref('')
const chainVerifyResult = ref<any>(null)
const chainVerifying = ref(false)

// ② 黑名单 / ④ 凭据规则只读快照
const blocklist = ref<any>(null)
const credRules = ref<any>(null)

const layerProj = computed(() => overview.value?.layers?.[def.value?.overviewKey] ?? null)
// ⑧ 审计无单键 enabled（真开关是顶层 audit_log_file_enabled / audit_chain_enabled
// 两键，见下方 ⑧ 专属段）；状态口径与总览页同源：日志文件开 = 层在岗。
const layerEnabled = computed<boolean>(() =>
  props.layer === 'audit'
    ? !!layerProj.value?.log_file_enabled
    : !!layerProj.value?.enabled,
)

// DLP 动作选项（有效值 block/log——dlp.rs effective_action 二值）
const DLP_ACTIONS = [
  { value: 'block', label: 'block · 拦截' },
  { value: 'log', label: 'log · 只记审计' },
]

// ② 未知命令姿态（与旧 SecurityView POLICY_CHOICES 同源语义）
const execPolicy = computed(() => config.value.exec_unknown_policy || 'allow')
const EXEC_CHOICES = [
  { value: 'allow', label: 'allow · 放行（默认）' },
  { value: 'ask', label: 'ask · 送审批卡' },
  { value: 'deny', label: 'deny · 硬拦' },
]

// ① 阈值（overview 投影带 configured 标记；编辑值取 config typed extra）
const thresholdVal = computed<number | null>(() => {
  const t = config.value?.layers?.injection?.extra?.threshold
  return typeof t === 'number' && t >= 0 && t <= 1 ? t : null
})

const auditStats = computed(() => overview.value?.intercepts ?? { total: 0, denied: 0, by_policy: {} })
const chain = computed(() => overview.value?.chain ?? { present: false, segments: 0 })

// ── 数据装载 ─────────────────────────────────────────────────────────
async function loadConfig() {
  try {
    config.value = (await request('security', 'config.get')) || {}
  } catch { /* ignore */ }
}

async function loadOverview() {
  try {
    overview.value = (await request('security', 'overview')) || {}
  } catch { /* ignore */ }
}

async function loadAudit() {
  try {
    const data = await request('security', 'audit', { limit: 100 })
    const all: any[] = data?.entries || []
    const pols = def.value?.policies ?? []
    entries.value = pols.length ? all.filter((e) => pols.includes(e.policy)) : all
  } catch { /* ignore */ }
}

async function loadStaticSnapshots() {
  if (props.layer === 'command') {
    try {
      blocklist.value = (await request('security', 'command_blocklist')) || {}
    } catch { /* ignore */ }
  }
  if (props.layer === 'credential') {
    try {
      credRules.value = (await request('security', 'credential_rules')) || {}
    } catch { /* ignore */ }
  }
}

// ── 写回（typed 全档保存；失败回滚由重新拉取兜底）──────────────────────
/** 确保 config.layers[name] 对象存在并返回（typed 载体键写入口）。 */
function layerObj(name: string): any {
  const c = config.value
  if (!c.layers || typeof c.layers !== 'object') c.layers = {}
  if (!c.layers[name] || typeof c.layers[name] !== 'object') c.layers[name] = {}
  return c.layers[name]
}

async function persist(key: string) {
  saving.value = true
  savingKey.value = key
  try {
    await request('security', 'config.save', config.value)
    toast.success('已保存（重启网关生效）')
    await Promise.all([loadOverview(), loadConfig()])
  } catch (e: any) {
    toast.error('保存失败: ' + e)
    await loadConfig()
  } finally {
    saving.value = false
    savingKey.value = ''
  }
}

async function toggleLayer(ev: Event) {
  if (!def.value) return
  const on = (ev.target as HTMLInputElement).checked
  layerObj(def.value.overviewKey).enabled = on
  await persist('layer-toggle')
}

async function saveThreshold(ev: Event) {
  const raw = (ev.target as HTMLInputElement).value
  const t = Number(raw)
  const extra = layerObj('injection')
  if (!extra.extra || typeof extra.extra !== 'object') extra.extra = {}
  if (Number.isFinite(t) && t >= 0 && t <= 1) {
    extra.extra.threshold = t
  } else {
    delete extra.extra.threshold
    toast.error('阈值须在 0~1，已恢复默认 0.7')
  }
  await persist('threshold')
}

async function saveExecPolicy(ev: Event) {
  config.value.exec_unknown_policy = (ev.target as HTMLSelectElement).value
  await persist('exec_unknown_policy')
}

function dlpArr(key: string): string[] {
  const d = layerObj('dlp')
  if (!Array.isArray(d[key])) d[key] = []
  return d[key]
}

async function saveDlpAction(key: 'action' | 'low_confidence_action' | 'inbound_action', ev: Event) {
  layerObj('dlp')[key] = (ev.target as HTMLSelectElement).value
  await persist('dlp-' + key)
}

const newDlpRule = ref('')
function addDlpRule() {
  const name = newDlpRule.value.trim()
  if (!name) return
  const arr = dlpArr('rules')
  if (!arr.includes(name)) arr.push(name)
  newDlpRule.value = ''
  persist('dlp-rules')
}
function removeDlpRule(name: string) {
  const arr = dlpArr('rules')
  const i = arr.indexOf(name)
  if (i >= 0) arr.splice(i, 1)
  persist('dlp-rules')
}

function ssrfHosts(): string[] {
  const s = layerObj('ssrf')
  if (!Array.isArray(s.allowed_hosts)) s.allowed_hosts = []
  return s.allowed_hosts
}
const newHost = ref('')
function addHost() {
  const h = newHost.value.trim()
  if (!h) return
  const arr = ssrfHosts()
  if (!arr.includes(h)) arr.push(h)
  newHost.value = ''
  persist('ssrf-hosts')
}
function removeHost(h: string) {
  const arr = ssrfHosts()
  const i = arr.indexOf(h)
  if (i >= 0) arr.splice(i, 1)
  persist('ssrf-hosts')
}

function topFlag(key: string): boolean {
  const v = config.value?.[key]
  return typeof v === 'boolean' ? v : key === 'audit_log_file_enabled'
}
async function toggleAuditFlag(key: 'audit_log_file_enabled' | 'audit_chain_enabled', ev: Event) {
  config.value[key] = (ev.target as HTMLInputElement).checked
  await persist(key)
}

async function verifyChain() {
  chainVerifying.value = true
  chainVerifyResult.value = null
  try {
    chainVerifyResult.value = await request('logs', 'chain_verify')
    if (chainVerifyResult.value?.valid === false) toast.error('审计链校验失败：存在改写')
  } catch (e: any) {
    toast.error('校验失败: ' + e)
  } finally {
    chainVerifying.value = false
  }
}

// ④ 凭据规则类别（前端按名映射——Rust 侧无类别元数据）
function credCategory(name: string): string {
  if (/^(aws|gcp|azure|heroku|netlify)_/.test(name) || name === 'azure_connection_string') return '云厂商'
  if (/^(github|discord|npm)_/.test(name)) return '代码/平台'
  if (/^(slack|stripe|sendgrid|twilio|mailgun)_/.test(name)) return 'IM/服务'
  if (/_private_key$|^private_key$|^pgp_private_key$/.test(name)) return '私钥'
  if (/^(jwt_token|bearer_token|basic_auth|api_key_in_url)$/.test(name)) return '令牌'
  if (/^db_connection_/.test(name)) return '数据库'
  return '通用'
}

function formatDate(ts?: string): string {
  if (!ts) return '--'
  const d = new Date(ts)
  return Number.isNaN(d.getTime()) ? ts : d.toLocaleString('zh-CN')
}

onMounted(async () => {
  await Promise.all([loadOverview(), loadConfig(), loadAudit(), loadStaticSnapshots()])
  loading.value = false
})
</script>

<template>
  <div class="page-security-layer">
    <template v-if="def">
      <SecurityLayerHeader
        :num="def.num"
        :title="def.title"
        :duty="def.duty"
        :status-label="layerEnabled ? '已启用' : '已关闭'"
        :status-tone="layerEnabled ? 'on' : 'off'"
        effective="restart"
      />
    </template>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading && !def" class="empty-state">
        <h3>未知层</h3>
        <p>{{ layer }}</p>
      </div>

      <div v-if="!loading && def">
        <!-- 层开关 + 层参数（⑧ 审计无通用层开关——runtime 只读顶层两键，
             通用开关写 config.layers.audit.enabled 是幽灵键；真开关在 ⑧ 专属段） -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>层配置</h3></div>
          <div class="card-body">
            <label v-if="layer !== 'audit'" class="layer-toggle-row" data-test="layer-toggle-row">
              <input
                type="checkbox"
                data-test="layer-toggle"
                :checked="layerEnabled"
                :disabled="saving"
                @change="toggleLayer($event)"
              />
              <span>
                <strong>启用 {{ def.num }} {{ def.title }}</strong>
                <div class="hint">关闭后本层引擎不再装配（重启网关生效）。安全总闸（设置页 security.enabled）仍在更上层兜底。</div>
              </span>
            </label>

            <!-- ① 阈值 -->
            <div v-if="layer === 'injection'" class="layer-param" data-test="injection-threshold-block">
              <span class="param-key">判定阈值</span>
              <input
                type="number"
                class="form-input"
                style="width: 120px;"
                min="0"
                max="1"
                step="0.05"
                :value="thresholdVal ?? 0.7"
                :disabled="saving"
                data-test="injection-threshold"
                @change="saveThreshold($event)"
              />
              <div class="hint">
                当前生效 {{ layerProj?.threshold ?? 0.7 }}{{ layerProj?.threshold_configured ? '（已配置）' : '（默认 0.7，未配置或越界自动回落）' }}。越界值保存时被拒并回落默认。
              </div>
            </div>

            <!-- ② 未知命令姿态 -->
            <div v-if="layer === 'command'" class="layer-param" data-test="exec-policy-block">
              <span class="param-key">未知命令姿态</span>
              <select
                class="form-select"
                style="max-width: 260px;"
                data-test="exec-unknown-policy"
                :value="execPolicy"
                :disabled="saving"
                @change="saveExecPolicy($event)"
              >
                <option v-for="c in EXEC_CHOICES" :key="c.value" :value="c.value">{{ c.label }}</option>
              </select>
              <div class="hint">exec/spawn 未命中任何规则（含审批记忆）时的姿态。allow = 放行（旧行为）；ask = 弹审批卡；deny = 硬拦。</div>
            </div>

            <!-- ⑤ DLP 四键 -->
            <template v-if="layer === 'dlp'">
              <div class="layer-param">
                <span class="param-key">处置动作</span>
                <select class="form-select" style="max-width: 220px;" data-test="dlp-action" :value="config.layers?.dlp?.action || 'block'" :disabled="saving" @change="saveDlpAction('action', $event)">
                  <option v-for="a in DLP_ACTIONS" :key="a.value" :value="a.value">{{ a.label }}</option>
                </select>
                <div class="hint">高置信命中的处置，默认 block。</div>
              </div>
              <div class="layer-param">
                <span class="param-key">低置信动作</span>
                <select class="form-select" style="max-width: 220px;" data-test="dlp-low-action" :value="config.layers?.dlp?.low_confidence_action || 'log'" :disabled="saving" @change="saveDlpAction('low_confidence_action', $event)">
                  <option v-for="a in DLP_ACTIONS" :key="a.value" :value="a.value">{{ a.label }}</option>
                </select>
                <div class="hint">低置信命中（误报高发段）默认 log（只记审计不拦）。</div>
              </div>
              <div class="layer-param">
                <span class="param-key">入站动作</span>
                <select class="form-select" style="max-width: 220px;" data-test="dlp-inbound-action" :value="config.layers?.dlp?.inbound_action || 'log'" :disabled="saving" @change="saveDlpAction('inbound_action', $event)">
                  <option v-for="a in DLP_ACTIONS" :key="a.value" :value="a.value">{{ a.label }}</option>
                </select>
                <div class="hint">入站/本地存储操作的 DLP 处置，默认 log。</div>
              </div>
              <div class="layer-param" data-test="dlp-rules-block">
                <span class="param-key">规则过滤</span>
                <div class="chip-editor">
                  <span v-for="r in overview?.layers?.dlp?.rules || []" :key="r" class="chip">
                    {{ r }}
                    <button class="chip-x" :disabled="saving" @click="removeDlpRule(r)">×</button>
                  </span>
                  <input v-model="newDlpRule" placeholder="规则名（如 email、credit_card）回车添加" style="flex: 1; min-width: 200px;" @keyup.enter="addDlpRule" />
                </div>
                <div class="hint">空 = 全部 32 条规则启用；填入规则名可只启用子集（按规则名过滤）。</div>
              </div>
            </template>

            <!-- ⑥ 白名单 -->
            <div v-if="layer === 'ssrf'" class="layer-param" data-test="ssrf-hosts-block">
              <span class="param-key">白名单域名</span>
              <div class="chip-editor">
                <span v-for="h in overview?.layers?.ssrf?.allowed_hosts || []" :key="h" class="chip">
                  {{ h }}
                  <button class="chip-x" :disabled="saving" @click="removeHost(h)">×</button>
                </span>
                <input v-model="newHost" placeholder="域名（如 127.0.0.1、localhost）回车添加" style="flex: 1; min-width: 200px;" @keyup.enter="addHost" />
              </div>
              <div class="hint">SSRF 拦截显式放行的目标（内网联调/本机服务）。名单之外的内网/环回目标一律拦。</div>
            </div>

            <!-- ⑧ 双开关 + 链校验 -->
            <template v-if="layer === 'audit'">
              <label class="layer-toggle-row" data-test="audit-log-toggle-row">
                <input type="checkbox" data-test="audit-log-toggle" :checked="topFlag('audit_log_file_enabled')" :disabled="saving" @change="toggleAuditFlag('audit_log_file_enabled', $event)" />
                <span>
                  <strong>审计日志文件</strong>
                  <div class="hint">安全事件写入 workspace/logs/security_logs/*.jsonl（默认开）。</div>
                </span>
              </label>
              <label class="layer-toggle-row" data-test="audit-chain-toggle-row">
                <input type="checkbox" data-test="audit-chain-toggle" :checked="topFlag('audit_chain_enabled')" :disabled="saving" @change="toggleAuditFlag('audit_chain_enabled', $event)" />
                <span>
                  <strong>Merkle 防篡改链</strong>
                  <div class="hint">审计事件追加进 append-only 哈希链（默认关），链校验可发现历史改写。</div>
                </span>
              </label>
              <div class="layer-param" data-test="chain-status-block">
                <span class="param-key">链状态</span>
                <span class="chain-state">
                  {{ chain.present ? `在场 · ${chain.segments} 段` : '不在场（未启用或无审计记录）' }}
                </span>
                <button class="btn btn-sm" data-test="chain-verify" :disabled="!chain.present || chainVerifying" @click="verifyChain">
                  {{ chainVerifying ? '校验中…' : '校验链完整性' }}
                </button>
                <div v-if="chainVerifyResult" class="hint" data-test="chain-verify-result">
                  校验结果：{{ chainVerifyResult.valid ? `✅ 通过（${chainVerifyResult.total_segments ?? chain.segments} 段未被改写）` : `❌ 失败：首破段 #${chainVerifyResult.first_broken_index ?? '?'} · 破坏 ${chainVerifyResult.broken_count ?? '?'} 段` }}
                </div>
              </div>
              <div class="stats-grid" style="margin-top: var(--space-2);">
                <div class="stat-card">
                  <div class="stat-label">审计事件总数</div>
                  <div class="stat-value">{{ auditStats.total || 0 }}</div>
                </div>
                <div class="stat-card">
                  <div class="stat-label">拦截/拒绝</div>
                  <div class="stat-value" style="color: var(--error);">{{ auditStats.denied || 0 }}</div>
                </div>
                <div class="stat-card">
                  <div class="stat-label">涉及 policy 桶</div>
                  <div class="stat-value">{{ Object.keys(auditStats.by_policy || {}).length }}</div>
                </div>
              </div>
            </template>
          </div>
        </div>

        <!-- ② 黑名单只读清单 -->
        <div v-if="layer === 'command' && blocklist" class="card" style="margin-bottom: var(--space-4);" data-test="blocklist-card">
          <div class="card-header"><h3>内置命令黑名单</h3></div>
          <div class="card-body">
            <div class="hint" style="margin: 0 0 var(--space-2);">
              {{ blocklist.note }}。当前 {{ blocklist.entries?.length || 0 }} 条。
            </div>
            <div class="table-wrap">
              <table data-test="blocklist-table">
                <thead><tr><th>命令</th><th>类别</th><th>严重度</th><th>平台</th><th>原因</th></tr></thead>
                <tbody>
                  <tr v-for="(e, i) in blocklist.entries || []" :key="i">
                    <td class="mono">{{ e.name }}</td>
                    <td>{{ e.category }}</td>
                    <td>
                      <span class="badge" :class="{ 'badge-error': e.severity === 'Critical', 'badge-warning': e.severity === 'High', 'badge-info': e.severity === 'Medium', 'badge-neutral': e.severity === 'Low' }">{{ e.severity }}</span>
                    </td>
                    <td style="font-size: var(--text-xs);">{{ e.platform }}</td>
                    <td style="font-size: var(--text-xs); color: var(--text-muted, #888);">{{ e.reason }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- ④ 凭据规则只读清单 -->
        <div v-if="layer === 'credential' && credRules" class="card" style="margin-bottom: var(--space-4);" data-test="cred-rules-card">
          <div class="card-header"><h3>内置凭据规则</h3></div>
          <div class="card-body">
            <div class="hint" style="margin: 0 0 var(--space-2);">
              {{ credRules.note }}。当前 {{ credRules.rules?.length || 0 }} 条。vault 引用（<code>vault:别名</code> / <code>{{ '\{\{vault:别名\}\}' }}</code>）自动豁免——引用原文不是明文秘密。
            </div>
            <div class="table-wrap">
              <table data-test="cred-rules-table">
                <thead><tr><th>规则名</th><th>类别</th></tr></thead>
                <tbody>
                  <tr v-for="r in credRules.rules || []" :key="r.name">
                    <td class="mono">{{ r.name }}</td>
                    <td>{{ credCategory(r.name) }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- 拦截记录（audit 页 = 全量审计记录） -->
        <div class="card">
          <div class="card-header"><h3>{{ layer === 'audit' ? '审计记录' : '拦截记录' }}</h3></div>
          <div class="card-body">
            <div v-if="entries.length === 0" class="empty-state" style="padding: var(--space-4);">
              <p style="margin: 0;">暂无记录</p>
            </div>
            <div v-else class="table-wrap">
              <table data-test="intercept-table">
                <thead><tr><th>时间</th><th>操作</th><th>风险级别</th><th>目标</th><th>policy</th><th>结果</th><th>原因</th></tr></thead>
                <tbody>
                  <tr v-for="(e, idx) in entries" :key="idx">
                    <td style="font-size: var(--text-xs);">{{ formatDate(e.timestamp) }}</td>
                    <td>{{ e.operation || '--' }}</td>
                    <td>
                      <span class="badge" :class="{ 'badge-error': e.risk_level === 'CRITICAL', 'badge-warning': e.risk_level === 'HIGH', 'badge-info': e.risk_level === 'MEDIUM', 'badge-neutral': e.risk_level === 'LOW' }">{{ e.risk_level || '--' }}</span>
                    </td>
                    <td style="max-width: 200px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">{{ e.target || '--' }}</td>
                    <td class="mono" style="font-size: var(--text-xs);">{{ e.policy || '--' }}</td>
                    <td>
                      <span :style="{ color: e.result === 'allow' ? 'var(--success, #2da44e)' : 'var(--error)' }">{{ e.result }}</span>
                    </td>
                    <td style="max-width: 220px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--text-xs); color: var(--text-muted, #888);">{{ e.reason || '--' }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
            <div class="hint" style="margin-top: var(--space-2);">显示最近 {{ entries.length }} 条（{{ def.policies.length ? '按本层 policy 过滤' : '全部' }}）。</div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.hint {
  font-size: var(--text-xs);
  color: var(--text-muted, #888);
  margin-top: var(--space-1);
  max-width: 640px;
}
.mono {
  font-family: var(--font-mono, 'Consolas', monospace);
  font-size: var(--text-xs);
}
.layer-toggle-row {
  display: flex;
  align-items: flex-start;
  gap: var(--space-2);
  cursor: pointer;
  user-select: none;
  margin-bottom: var(--space-2);
}
.layer-toggle-row input[type='checkbox'] {
  margin-top: 3px;
}
.layer-toggle-row .hint {
  font-weight: 400;
}
.layer-param {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  flex-wrap: wrap;
  margin: var(--space-2) 0;
}
.param-key {
  font-size: var(--text-sm);
  min-width: 96px;
}
.chip-editor {
  display: flex;
  gap: var(--space-1);
  flex-wrap: wrap;
  align-items: center;
  flex: 1;
}
.chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: var(--text-xs);
  font-family: var(--font-mono, 'Consolas', monospace);
  padding: 2px 6px 2px 10px;
  border-radius: 999px;
  background: var(--bg-elev, rgba(128, 128, 128, 0.12));
  border: 1px solid var(--border, rgba(128, 128, 128, 0.3));
}
.chip-x {
  border: none;
  background: none;
  color: var(--text-muted, #888);
  cursor: pointer;
  font-size: var(--text-sm);
  padding: 0 2px;
}
.chip-x:hover {
  color: var(--error);
}
.chain-state {
  font-size: var(--text-sm);
}
</style>
