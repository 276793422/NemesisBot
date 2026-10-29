<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { groupModelsByFamily } from '../utils/providerFamilies'

const { request } = useWSAPI()
const toast = useToast()

// Backend returns: model_name, model, api_base, api_key (masked), proxy, is_default
// + P3-2 raw extras (null = 未设置) + catalog_match (exact-key hit, nullable)
interface CatalogMatch {
  context_window?: number
  max_output_tokens?: number
  family?: string
}
interface Model {
  model_name: string
  model: string
  api_base?: string
  api_key?: string
  proxy?: string
  is_default?: boolean
  protocol?: string | null
  model_tier?: string | null
  reasoning_effort?: string | null
  // per-model 流式开关（2026-10-13 stream 键；null/缺省 = auto）。
  stream?: string | null
  model_size_b?: number | string | null
  real_name?: string | null
  context_window?: number | string | null
  // per-model 绝对压缩点（2026-10-20；null/缺省 = 窗口×75% 默认链）。
  compact_context_tokens?: number | string | null
  catalog_match?: CatalogMatch | null
  // G4 (U15)：key 来源分类（后端 classify_key_source，零明文）。
  key_source?: { kind: string; ref: string } | null
}

const models = ref<Model[]>([])
const loading = ref(true)
const showAdd = ref(false)
// Backend add expects: name, model, key, base_url?, proxy?, protocol?, stream?
// protocol = LLM 协议选择器：新建条目默认 Claude（anthropic）。
const addForm = ref({ name: '', model: '', key: '', base_url: '', proxy: '', protocol: 'anthropic', stream: '' })

// 协议类型下拉选项（值集与后端 normalize_model_protocol 一致）。
const PROTOCOL_OPTIONS = [
  { value: 'anthropic', label: 'Claude（Anthropic 消息协议）' },
  { value: 'openai', label: 'OpenAI 兼容（chat/completions）' },
  { value: 'responses', label: 'OpenAI Responses' },
  // 职能框架 M4/M6：图像生成 lane——仅供 generate_image 工具消费，
  // 后端四处守卫（CLI add --default / probe / set_default / factory）拒绝其作对话模型。
  { value: 'images-openai', label: '图像生成（OpenAI images）·仅供 generate_image 工具' },
]
// 流式开关下拉选项（值集与后端 normalize_stream_value 一致；'' = auto）。
const STREAM_OPTIONS = [
  { value: '', label: 'auto（跟随 lane 默认）' },
  { value: 'yes', label: '强制流式' },
  { value: 'no', label: '强制非流式' },
]
// 图像协议条目判定（卡片徽标 + 设为默认禁用）。
function isImageModel(m: { protocol?: string | null }): boolean {
  return m.protocol === 'images-openai'
}
const switching = ref<string | null>(null)

// ── 五项增强批（2026-10-27）：①真测试 ②复制 ③改名 ④列表模式 ⑤一键测试+圆点 ──

/** ④ 视图模式（localStorage 持久，纯 UI 偏好）。 */
type ViewMode = 'card' | 'list'
const viewMode = ref<ViewMode>(localStorage.getItem('models.viewMode') === 'list' ? 'list' : 'card')
function setViewMode(v: ViewMode) {
  viewMode.value = v
  localStorage.setItem('models.viewMode', v)
}

/** ⑤ 圆点状态灯（组件内存态，不落盘——重启自然回灰）。
 * idle 灰 → testing 黄 → ok 绿 / fail 红；unsupported（图像 lane）保持灰 + tooltip。 */
interface DotState { status: 'idle' | 'testing' | 'ok' | 'fail' | 'unsupported'; detail?: string; kind?: string; latency_ms?: number }
const dotState = ref<Record<string, DotState>>({})
function dotOf(name: string): DotState {
  return dotState.value[name] ?? { status: 'idle' }
}
function setDot(name: string, s: DotState) {
  dotState.value = { ...dotState.value, [name]: s }
}
function dotTitle(name: string): string {
  const d = dotOf(name)
  if (d.status === 'ok') return `测试通过 · ${d.latency_ms ?? '?'}ms`
  if (d.status === 'fail') return `测试失败（${d.kind ?? 'other'}）：${d.detail ?? ''}`
  if (d.status === 'unsupported') return d.detail || '不支持对话测试'
  return d.status === 'testing' ? '测试中…' : '未测试'
}

/** ① 单模型测试——消费后端真实响应（ok/fail/unsupported 三态染点）。
 * quiet = 批量测试时跳过逐模型 toast（只染点，汇总 toast 兜底）。 */
async function testModel(name: string, quiet = false) {
  setDot(name, { status: 'testing' })
  try {
    const data = await request('models', 'test', { name })
    if (data?.status === 'ok') {
      setDot(name, { status: 'ok', latency_ms: data.latency_ms })
      if (!quiet) toast.success(`${name} 测试通过（${data.latency_ms}ms · ${data.lane}）`)
    } else if (data?.status === 'unsupported') {
      setDot(name, { status: 'unsupported', detail: data.detail })
      if (!quiet) toast.info(data.detail || '该条目不支持对话测试')
    } else {
      setDot(name, { status: 'fail', kind: data?.kind, detail: data?.detail })
      if (!quiet) toast.error(`${name} 测试失败：${data?.detail || '未知错误'}`)
    }
  } catch (e: any) {
    setDot(name, { status: 'fail', kind: 'error', detail: String(e) })
    if (!quiet) toast.error('测试失败: ' + e)
  }
}

/** ⑤ 一键测试全部：并发 3 的 worker 池（已拍板，防中继限流），完成即染点；可中途取消。 */
const testProgress = ref<{ done: number; total: number } | null>(null)
const testAllRunning = ref(false)
let testAllCancel = false
async function testAll() {
  if (testAllRunning.value) return
  testAllRunning.value = true
  testAllCancel = false
  const names = models.value.map((m) => m.model_name)
  testProgress.value = { done: 0, total: names.length }
  const queue = [...names]
  const CONCURRENCY = 3
  const worker = async () => {
    while (queue.length > 0 && !testAllCancel) {
      const name = queue.shift()!
      await testModel(name, true)
      if (testProgress.value) {
        testProgress.value = { ...testProgress.value, done: testProgress.value.done + 1 }
      }
    }
  }
  await Promise.all(Array.from({ length: Math.min(CONCURRENCY, names.length) }, worker))
  testAllRunning.value = false
  const okN = names.filter((n) => dotOf(n).status === 'ok').length
  const failN = names.filter((n) => dotOf(n).status === 'fail').length
  const skipN = names.length - okN - failN
  if (testAllCancel) {
    toast.warn(`批量测试已取消（完成 ${testProgress.value?.done ?? 0}/${names.length}）`)
  } else {
    toast.success(`批量测试完成：${okN} 通过 / ${failN} 失败 / ${skipN} 跳过（图像 lane）`)
  }
  setTimeout(() => { testProgress.value = null }, 3000)
}
function cancelTestAll() {
  testAllCancel = true
}

/** ② 复制模型（服务端深拷贝整条目含 key——前端拿到的 key 是脱敏的）。
 * 模型 ID 可改（2026-10-27 漏项补救）：复制即换 LLM，一步到位。 */
const copySource = ref<string | null>(null)
const copyNewName = ref('')
const copyNewModel = ref('')
const copyOrigModel = ref('')
const copyBusy = ref(false)
function openCopy(m: Model) {
  copySource.value = m.model_name
  copyNewName.value = m.model_name + '-copy'
  copyOrigModel.value = m.model
  copyNewModel.value = m.model // 预填源模型 ID，直接改成要用的 LLM
}
async function submitCopy() {
  if (!copySource.value) return
  const nn = copyNewName.value.trim()
  if (!nn) { toast.warn('请输入新名称'); return }
  const nm = copyNewModel.value.trim()
  if (!nm) { toast.warn('请输入模型 ID（实际调用的 LLM）'); return }
  copyBusy.value = true
  try {
    const payload: any = { name: copySource.value, new_name: nn }
    if (nm !== copyOrigModel.value) payload.new_model = nm
    await request('models', 'copy', payload)
    toast.success(nm !== copyOrigModel.value ? `已复制为 ${nn}（指向 ${nm}）` : `已复制为 ${nn}（同供应商加模型的最快路径）`)
    copySource.value = null
    await loadModels()
  } catch (e: any) {
    toast.error('复制失败: ' + e)
  }
  copyBusy.value = false
}

/** ③ 改名/换模型（后端同步三处引用；若改的是当前默认模型，自动热切换——
 * 显示名与模型 ID 同一趟 RMW 落盘，热切从 config 重解析一并生效）。 */
const renameSource = ref<string | null>(null)
const renameNewName = ref('')
const renameNewModel = ref('')
const renameOrigModel = ref('')
const renameBusy = ref(false)
function openRename(m: Model) {
  renameSource.value = m.model_name
  renameNewName.value = m.model_name
  renameOrigModel.value = m.model
  renameNewModel.value = m.model
}
async function submitRename() {
  const oldName = renameSource.value
  if (!oldName) return
  const nn = renameNewName.value.trim()
  if (!nn) { toast.warn('请输入新名称'); return }
  const nm = renameNewModel.value.trim()
  if (!nm) { toast.warn('请输入模型 ID（实际调用的 LLM）'); return }
  const nameChanged = nn !== oldName
  const modelChanged = nm !== renameOrigModel.value
  if (!nameChanged && !modelChanged) { toast.info('没有修改'); return }
  renameBusy.value = true
  try {
    const payload: any = { name: oldName, new_name: nn }
    if (modelChanged) payload.new_model = nm
    await request('models', 'rename', payload)
    const parts = [
      nameChanged ? `${oldName} → ${nn}` : null,
      modelChanged ? `模型 → ${nm}` : null,
    ].filter(Boolean)
    toast.success(`已保存：${parts.join('；')}（引用已同步）`)
    // 圆点状态跟名迁移（点状态属于模型本身，不属于名字）。
    if (nameChanged) {
      const old = dotState.value[oldName]
      if (old) {
        const rest = { ...dotState.value }
        delete rest[oldName]
        dotState.value = { ...rest, [nn]: old }
      }
    }
    renameSource.value = null
    await loadModels()
  } catch (e: any) {
    toast.error('改名失败: ' + e)
  }
  renameBusy.value = false
}

/** 列表模式的属性入口：切回卡片视图并展开该模型属性编辑器。 */
function openAttrsFromList(m: Model) {
  if (!expandedAttrs.value.has(m.model_name)) toggleAttrs(m)
  setViewMode('card')
}

// P3-2: attribute editor — which cards are expanded + per-card drafts.
const expandedAttrs = ref<Set<string>>(new Set())
interface AttrDraft {
  tier: string
  effort: string // '' = off（不发送）
  size: string
  realName: string
  ctx: string
  compactCtx: string // '' = 自动（窗口 × 75%）
  protocol: string // '' = 自动识别（按名称前缀）
  stream: string // '' = auto（跟随 lane 默认）
}
const attrDrafts = ref<Record<string, AttrDraft>>({})

// P3-2: models.dev catalog cache state + refresh busy flag.
interface CatalogInfo { exists: boolean; fetched_at: string; entries: number }
const catalogInfo = ref<CatalogInfo | null>(null)
const catalogUpdating = ref(false)

// P2B: 模型工具健康（models.health）——近 N 天参数校验失败趋势，
// 样本足够且失败率高时后端附 tier 校准建议（hint）。
interface ToolHealthRow {
  model: string
  tool_calls: number
  validation_failures: number
  failure_rate: number
  hint?: string | null
  pct: number
}
const toolHealth = ref<ToolHealthRow[]>([])
const toolHealthDays = ref(0)
const toolHealthNote = ref('')
const healthByName = computed<Record<string, ToolHealthRow>>(() => {
  const map: Record<string, ToolHealthRow> = {}
  for (const r of toolHealth.value) map[r.model] = r
  return map
})

async function loadToolHealth() {
  try {
    const data = await request('models', 'health')
    toolHealth.value = (data?.models || []).map((r: any) => ({
      ...r,
      pct: Math.round((r.failure_rate || 0) * 100),
    }))
    toolHealthDays.value = data?.days || 0
    toolHealthNote.value = data?.note || ''
  } catch {
    // 健康面板是增值信息，拉取失败不打扰主列表。
  }
}

async function loadModels() {
  try {
    const data = await request('models', 'list')
    models.value = data?.models || []
  } catch (e: any) {
    toast.error('加载模型失败: ' + e)
  }
  loading.value = false
}

// P10（能力扩展 WS5）：模型列表按 provider 家族分组展示 + 家族筛选。
// 家族推断/分组逻辑在 utils/providerFamilies.ts（镜像 P9 预设表）。
const familyFilter = ref('') // '' = 全部家族
const collapsedGroups = ref<Set<string>>(new Set())
const familyGroups = computed(() => groupModelsByFamily(models.value, (m) => m.model))
const visibleGroups = computed(() =>
  familyFilter.value ? familyGroups.value.filter((g) => g.id === familyFilter.value) : familyGroups.value,
)
function toggleGroup(id: string) {
  const s = new Set(collapsedGroups.value)
  if (s.has(id)) s.delete(id)
  else s.add(id)
  collapsedGroups.value = s
}

// G4 (U15)：key 来源徽标（env 绿 / yaml 蓝 / inline 黄「⚠ 明文」/ none 灰）。
const KEY_SOURCE_BADGES: Record<string, { label: string; cls: string; title: string }> = {
  env: { label: 'env 环境变量', cls: 'ks-env', title: 'Key 存于环境变量，config.json 不含明文（推荐）' },
  yaml: { label: 'yaml 引用', cls: 'ks-yaml', title: 'Key 存于 workspace/config/credentials.yaml（0600，推荐）' },
  inline: { label: '⚠ 明文', cls: 'ks-inline', title: 'Key 以明文写在 config.json，建议迁移到 env 或 credentials.yaml' },
  none: { label: '无 key', cls: 'ks-none', title: '未配置 API Key' },
}
function keySourceBadge(m: Model) {
  return KEY_SOURCE_BADGES[m.key_source?.kind ?? 'none'] ?? KEY_SOURCE_BADGES.none
}
// 存在明文 key → 顶部显示「迁移明文 key」引导卡。
const inlineKeyModels = computed(() => models.value.filter(m => m.key_source?.kind === 'inline'))
// 明文 key 一步迁移（2026-10-27）：页面内一键迁 credentials.yaml / 加密
// vault——免 CLI 免重启。保值迁移（Key 原值搬入存储，config.json 只留
// 引用），运行中模型行为不变；vault 的 argon2 锁定形态先弹口令行。
const migrating = ref(false)
const showVaultPw = ref(false)
const vaultPwInput = ref('')

async function migrateInlineKeys(target: 'yaml' | 'vault', passphrase?: string) {
  if (migrating.value) return
  migrating.value = true
  try {
    const payload: Record<string, unknown> = { target }
    if (passphrase) payload.passphrase = passphrase
    const r = await request('models', 'migrate_keys', payload)
    const n = r?.migrated?.length ?? 0
    if (n > 0) {
      toast.success(`已迁移 ${n} 个模型的 Key（config.json 只留引用，模型行为不变）`)
    } else {
      toast.success('没有需要迁移的明文 Key')
    }
    dismissVaultPw()
    await loadModels()
  } catch (e: any) {
    toast.error('迁移失败: ' + e)
  } finally {
    migrating.value = false
  }
}

/** vault 目标：argon2 锁定时先要口令（DPAPI / 已带 env 口令直接执行）。 */
async function migrateToVault() {
  if (migrating.value) return
  try {
    const st = await request('vault', 'status')
    if (st?.exists && st?.mode === 'argon2id' && !st?.unlocked) {
      showVaultPw.value = true
      return
    }
  } catch {
    // status 拉不到（如未注册）就直接试——vault 臂会诚实报错。
  }
  await migrateInlineKeys('vault')
}

async function confirmVaultPw() {
  const pw = vaultPwInput.value
  if (!pw || migrating.value) return
  await migrateInlineKeys('vault', pw)
}

function dismissVaultPw() {
  showVaultPw.value = false
  vaultPwInput.value = ''
}

async function loadCatalogInfo() {
  try {
    catalogInfo.value = await request('models', 'catalog_info')
  } catch {
    catalogInfo.value = { exists: false, fetched_at: '', entries: 0 }
  }
}

/** P3-2: 拉取 models.dev 目录（后端 spawn CLI `model catalog-update`，90s 超时）。 */
async function updateCatalog() {
  if (catalogUpdating.value) return
  catalogUpdating.value = true
  try {
    catalogInfo.value = await request('models', 'catalog_update')
    toast.success(`模型目录已更新（${catalogInfo.value?.entries ?? 0} 条）`)
    await loadModels() // catalog_match per model may have changed
  } catch (e: any) {
    toast.error('目录更新失败: ' + e)
  }
  catalogUpdating.value = false
}

function toggleAttrs(m: Model) {
  const s = new Set(expandedAttrs.value)
  if (s.has(m.model_name)) {
    s.delete(m.model_name)
  } else {
    s.add(m.model_name)
    // Seed the draft from current values (null → empty/off).
    attrDrafts.value[m.model_name] = {
      tier: m.model_tier || 'auto',
      effort: m.reasoning_effort || '',
      size: m.model_size_b != null ? String(m.model_size_b) : '',
      realName: m.real_name || '',
      ctx: m.context_window != null ? String(m.context_window) : '',
      compactCtx: m.compact_context_tokens != null ? String(m.compact_context_tokens) : '',
      protocol: m.protocol || '',
      stream: m.stream || '',
    }
  }
  expandedAttrs.value = s
}

/** v-model 对 type="number" 输入自动转 number（Vue looseToNumber，输入合法
 * 数字时草稿从字符串漂移成 number）——凡从 attrDrafts 读值做字符串运算/
 * 比较必须先经本函数归一，否则 number.trim() 抛 TypeError 炸掉整棵渲染树
 * （BUG-20261029-175）。null/undefined 归空串，与 seed 语义一致。 */
function asStr(v: unknown): string {
  return String(v ?? '')
}

/** Fields whose draft differs from the loaded values (only these are saved). */
function dirtyFields(m: Model): string[] {
  const d = attrDrafts.value[m.model_name]
  if (!d) return []
  const out: string[] = []
  if ((m.model_tier || 'auto') !== d.tier) out.push('model_tier')
  if ((m.reasoning_effort || '') !== d.effort) out.push('reasoning_effort')
  const sizeStr = m.model_size_b != null ? String(m.model_size_b) : ''
  if (sizeStr !== asStr(d.size).trim()) out.push('model_size_b')
  if ((m.real_name || '') !== asStr(d.realName).trim()) out.push('real_name')
  const ctxStr = m.context_window != null ? String(m.context_window) : ''
  if (ctxStr !== asStr(d.ctx).trim()) out.push('context_window')
  const ccStr = m.compact_context_tokens != null ? String(m.compact_context_tokens) : ''
  if (ccStr !== asStr(d.compactCtx).trim()) out.push('compact_context_tokens')
  if ((m.protocol || '') !== d.protocol) out.push('protocol')
  if ((m.stream || '') !== d.stream) out.push('stream')
  return out
}

/** 生效方式标注（对码结论）：tier/effort 走 agent 每轮 LLM 开头的
 * check_config_reload 重读（改完即时生效）；size/real_name 参与 auto 档
 * 检测、同链路生效；显式 tier 下 size/real_name 不参与。 */
const FIELD_EFFECT: Record<string, string> = {
  model_tier: '即时生效（下一次 LLM 轮自动重读配置）',
  reasoning_effort: '即时生效（下次 LLM 调用前重读配置）',
  model_size_b: 'auto 档下即时生效（参与能力自动检测）',
  real_name: 'auto 档下即时生效（别名识别真名）',
  context_window: '保存后生效（上下文预算依据）',
  compact_context_tokens: '即时生效（每轮压缩检查前重读配置）；留空 = 自动（窗口 × 75%）',
  protocol: '改默认模型立即生效（自动热切）；其他模型设为默认时生效；或重启后生效',
  stream: '改默认模型立即生效（自动热切）；其他模型设为默认时生效；或重启后生效',
}

/** P3-2: 保存属性 — 逐字段走后端 raw-JSON RMW（保留 config.json 其余键）。 */
async function saveAttrs(m: Model) {
  const d = attrDrafts.value[m.model_name]
  if (!d) return
  const fields = dirtyFields(m)
  if (fields.length === 0) {
    toast.info('没有修改过的属性')
    return
  }
  const sizeRaw = asStr(d.size).trim()
  const ctxRaw = asStr(d.ctx).trim()
  const values: Record<string, unknown> = {
    model_tier: d.tier,
    // ''（off）由后端归一为空串 = 不发送
    reasoning_effort: d.effort || 'off',
    model_size_b: sizeRaw === '' ? null : Number(sizeRaw),
    real_name: asStr(d.realName).trim(),
    context_window: ctxRaw === '' ? null : Number(ctxRaw),
    // 压缩点后端支持空串清除（移键落盘）——直接发串，不做 v1 skip-null。
    compact_context_tokens: asStr(d.compactCtx).trim(),
    protocol: d.protocol, // 空串 = 自动识别（后端接受空串清除，非 null）
    stream: d.stream, // 空串 = auto（后端接受空串清除，非 null）
  }
  // v1 不支持写 null：清空场景跳过该字段，保存名单只含真正落盘的字段，
  // toast 如实反映（不能宣称保存了实际没写的字段）。
  const skipped: string[] = []
  for (const f of fields) {
    if (values[f] == null) { skipped.push(f); continue } // 清空场景：v1 不支持写 null，留原值
    try {
      await request('models', 'update_field', { name: m.model_name, field: f, value: values[f] })
    } catch (e: any) {
      toast.error(`${f} 保存失败: ` + e)
      await loadModels()
      return
    }
  }
  const saved = fields.filter((f) => !skipped.includes(f))
  if (saved.length > 0) toast.success(`已保存：${saved.join('、')}`)
  if (skipped.length > 0) {
    toast.info(`未保存（v1 暂不支持清空，保留原值）：${skipped.join('、')}`)
  }
  await loadModels()
  // Re-seed the draft from the refreshed values so the dirty diff resets.
  const fresh = models.value.find((x) => x.model_name === m.model_name)
  if (fresh) {
    attrDrafts.value[m.model_name] = {
      tier: fresh.model_tier || 'auto',
      effort: fresh.reasoning_effort || '',
      size: fresh.model_size_b != null ? String(fresh.model_size_b) : '',
      realName: fresh.real_name || '',
      ctx: fresh.context_window != null ? String(fresh.context_window) : '',
      compactCtx: fresh.compact_context_tokens != null ? String(fresh.compact_context_tokens) : '',
      protocol: fresh.protocol || '',
      stream: fresh.stream || '',
    }
  }
}

/** P3-2: context_window 一键填入 models.dev 目录值。 */
function fillCtxFromCatalog(m: Model) {
  const cw = m.catalog_match?.context_window
  if (cw == null) return
  const d = attrDrafts.value[m.model_name]
  if (d) d.ctx = String(cw)
}

/** 压缩点超窗口的前端黄字提醒（真实钳制在网关运行期，一次性 warn）。
 * 窗口取条目 context_window，缺省回落目录值；两者皆缺 = 无法预判，不提示。 */
function compactCtxClampHint(m: Model): string {
  const d = attrDrafts.value[m.model_name]
  if (!d) return ''
  const raw = asStr(d.compactCtx).trim()
  if (raw === '') return ''
  const v = Number(raw)
  if (!Number.isFinite(v) || v <= 0) return ''
  const winRaw = m.context_window ?? m.catalog_match?.context_window
  const win = winRaw != null ? Number(winRaw) : NaN
  if (Number.isFinite(win) && win > 0 && v > win) {
    return `⚠ 设定 ${v.toLocaleString()} 超过窗口 ${win.toLocaleString()}，会被钳到窗口；如需更大请先设 context_window`
  }
  return ''
}

async function addModel() {
  if (!addForm.value.name) { toast.warn('请输入模型名称'); return }
  if (!addForm.value.model) { toast.warn('请输入模型 ID'); return }
  if (!addForm.value.key) { toast.warn('请输入 API Key'); return }
  try {
    const payload: any = {
      name: addForm.value.name,
      model: addForm.value.model,
      key: addForm.value.key,
    }
    if (addForm.value.base_url) payload.base_url = addForm.value.base_url
    if (addForm.value.proxy) payload.proxy = addForm.value.proxy
    payload.protocol = addForm.value.protocol
    if (addForm.value.stream) payload.stream = addForm.value.stream // 缺省不传 = auto
    await request('models', 'add', payload)
    toast.success('模型已添加')
    showAdd.value = false
    addForm.value = { name: '', model: '', key: '', base_url: '', proxy: '', protocol: 'anthropic', stream: '' }
    await loadModels()
  } catch (e: any) {
    toast.error('添加失败: ' + e)
  }
}

async function deleteModel(name: string) {
  if (!confirm(`确定删除模型 "${name}" 吗？`)) return
  try {
    await request('models', 'delete', { name })
    toast.success('已删除')
    await loadModels()
  } catch (e: any) {
    toast.error('删除失败: ' + e)
  }
}

async function setDefault(name: string) {
  const prev = switching.value
  switching.value = name
  try {
    await request('models', 'set_default', { name })
    toast.success(`${name} 已设为默认模型，立即生效`)
    await loadModels()
  } catch (e: any) {
    toast.error('设置失败: ' + e)
  }
  switching.value = null
}

onMounted(() => {
  loadModels()
  loadCatalogInfo()
  loadToolHealth()
})
</script>

<template>
  <div class="page-models">
    <div class="page-header">
      <h2>模型管理</h2>
      <div class="page-header-actions">
        <!-- P3-2: models.dev 目录缓存状态 + 一键刷新（后端 spawn CLI，90s 超时） -->
        <span v-if="catalogInfo" class="catalog-hint">
          {{ catalogInfo.exists
              ? `目录缓存：${catalogInfo.entries} 条 · ${catalogInfo.fetched_at || '时间未知'}`
              : '目录未拉取（context_window 自动填充需先更新目录）' }}
        </span>
        <button class="btn" :disabled="catalogUpdating" @click="updateCatalog">
          <span v-if="catalogUpdating" class="spinner" style="width:14px;height:14px;"></span>
          {{ catalogUpdating ? '拉取中…' : '更新模型目录' }}
        </button>
        <!-- P10: 家族筛选下拉（家族分组推断自 P9 预设表镜像） -->
        <select v-if="models.length > 0" v-model="familyFilter" class="form-input family-filter" title="按 provider 家族筛选">
          <option value="">全部家族</option>
          <option v-for="g in familyGroups" :key="g.id" :value="g.id">{{ g.label }}（{{ g.items.length }}）</option>
        </select>
        <!-- 五项增强批④：卡片 | 列表 视图切换（localStorage 持久） -->
        <div v-if="models.length > 0" class="view-toggle" role="group" aria-label="视图切换">
          <button :class="{ active: viewMode === 'card' }" @click="setViewMode('card')">卡片</button>
          <button :class="{ active: viewMode === 'list' }" @click="setViewMode('list')">列表</button>
        </div>
        <!-- 五项增强批⑤：一键测试全部（并发 3，完成即染点，可取消） -->
        <button v-if="models.length > 0 && !testAllRunning" class="btn" @click="testAll" title="测试全部模型（并发 3）">⚡ 一键测试</button>
        <template v-if="testAllRunning">
          <span class="test-progress">测试中 {{ testProgress?.done }}/{{ testProgress?.total }}</span>
          <button class="btn btn-danger" @click="cancelTestAll">取消</button>
        </template>
        <button class="btn btn-primary" @click="showAdd = !showAdd">{{ showAdd ? '取消' : '+ 添加模型' }}</button>
      </div>
    </div>
    <div class="page-body">
      <!-- G4 (U15)：key 来源说明 + 明文迁移引导 -->
      <div class="key-source-hint">
        <span>🔐 Key 来源：</span>
        <span class="ks-badge ks-env">env 环境变量</span>
        <span class="ks-badge ks-yaml">yaml 引用</span>
        <span class="ks-badge ks-inline">⚠ 明文</span>
        <span class="ks-badge ks-none">无 key</span>
        <span class="key-source-hint-note">推荐 env / yaml —— config.json 不落明文；明文 key 可在下方一键迁移</span>
      </div>
      <div v-if="inlineKeyModels.length > 0" class="card key-import-card">
        <div class="key-import-body">
          <span class="key-import-title">⚠ 检测到 {{ inlineKeyModels.length }} 个模型使用明文 Key（写在 config.json）</span>
          <span class="key-import-note">一键迁移（免命令行、免重启）：Key 原值搬入存储，config.json 只留引用——运行中的模型行为不变。</span>
        </div>
        <div class="key-import-actions">
          <button class="btn btn-sm btn-primary" :disabled="migrating" @click="migrateInlineKeys('yaml')">{{ migrating ? '迁移中…' : '迁移到 credentials.yaml' }}</button>
          <button class="btn btn-sm" :disabled="migrating" @click="migrateToVault">迁移到加密 vault</button>
        </div>
      </div>
      <div v-if="showVaultPw" class="card key-import-card vault-pw-card">
        <span class="key-import-note">vault 为 argon2 口令模式且当前锁定——输入口令解锁并迁移（口令只用于本次请求，不落盘不落日志）：</span>
        <input class="form-input vault-pw-input" type="password" v-model="vaultPwInput" placeholder="vault 口令" @keyup.enter="confirmVaultPw" />
        <button class="btn btn-sm btn-primary" :disabled="migrating || !vaultPwInput" @click="confirmVaultPw">解锁并迁移</button>
        <button class="btn btn-sm" :disabled="migrating" @click="dismissVaultPw">取消</button>
      </div>
      <!-- Add form -->
      <div v-if="showAdd" class="card" style="margin-bottom: var(--space-4);">
        <div class="card-header"><h3>添加模型</h3></div>
        <div class="card-body">
          <div style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-3);">
            <div class="form-group">
              <label class="form-label">名称 *（显示名称）</label>
              <input class="form-input" v-model="addForm.name" placeholder="例如: 我的GPT4">
            </div>
            <div class="form-group">
              <label class="form-label">模型 ID *（实际调用）</label>
              <input class="form-input" v-model="addForm.model" placeholder="例如: gpt-4o / zhipu/glm-4">
            </div>
            <div class="form-group">
              <label class="form-label">API Key *</label>
              <input class="form-input" type="password" v-model="addForm.key" placeholder="sk-...">
            </div>
            <div class="form-group">
              <label class="form-label">Base URL</label>
              <input class="form-input" v-model="addForm.base_url" placeholder="https://api.openai.com/v1">
            </div>
            <div class="form-group">
              <label class="form-label">协议类型</label>
              <select class="form-input" v-model="addForm.protocol">
                <option v-for="p in PROTOCOL_OPTIONS" :key="p.value" :value="p.value">{{ p.label }}</option>
              </select>
            </div>
            <div class="form-group">
              <label class="form-label">流式开关</label>
              <select class="form-input" v-model="addForm.stream">
                <option v-for="s in STREAM_OPTIONS" :key="s.value" :value="s.value">{{ s.label }}</option>
              </select>
            </div>
          </div>
          <div class="form-group" style="margin-top: var(--space-3);">
            <label class="form-label">代理</label>
            <input class="form-input" v-model="addForm.proxy" placeholder="http://proxy:port" style="max-width: 300px;">
          </div>
          <div style="margin-top: var(--space-3); display: flex; justify-content: flex-end; gap: var(--space-2);">
            <button class="btn" @click="showAdd = false">取消</button>
            <button class="btn btn-primary" @click="addModel">添加</button>
          </div>
        </div>
      </div>

      <!-- Loading -->
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <!-- Empty -->
      <div v-if="!loading && models.length === 0" class="empty-state">
        <h3>暂无模型</h3>
        <p>点击上方"添加模型"按钮配置第一个 AI 模型</p>
      </div>

      <!-- Model list（P10：provider 家族分组 + 折叠 + 筛选；五项增强批④：卡片 | 列表双视图） -->
      <div v-if="!loading && models.length > 0">
        <template v-if="viewMode === 'card'">
        <template v-if="visibleGroups.length > 0">
          <section v-for="g in visibleGroups" :key="g.id || 'unknown'" class="family-group">
            <button type="button" class="family-header" @click="toggleGroup(g.id)">
              <span class="family-chevron" :class="{ 'family-chevron--open': !collapsedGroups.has(g.id) }">&#9656;</span>
              <span class="family-name">{{ g.label }}</span>
              <span class="family-count">{{ g.items.length }}</span>
            </button>
            <div v-show="!collapsedGroups.has(g.id)" class="family-grid">
        <div
          v-for="m in g.items"
          :key="m.model_name"
          class="card model-card"
          :class="{ 'model-card--default': m.is_default, 'model-card--switching': switching === m.model_name }"
        >
          <div class="card-header">
            <h3>
              <span class="test-dot" :class="'dot--' + dotOf(m.model_name).status" :title="dotTitle(m.model_name)"></span>{{ m.model_name }}
            </h3>
            <div style="display: flex; gap: var(--space-2); align-items: center;">
              <span v-if="m.is_default" class="badge badge-success">&#10003; 默认</span>
              <span v-if="isImageModel(m)" class="badge badge-warning" title="图像生成条目：仅供 generate_image 工具消费，不能作对话模型/默认模型">🖼 图像</span>
              <span v-if="m.model" class="badge badge-info">{{ m.model }}</span>
            </div>
          </div>
          <div class="card-body">
            <div class="settings-grid" style="font-size: var(--text-sm);">
              <span class="settings-key">API Key</span>
              <span class="settings-value">
                {{ m.api_key || '--' }}
                <!-- G4 (U15)：key 来源徽标 -->
                <span class="ks-badge" :class="keySourceBadge(m).cls" :title="keySourceBadge(m).title">{{ keySourceBadge(m).label }}</span>
              </span>
              <span class="settings-key">Base URL</span>
              <span class="settings-value">{{ m.api_base || '--' }}</span>
              <span class="settings-key">代理</span>
              <span class="settings-value">{{ m.proxy || '--' }}</span>
              <span class="settings-key">协议</span>
              <span class="settings-value">{{ isImageModel(m) ? '图像生成（generate_image 专用）' : (m.protocol || '自动识别') }}</span>
              <span class="settings-key">能力档</span>
              <span class="settings-value">{{ m.model_tier || 'auto（自动检测）' }}</span>
              <span class="settings-key">工具健康<template v-if="toolHealthDays"> · 近{{ toolHealthDays }}天</template></span>
              <span class="settings-value">
                <template v-if="healthByName[m.model_name]">
                  {{ healthByName[m.model_name].tool_calls }} 次调用 · 校验失败
                  {{ healthByName[m.model_name].validation_failures }}
                  <span
                    class="badge"
                    :class="(healthByName[m.model_name].pct >= 20 && healthByName[m.model_name].tool_calls >= 10) ? 'badge-warning' : 'badge-success'"
                  >{{ healthByName[m.model_name].pct }}%</span>
                </template>
                <template v-else-if="toolHealthNote">{{ toolHealthNote }}</template>
                <template v-else>暂无调用记录</template>
              </span>
            </div>
            <!-- P2B: 后端阈值建议（样本足够且失败率 ≥20%） -->
            <p v-if="healthByName[m.model_name]?.hint" class="tool-health-hint">
              💡 {{ healthByName[m.model_name].hint }}
            </p>
          </div>
          <!-- P3-2: 属性编辑展开区（tier / effort / 参数量 / 真名 / context_window / protocol / stream） -->
          <div v-if="expandedAttrs.has(m.model_name) && attrDrafts[m.model_name]" class="attr-editor">
            <div class="attr-grid">
              <div class="attr-field">
                <label class="form-label">协议类型</label>
                <select class="form-input" v-model="attrDrafts[m.model_name].protocol">
                  <option value="">自动识别（按模型名前缀）</option>
                  <option v-for="p in PROTOCOL_OPTIONS" :key="p.value" :value="p.value">{{ p.label }}</option>
                </select>
                <span class="attr-effect">{{ FIELD_EFFECT['protocol'] }}</span>
              </div>
              <div class="attr-field">
                <label class="form-label">流式开关</label>
                <select class="form-input" v-model="attrDrafts[m.model_name].stream">
                  <option v-for="s in STREAM_OPTIONS" :key="s.value" :value="s.value">{{ s.label }}</option>
                </select>
                <span class="attr-effect">{{ FIELD_EFFECT['stream'] }}</span>
              </div>
              <div class="attr-field">
                <label class="form-label">能力档 tier</label>
                <select class="form-input" v-model="attrDrafts[m.model_name].tier">
                  <option value="auto">auto（自动检测）</option>
                  <option value="mini">mini（小模型 · 核心 13 工具）</option>
                  <option value="normal">normal（中模型 · ~26 工具）</option>
                  <option value="big">big（大模型 · 全量 42 工具）</option>
                </select>
                <span class="attr-effect">{{ FIELD_EFFECT['model_tier'] }}</span>
              </div>
              <div class="attr-field">
                <label class="form-label">推理力度 effort</label>
                <select class="form-input" v-model="attrDrafts[m.model_name].effort">
                  <option value="">off（不发送）</option>
                  <option value="low">low</option>
                  <option value="medium">medium</option>
                  <option value="high">high</option>
                </select>
                <span class="attr-effect">{{ FIELD_EFFECT['reasoning_effort'] }}</span>
              </div>
              <div class="attr-field">
                <label class="form-label">参数量（十亿参数，如 30 = 30B）</label>
                <input class="form-input" type="number" min="1" v-model="attrDrafts[m.model_name].size" placeholder="30" />
                <span class="attr-effect">{{ FIELD_EFFECT['model_size_b'] }}</span>
              </div>
              <div class="attr-field">
                <label class="form-label">真名（别名模型的实际型号名）</label>
                <input class="form-input" v-model="attrDrafts[m.model_name].realName" placeholder="如 Qwen3-30B" />
                <span class="attr-effect">{{ FIELD_EFFECT['real_name'] }}</span>
              </div>
              <div class="attr-field attr-field--wide">
                <label class="form-label">
                  context_window
                  <template v-if="m.catalog_match?.context_window">
                    · 目录值：{{ m.catalog_match.context_window.toLocaleString() }}
                    <a href="javascript:void(0)" @click="fillCtxFromCatalog(m)">填入</a>
                  </template>
                </label>
                <input class="form-input" type="number" min="1" v-model="attrDrafts[m.model_name].ctx" placeholder="131072" />
                <span class="attr-effect">
                  {{ FIELD_EFFECT['context_window'] }}
                  <template v-if="m.catalog_match?.family">（models.dev：{{ m.catalog_match.family }}）</template>
                </span>
              </div>
              <div class="attr-field attr-field--wide">
                <label class="form-label">上下文压缩点 compact_context_tokens（设 N = 到 N token 即压缩，不乘比例）</label>
                <input class="form-input" type="number" min="1" v-model="attrDrafts[m.model_name].compactCtx" placeholder="留空 = 自动（窗口 × 75%）" />
                <span v-if="compactCtxClampHint(m)" class="attr-effect attr-clamp-warn">{{ compactCtxClampHint(m) }}</span>
                <span class="attr-effect">
                  {{ FIELD_EFFECT['compact_context_tokens'] }}。建议大于尾巴保留预算（默认 20000），否则压缩无法推进
                </span>
              </div>
            </div>
            <div class="attr-actions">
              <span class="attr-dirty">{{ dirtyFields(m).length ? `已修改：${dirtyFields(m).join('、')}` : '无修改' }}</span>
              <button class="btn btn-sm" @click="toggleAttrs(m)">收起</button>
              <button class="btn btn-sm btn-primary" @click="saveAttrs(m)">保存属性</button>
            </div>
          </div>
          <div class="card-footer">
            <button class="btn btn-sm btn-ghost" @click="testModel(m.model_name)" :disabled="dotOf(m.model_name).status === 'testing'">
              <span v-if="dotOf(m.model_name).status === 'testing'" class="spinner" style="width:14px;height:14px;"></span>
              {{ dotOf(m.model_name).status === 'testing' ? '测试中...' : '测试' }}
            </button>
            <button class="btn btn-sm btn-ghost" @click="toggleAttrs(m)">
              {{ expandedAttrs.has(m.model_name) ? '收起属性' : '属性' }}
            </button>
            <button class="btn btn-sm btn-ghost" title="整条配置深拷贝（含 Key），同供应商加模型" @click="openCopy(m)">复制</button>
            <button class="btn btn-sm btn-ghost" title="改名 / 换模型（引用自动同步；默认模型自动热切）" @click="openRename(m)">改名</button>
            <button
              v-if="!m.is_default"
              class="btn btn-sm btn-primary"
              @click="setDefault(m.model_name)"
              :disabled="switching !== null || isImageModel(m)"
              :title="isImageModel(m) ? '图像生成条目不能设为默认对话模型（仅供 generate_image 工具）' : ''"
            >
              <span v-if="switching === m.model_name" class="spinner" style="width:14px;height:14px;"></span>
              {{ switching === m.model_name ? '切换中...' : '设为默认' }}
            </button>
            <span v-else class="model-active-label">当前使用中</span>
            <button class="btn btn-sm btn-danger" @click="deleteModel(m.model_name)" :disabled="switching !== null">删除</button>
          </div>
        </div>
            </div>
          </section>
        </template>
        <div v-else class="empty-state">
          <h3>该家族暂无模型</h3>
          <p>当前筛选下没有匹配的模型</p>
        </div>
        </template>
        <!-- ④ 列表视图：拍平家族分组的紧凑全景表（一屏可见全部模型） -->
        <div v-else class="table-wrap list-table">
          <table>
            <thead>
              <tr>
                <th class="col-dot"></th>
                <th>显示名称</th>
                <th>模型全名</th>
                <th>协议</th>
                <th>档位</th>
                <th>流式</th>
                <th>默认</th>
                <th>延迟</th>
                <th class="col-actions">操作</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="m in models" :key="m.model_name" :class="{ 'row-default': m.is_default }">
                <td class="col-dot"><span class="test-dot" :class="'dot--' + dotOf(m.model_name).status" :title="dotTitle(m.model_name)"></span></td>
                <td class="cell-name">
                  {{ m.model_name }}
                  <span v-if="isImageModel(m)" class="badge badge-warning" title="图像生成条目：仅供 generate_image 工具">🖼</span>
                </td>
                <td class="cell-mono">{{ m.model }}</td>
                <td>{{ isImageModel(m) ? '图像' : (m.protocol || '自动') }}</td>
                <td>{{ m.model_tier || 'auto' }}</td>
                <td>{{ m.stream === 'yes' ? '流式' : m.stream === 'no' ? '非流式' : 'auto' }}</td>
                <td><span v-if="m.is_default" class="badge badge-success">&#10003; 默认</span></td>
                <td>{{ dotOf(m.model_name).status === 'ok' ? dotOf(m.model_name).latency_ms + 'ms' : '--' }}</td>
                <td class="col-actions">
                  <button class="btn btn-sm btn-ghost" :disabled="dotOf(m.model_name).status === 'testing'" @click="testModel(m.model_name)">测试</button>
                  <button class="btn btn-sm btn-ghost" title="切换到卡片视图编辑属性" @click="openAttrsFromList(m)">属性</button>
                  <button
                    v-if="!m.is_default"
                    class="btn btn-sm btn-primary"
                    :disabled="switching !== null || isImageModel(m)"
                    :title="isImageModel(m) ? '图像生成条目不能设为默认对话模型（仅供 generate_image 工具）' : ''"
                    @click="setDefault(m.model_name)"
                  >设默认</button>
                  <button class="btn btn-sm btn-ghost" @click="openCopy(m)">复制</button>
                  <button class="btn btn-sm btn-ghost" @click="openRename(m)">改名</button>
                  <button class="btn btn-sm btn-danger" @click="deleteModel(m.model_name)">删除</button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>

    <!-- ② 复制模型（服务端深拷贝整条目含 key） -->
    <div v-if="copySource" class="modal-backdrop" @click.self="copySource = null">
      <div class="modal" style="max-width: 420px;">
        <div class="modal-header">
          <h3>复制模型「{{ copySource }}」</h3>
          <button class="modal-close" @click="copySource = null">&times;</button>
        </div>
        <div class="modal-body">
          <div class="form-group">
            <label class="form-label">新名称 *</label>
            <input class="form-input" v-model="copyNewName" @keyup.enter="submitCopy" />
          </div>
          <div class="form-group">
            <label class="form-label">模型 ID *（实际调用的 LLM，可改）</label>
            <input class="form-input" v-model="copyNewModel" placeholder="例如: zhipu/glm-4.7" @keyup.enter="submitCopy" />
          </div>
          <p class="modal-note">整条配置深拷贝（含 API Key 与全部属性）；改掉模型 ID 即指向新 LLM。不会自动设为默认。</p>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="copySource = null">取消</button>
          <button class="btn btn-primary" :disabled="copyBusy" @click="submitCopy">复制</button>
        </div>
      </div>
    </div>

    <!-- ③ 改名（三处引用同步；活跃默认自动热切） -->
    <div v-if="renameSource" class="modal-backdrop" @click.self="renameSource = null">
      <div class="modal" style="max-width: 420px;">
        <div class="modal-header">
          <h3>改名 / 换模型「{{ renameSource }}」</h3>
          <button class="modal-close" @click="renameSource = null">&times;</button>
        </div>
        <div class="modal-body">
          <div class="form-group">
            <label class="form-label">新名称 *（显示名称）</label>
            <input class="form-input" v-model="renameNewName" @keyup.enter="submitRename" />
          </div>
          <div class="form-group">
            <label class="form-label">模型 ID *（实际调用的 LLM，可改）</label>
            <input class="form-input" v-model="renameNewModel" placeholder="例如: zhipu/glm-4.8" @keyup.enter="submitRename" />
          </div>
          <p class="modal-note">默认模型 / 小模型 / 图像模型引用自动同步；若改的是当前默认模型，运行中立即热切换（新模型 ID 一并生效）。</p>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="renameSource = null">取消</button>
          <button class="btn btn-primary" :disabled="renameBusy" @click="submitRename">保存</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* P10: provider 家族分组（折叠组头 + 组内卡片网格） */
.family-group {
  margin-bottom: var(--space-4);
}

.family-header {
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  padding: 4px 8px;
  margin-bottom: var(--space-2);
  border: none;
  background: transparent;
  color: var(--text-primary);
  font-size: var(--text-sm);
  font-weight: 600;
  cursor: pointer;
  border-radius: var(--radius-sm);
}

.family-header:hover {
  background: var(--bg-tertiary);
}

.family-chevron {
  display: inline-block;
  transition: transform 0.15s;
  color: var(--text-muted);
}

.family-chevron--open {
  transform: rotate(90deg);
}

.family-count {
  font-size: var(--text-xs);
  font-weight: 500;
  color: var(--text-muted);
  background: var(--bg-tertiary);
  border-radius: 999px;
  padding: 0 8px;
  line-height: 18px;
}

.family-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
  gap: var(--space-4);
}

.family-filter {
  width: auto;
  min-width: 160px;
}

.model-card {
  transition: border-color 0.25s, box-shadow 0.25s, background-color 0.25s;
}

.model-card--default {
  border-color: var(--color-success, #22c55e);
  box-shadow: 0 0 0 1px var(--color-success, #22c55e), 0 2px 8px rgba(34, 197, 94, 0.15);
}

:root[data-theme='dark'] .model-card--default {
  box-shadow: 0 0 0 1px var(--color-success, #22c55e), 0 2px 12px rgba(34, 197, 94, 0.25);
}

.model-card--switching {
  opacity: 0.7;
  pointer-events: none;
}

.model-active-label {
  font-size: var(--text-sm, 13px);
  color: var(--color-success, #22c55e);
  font-weight: 500;
}

/* P3-2 attribute editor */
.catalog-hint {
  font-size: 12px;
  color: var(--text-muted);
  margin-right: var(--space-2);
}
/* P2B 工具健康：后端阈值建议（tier 校准）注记 */
.tool-health-hint {
  margin: 0;
  padding: 8px 16px;
  border-top: 1px dashed var(--border);
  font-size: 12px;
  color: var(--text-muted);
  white-space: pre-line;
}
.attr-editor {
  padding: 10px 16px;
  border-top: 1px dashed var(--border);
  background: var(--bg-primary);
}
.attr-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 10px 14px;
}
.attr-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.attr-field--wide {
  grid-column: 1 / -1;
}
.attr-effect {
  font-size: 11px;
  color: var(--text-muted);
}
/* 压缩点超窗口前端提醒（黄字；真实钳制与一次性 warn 在网关运行期） */
.attr-clamp-warn {
  font-size: 11px;
  color: var(--warning);
}
.attr-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 10px;
}
.attr-dirty {
  flex: 1;
  font-size: 11px;
  color: var(--text-muted);
}

/* G4 (U15)：key 来源徽标 + 明文迁移引导卡 */
.key-source-hint {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-bottom: var(--space-3);
}

.key-source-hint-note { opacity: 0.85; }

.ks-badge {
  display: inline-block;
  padding: 1px 8px;
  border-radius: var(--radius-sm);
  font-size: 11px;
  font-weight: 600;
  white-space: nowrap;
  vertical-align: middle;
}

.ks-env {
  background: rgba(34, 197, 94, 0.15);
  color: #16a34a;
}

.ks-yaml {
  background: rgba(59, 130, 246, 0.15);
  color: #2563eb;
}

.ks-inline {
  background: rgba(245, 158, 11, 0.18);
  color: #d97706;
}

.ks-none {
  background: var(--bg-tertiary);
  color: var(--text-muted);
}

.key-import-card {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-4);
  border-left: 3px solid #f59e0b;
}

.key-import-body {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: var(--text-sm);
}

.key-import-title { font-weight: 600; color: #d97706; }

.key-import-note { color: var(--text-secondary); font-size: var(--text-xs); }

.key-import-actions { display: flex; gap: var(--space-2); flex-shrink: 0; }

.vault-pw-card { align-items: center; }
.vault-pw-input { max-width: 220px; }

/* ── 五项增强批（2026-10-27）：圆点状态灯 + 视图切换 + 列表模式 ── */
.test-dot {
  display: inline-block;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  margin-right: 6px;
  vertical-align: middle;
  background: var(--text-muted);
  opacity: 0.45;
  cursor: help;
  flex-shrink: 0;
}
.dot--testing {
  background: #f59e0b;
  opacity: 1;
  animation: dot-pulse 1s ease-in-out infinite;
}
.dot--ok {
  background: var(--color-success, #22c55e);
  opacity: 1;
}
.dot--fail {
  background: var(--color-error, #ef4444);
  opacity: 1;
}
@keyframes dot-pulse {
  0%, 100% { transform: scale(1); opacity: 1; }
  50% { transform: scale(1.35); opacity: 0.55; }
}

.view-toggle {
  display: inline-flex;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  overflow: hidden;
}
.view-toggle button {
  border: none;
  background: var(--surface);
  color: var(--text-secondary);
  padding: 6px 14px;
  font-size: var(--text-sm);
  cursor: pointer;
}
.view-toggle button.active {
  background: var(--bg-tertiary);
  color: var(--text-primary);
  font-weight: 600;
}

.test-progress {
  font-size: var(--text-xs);
  color: var(--text-muted);
  white-space: nowrap;
}

.list-table { margin-top: var(--space-2); }
.list-table .cell-name { font-weight: 600; white-space: nowrap; }
.list-table .cell-mono { font-family: monospace; font-size: var(--text-xs); }
.list-table .col-dot { width: 24px; }
.list-table .col-actions { white-space: nowrap; text-align: right; }
.list-table .col-actions .btn { margin-left: 4px; }
.list-table tr.row-default td {
  background: rgba(34, 197, 94, 0.05);
}

.modal-note {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin: 0;
}
</style>
