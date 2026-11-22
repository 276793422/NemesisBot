<script setup lang="ts">
// 事件管理面（项1 事件驱动唤醒 P2，2026-11-16；2026-11-17 规则表单化重构；
// 2026-11-18 双 TAB——用户拍板首页只留信息展示、规则设置挪第二页：总览 =
// 说明 + 泵统计 + 规则总览表 + cron/进程收编，规则设置 = 表单编辑区；切 TAB
// 不强制重载，两 TAB 共享同一份数据 ref）。
// 三族事件源统一视图：
// ① fs 事件规则（events.json 热重载，events.* WSAPI：规则表 + **字段级表单编辑**
//    （原裸 JSON textarea 改为规则卡列表：五族 watch 动态字段组 + deliver 表单 +
//    会话 picker；未知 kind 降级原文编辑、未知字段保真合并——数据不丢）+
//    泵统计）；
// ② 时间触发源（tasks.cron.list 只读收编——管理走任务页，此处只展示）。
//    （进程完成唤醒卡片已随 2026-11-22 完成唤醒摘除批删除。）
// 三闸语义（冷却/小时窗/estop 冻结）由后端 nemesis-events 执行，页面只展示。
// Schema 单一真相源 = crates/nemesis-events/src/rules.rs（五族放行面：
// fs/webhook/proc/sys/probe；cron 已砍单）。前端校验一层（必填/数字域/target
// 形态）提前拦明显错误，后端 rules.rs 校验兜底（错误原文回显保存钮旁）。
import { ref, watch, computed, onMounted, onUnmounted, nextTick } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { useSessionStore } from '../stores/session'

const { request } = useWSAPI()
const toast = useToast()
// 会话 picker 数据源复用全局 session store（HookView 同款，5s 缓存内不重复拉取）。
const sessionStore = useSessionStore()

const loading = ref(true)
const available = ref(true)
// 双 TAB（2026-11-18 用户拍板：首页只留信息展示，规则设置挪第二页）：
// 总览 = 说明 + 泵统计 + 规则总览表 + cron/进程收编；规则设置 = 表单编辑区。
// 切 TAB 不强制重载（两 TAB 共享同一份数据 ref；5s 轮询两 TAB 都续跑，
// rulesDirty 守卫护编辑区——无「切页丢未保存编辑」的新行为）。
const activeTab = ref<'overview' | 'rules'>('overview')
const stats = ref<any>({})
const rules = ref<any[]>([])
const pending = ref<any[]>([])
// sys 采样当前值（懒闸语义：无 sys 规则期间后端不采样 = null，页面隐藏）。
const sysCurrent = ref<any>(null)
const saving = ref(false)
// 保存钮旁错误区（前端校验失败列表 / 后端拒绝原文）——不弹窗。
const saveError = ref<string>('')
// 用户编辑器有未保存修改时，5s 轮询不得覆写表单（否则编辑被冲掉）。
const rulesDirty = ref(false)
// 编辑世代计数（BUG-20261122-005）：load 的 rules.get 与 saveRules 的
// rules.save 都有飞行窗口——窗口内用户可能落键。世代号区分「保存已覆盖的
// 旧编辑」与「窗口内新到的编辑」：成功路径仅世代未变才清 rulesDirty，否则
// 保留 dirty（轮询继续避让表单）并提示再存；load 侧 await 后复核再覆写。
let editSeq = 0
function markDirty() {
  rulesDirty.value = true
  editSeq++
}
// 规则表单卡模型（load 时 parseRules；编辑直接改表单字段，保存时 serializeRule）。
const forms = ref<RuleForm[]>([])

// 时间触发源（cron 收编只读）
const cronJobs = ref<any[]>([])

// ---------------------------------------------------------------------------
// 规则表单模型（2026-11-17 表单化）
// ---------------------------------------------------------------------------

/** watch.kind 放行面（rules.rs 单一真相源的前端镜像；cron 已砍单不在列）。 */
const KINDS = ['fs', 'webhook', 'proc', 'sys', 'probe'] as const

interface RuleForm {
  id: string
  kind: string
  /** 前端不认识的 kind → 整卡降级原文编辑（不渲染假表单、不丢数据）。 */
  isUnknownKind: boolean
  /** unknown kind 的整卡原文（保存原样透传）。 */
  rawJson: string
  // watch 字段（字符串绑定；空串 = 缺省不输出该 Option 字段）。
  /** fs = 监视根；sys+disk = 挂载点定位（同一 WatchSpec.path 字段，按 kind 换语义）。 */
  wPath: string
  wGlob: string
  wDebounce: string
  wContentPattern: string
  wToken: string
  wJobId: string
  wPattern: string
  wNotifyStart: boolean
  /** 原文是否有 notify_start 字段（保真：从未有且未勾 = 不输出）。 */
  wNotifyStartWasPresent: boolean
  wMetric: string
  wAbove: string
  wBelow: string
  wProbeType: string
  wTarget: string
  wInterval: string
  wTimeout: string
  // deliver 字段
  dSessionKey: string
  dPrompt: string
  dCooldown: string
  dMerge: string
  dMaxPerHour: string
  dEnabled: boolean
  // 未知字段保真（保存时原样合并回输出，防静默丢弃）。
  extraWatch: Record<string, unknown>
  extraDeliver: Record<string, unknown>
  extraTop: Record<string, unknown>
}

const WATCH_KEYS = new Set([
  'kind', 'path', 'glob', 'debounce_ms', 'content_pattern', 'token', 'job_id',
  'pattern', 'notify_start', 'metric', 'above', 'below', 'probe_type', 'target',
  'interval_secs', 'timeout_secs',
])
const DELIVER_KEYS = new Set(['session_key', 'prompt', 'cooldown_secs', 'merge_window_secs', 'max_per_hour', 'enabled'])
const TOP_KEYS = new Set(['id', 'watch', 'deliver'])

function newRuleForm(): RuleForm {
  return {
    id: '', kind: 'fs', isUnknownKind: false, rawJson: '',
    wPath: '', wGlob: '', wDebounce: '', wContentPattern: '',
    wToken: '', wJobId: '', wPattern: '', wNotifyStart: false, wNotifyStartWasPresent: false,
    wMetric: '', wAbove: '', wBelow: '',
    wProbeType: '', wTarget: '', wInterval: '', wTimeout: '',
    dSessionKey: '', dPrompt: '', dCooldown: '', dMerge: '', dMaxPerHour: '',
    dEnabled: true,
    extraWatch: {}, extraDeliver: {}, extraTop: {},
  }
}

function asStr(v: any): string {
  if (v == null) return ''
  return typeof v === 'string' ? v : String(v)
}

function objExcept(o: any, known: Set<string>): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  if (o && typeof o === 'object' && !Array.isArray(o)) {
    for (const k of Object.keys(o)) {
      if (!known.has(k)) out[k] = o[k]
    }
  }
  return out
}

/** rules.get 数组 → 表单卡模型。已知字段进表单、剩余收进 extra* 保真。 */
function parseRules(arr: any[]): RuleForm[] {
  return arr.map((r: any) => {
    const f = newRuleForm()
    f.id = typeof r?.id === 'string' ? r.id : ''
    const w = r?.watch ?? {}
    const d = r?.deliver ?? {}
    f.kind = typeof w?.kind === 'string' ? w.kind : ''
    f.isUnknownKind = !(KINDS as readonly string[]).includes(f.kind)
    if (f.isUnknownKind) {
      f.rawJson = JSON.stringify(r, null, 2)
      return f
    }
    f.wPath = asStr(w.path)
    f.wGlob = asStr(w.glob)
    f.wDebounce = asStr(w.debounce_ms)
    f.wContentPattern = asStr(w.content_pattern)
    f.wToken = asStr(w.token)
    f.wJobId = asStr(w.job_id)
    f.wPattern = asStr(w.pattern)
    if (w.notify_start != null) {
      f.wNotifyStart = !!w.notify_start
      f.wNotifyStartWasPresent = true
    }
    f.wMetric = asStr(w.metric)
    f.wAbove = asStr(w.above)
    f.wBelow = asStr(w.below)
    f.wProbeType = asStr(w.probe_type)
    f.wTarget = asStr(w.target)
    f.wInterval = asStr(w.interval_secs)
    f.wTimeout = asStr(w.timeout_secs)
    f.dSessionKey = asStr(d.session_key)
    f.dPrompt = asStr(d.prompt)
    f.dCooldown = asStr(d.cooldown_secs)
    f.dMerge = asStr(d.merge_window_secs)
    f.dMaxPerHour = asStr(d.max_per_hour)
    if (d.enabled != null) f.dEnabled = !!d.enabled
    f.extraWatch = objExcept(w, WATCH_KEYS)
    f.extraDeliver = objExcept(d, DELIVER_KEYS)
    f.extraTop = objExcept(r, TOP_KEYS)
    return f
  })
}

/** 数字字段序列化：空串 = omit（缺省语义）；非法数字返回 null（校验层拦）。 */
function numOrNull(s: string): number | null {
  const t = s.trim()
  if (!t) return null
  const n = Number(t)
  return Number.isFinite(n) ? n : NaN
}

/** 表单卡 → 规则 JSON。返回 { error } 表示该卡无法序列化（校验层已拦前置形态，
 * 这里只兜 unknown kind 原文 parse 失败）。 */
function serializeRule(f: RuleForm): any {
  if (f.isUnknownKind) {
    // parse 失败也原样透传字符串？不行——events.rules.save 要结构化数组。
    // 原文非法时由校验层拦（validateForms 检查 rawJson 可 parse），这里信任前置。
    return JSON.parse(f.rawJson)
  }
  const w: any = { kind: f.kind }
  if (f.kind === 'fs') {
    if (f.wPath.trim()) w.path = f.wPath
    if (f.wGlob.trim()) w.glob = f.wGlob
    const deb = numOrNull(f.wDebounce)
    if (deb != null) w.debounce_ms = deb
    if (f.wContentPattern.trim()) w.content_pattern = f.wContentPattern
  } else if (f.kind === 'webhook') {
    if (f.wToken.trim()) w.token = f.wToken
  } else if (f.kind === 'proc') {
    const job = numOrNull(f.wJobId)
    if (job != null) w.job_id = job
    if (f.wPattern.trim()) w.pattern = f.wPattern
    if (f.wNotifyStart || f.wNotifyStartWasPresent) w.notify_start = f.wNotifyStart
  } else if (f.kind === 'sys') {
    if (f.wMetric.trim()) w.metric = f.wMetric
    const above = numOrNull(f.wAbove)
    if (above != null) w.above = above
    const below = numOrNull(f.wBelow)
    if (below != null) w.below = below
    if (f.wPath.trim()) w.path = f.wPath
  } else if (f.kind === 'probe') {
    if (f.wProbeType.trim()) w.probe_type = f.wProbeType
    if (f.wTarget.trim()) w.target = f.wTarget
    const itv = numOrNull(f.wInterval)
    if (itv != null) w.interval_secs = itv
    const to = numOrNull(f.wTimeout)
    if (to != null) w.timeout_secs = to
  }
  Object.assign(w, f.extraWatch)

  const d: any = {}
  if (f.dSessionKey.trim()) d.session_key = f.dSessionKey
  if (f.dPrompt.trim()) d.prompt = f.dPrompt
  const cd = numOrNull(f.dCooldown)
  if (cd != null) d.cooldown_secs = cd
  const mg = numOrNull(f.dMerge)
  if (mg != null) d.merge_window_secs = mg
  const mph = numOrNull(f.dMaxPerHour)
  if (mph != null) d.max_per_hour = mph
  // enabled 恒输出：缺省语义 = true（omit ≡ true），但用户停用必须显式落盘
  // false——条件输出会把「取消勾选」静默丢回缺省 true。
  d.enabled = f.dEnabled
  Object.assign(d, f.extraDeliver)

  const out: any = { id: f.id.trim(), watch: w, deliver: d }
  Object.assign(out, f.extraTop)
  return out
}

/** probe target 形态校验（rules.rs :326-333 同规则的前端近似）。 */
function probeTargetOk(type: string, target: string): boolean {
  const t = target.trim()
  if (type === 'http') return t.startsWith('http://') || t.startsWith('https://')
  if (type === 'tcp') {
    const i = t.lastIndexOf(':')
    if (i <= 0 || i === t.length - 1) return false
    const port = Number(t.slice(i + 1))
    return Number.isInteger(port) && port > 0 && port <= 65535
  }
  if (type === 'command') return t.length > 0
  return false
}

/** JS 近似正则校验：Rust regex 的内联 flag 组（(?i) / (?is) 等）JS 不认，
 * 剥掉开头的 flag 组再编译——否则 (?i)fatal|panic 这类后端合法规则被前端
 * 误拦。中间的内联组少见，交后端 Rust regex 兜底。 */
function jsRegexOk(pattern: string): boolean {
  const s = pattern.trim().replace(/^\(\?[-a-zA-Z]+\)/, '')
  try {
    new RegExp(s)
    return true
  } catch {
    return false
  }
}

/** 前端校验一层（保存前；与 rules.rs 同规则提前拦明显错误，后端兜底）。
 * 返回错误列表（空 = 通过）。 */
function validateForms(forms: RuleForm[]): string[] {
  const errs: string[] = []
  const seenIds = new Set<string>()
  forms.forEach((f, i) => {
    const tag = `规则#${i + 1}${f.id.trim() ? `（${f.id.trim()}）` : ''}`
    if (f.isUnknownKind) {
      try {
        JSON.parse(f.rawJson)
      } catch {
        errs.push(`${tag}: 原文 JSON 非法`)
      }
      return
    }
    const id = f.id.trim()
    if (!id) errs.push(`${tag}: id 必填`)
    else if (seenIds.has(id)) errs.push(`${tag}: id 重复（${id}）`)
    else seenIds.add(id)
    if (f.kind === 'fs') {
      if (!f.wPath.trim()) errs.push(`${tag}: 监视根 path 必填`)
      const deb = numOrNull(f.wDebounce)
      if (deb != null && (Number.isNaN(deb) || deb < 0)) errs.push(`${tag}: 去重窗口须 ≥0 的毫秒数`)
      if (f.wContentPattern.trim()) {
        if (!jsRegexOk(f.wContentPattern)) errs.push(`${tag}: 新增行正则非法（前端近似校验，后端 Rust regex 兜底）`)
      }
    } else if (f.kind === 'webhook') {
      if (!f.wToken.trim()) errs.push(`${tag}: 调用方凭据 token 必填`)
    } else if (f.kind === 'proc') {
      const job = numOrNull(f.wJobId)
      if (job == null || Number.isNaN(job) || job < 0 || !Number.isInteger(job)) errs.push(`${tag}: 后台任务 id 必填（非负整数，spawn 结果里拿）`)
      if (!f.wPattern.trim()) errs.push(`${tag}: 输出行正则 pattern 必填`)
      else if (!jsRegexOk(f.wPattern)) errs.push(`${tag}: 输出行正则非法（前端近似校验，后端 Rust regex 兜底）`)
    } else if (f.kind === 'sys') {
      if (!['cpu', 'mem', 'disk'].includes(f.wMetric)) errs.push(`${tag}: 指标 metric 必选（cpu|mem|disk）`)
      const above = numOrNull(f.wAbove)
      if (above == null || Number.isNaN(above) || above < 0 || above > 100) errs.push(`${tag}: 触发阈值 above 必填（0-100 使用率%）`)
      const below = numOrNull(f.wBelow)
      if (below != null) {
        if (Number.isNaN(below) || below < 0 || below > 100) errs.push(`${tag}: 回落阈值 below 须在 0-100`)
        else if (above != null && !Number.isNaN(above) && below >= above) errs.push(`${tag}: 回落阈值 below 须低于触发阈值 above（否则滞回永不回 normal）`)
      }
    } else if (f.kind === 'probe') {
      if (!['http', 'tcp', 'command'].includes(f.wProbeType)) errs.push(`${tag}: 探测形态 probe_type 必选（http|tcp|command）`)
      if (!probeTargetOk(f.wProbeType, f.wTarget)) {
        errs.push(
          f.wProbeType === 'http' ? `${tag}: 目标须为完整 URL（http:// 或 https:// 前缀）`
            : f.wProbeType === 'tcp' ? `${tag}: 目标须为 host:port（端口 1-65535）`
            : `${tag}: 目标（命令行）必填`,
        )
      }
      const itv = numOrNull(f.wInterval)
      if (itv != null && (Number.isNaN(itv) || itv < 5)) errs.push(`${tag}: 探测周期最小 5 秒（防把外部端点打成压力测试）`)
      const to = numOrNull(f.wTimeout)
      if (to != null && (Number.isNaN(to) || to <= 0)) errs.push(`${tag}: 探测超时须 >0 秒`)
    }
    const sk = f.dSessionKey.trim()
    if (!sk) errs.push(`${tag}: 目标会话 session_key 必填`)
    else if (!sk.startsWith('agent:') && !sk.startsWith('agents:')) errs.push(`${tag}: session_key 须为 agent:/agents: 预设形态（裸 id 会错路由进默认会话）`)
    if (!f.dPrompt.trim()) errs.push(`${tag}: 注入模板 prompt 必填`)
    for (const [label, raw, min] of [['冷却秒', f.dCooldown, 0], ['合并窗秒', f.dMerge, 0], ['每小时上限', f.dMaxPerHour, 0]] as const) {
      const n = numOrNull(raw)
      if (n != null && (Number.isNaN(n) || n < min)) errs.push(`${tag}: ${label}须 ≥${min} 的数字`)
    }
  })
  return errs
}

// ---------------------------------------------------------------------------
// 数据加载与保存
// ---------------------------------------------------------------------------

let pollTimer: ReturnType<typeof setInterval> | null = null

async function load() {
  try {
    const st = await request('events', 'status')
    available.value = true
    stats.value = st?.stats || {}
    rules.value = st?.rules || []
    pending.value = st?.pending || []
    sysCurrent.value = st?.sys_current || null
    if (!saving.value && !rulesDirty.value) {
      const rd = await request('events', 'rules.get')
      // 复核（BUG-20261122-005 TOCTOU）：rules.get 飞行窗口内用户开始编辑
      // （或进入保存）则放弃覆写——守卫检查在 await 之前，不设防飞行窗口。
      if (!saving.value && !rulesDirty.value) {
        forms.value = parseRules(rd?.rules ?? [])
      }
    }
  } catch {
    // events 泵未装配（feature 裁剪 / 旧网关）→ 诚实降级
    available.value = false
  }
  try {
    const cl = await request('tasks', 'cron.list')
    cronJobs.value = cl?.jobs || cl?.items || []
  } catch { /* cron 服务缺席不阻塞本页 */ }
  loading.value = false
}

async function saveRules() {
  saveError.value = ''
  const errs = validateForms(forms.value)
  if (errs.length > 0) {
    saveError.value = errs.join('；')
    return
  }
  let serialized: any[]
  try {
    serialized = forms.value.map(serializeRule)
  } catch {
    saveError.value = '规则原文 JSON 非法'
    return
  }
  saving.value = true
  const seqAtSave = editSeq
  try {
    const res = await request('events', 'rules.save', { rules: serialized })
    const warns: string[] = res?.warnings || []
    const newEdits = editSeq !== seqAtSave
    if (newEdits) {
      // 保存飞行窗口内又有新编辑（BUG-20261122-005 同族）：保存的是进入时
      // 快照，新编辑仍未落盘——保留 dirty（轮询继续避让表单），提示再存。
      rulesDirty.value = true
    } else {
      rulesDirty.value = false
    }
    if (warns.length > 0) {
      toast.warn('已保存，校验提示：' + warns.join('；'))
    } else if (newEdits) {
      toast.warn('已保存，但保存期间又有新的本地修改——请再点一次「保存规则」')
    } else {
      toast.success('规则已保存（泵 ≤250ms 热重载）')
    }
    await load()
  } catch (e: any) {
    // 后端校验拒绝——错误原文回显保存钮旁（不弹窗），文件未动。
    saveError.value = String(e)
  } finally {
    saving.value = false
  }
}

/** 总览表快捷启停：**按 id 定位**编辑卡（总览行 idx 是 events.status 投影数组的
 * 下标，与 forms 数组仅在「无本地未保存结构改动」时同序——用户删/增卡后按
 * 下标取会错位启停别的卡并连带保存本地改动）。找不到 = 本地已删/未保存，
 * 诚实提示不猜。 */
async function toggleRule(ruleId: string) {
  const i = forms.value.findIndex(x => x.id.trim() === ruleId)
  if (i < 0) {
    toast.warn(`规则 ${ruleId} 在编辑区已不存在（本地改动未保存）——请先保存或刷新`)
    return
  }
  const f = forms.value[i]
  f.dEnabled = !f.dEnabled
  markDirty()
  await saveRules()
}

function addRule() {
  const f = newRuleForm()
  // 建议 id：rule-N 顺延，撞已占用则继续顺延（用户可改）。
  let n = forms.value.length + 1
  const used = new Set(forms.value.map(x => x.id.trim()))
  while (used.has(`rule-${n}`)) n++
  f.id = `rule-${n}`
  forms.value.push(f)
  rulesDirty.value = true
}

function removeRule(i: number) {
  forms.value.splice(i, 1)
  rulesDirty.value = true
}

// ---------------------------------------------------------------------------
// 会话 picker（deliver.session_key 单选；复用 HookView picker 形态，落盘
// `agent:main:session:<sid>` 预设形态；agents: 群组形态走手输）。
// ---------------------------------------------------------------------------

const pickerIdx = ref<number | null>(null)
const pickerSearch = ref('')
// 弹层坐标（Teleport + position:fixed）：absolute 原版挂在卡片内行上，
// 弹层伸出卡片下缘被 .card{overflow:hidden} 裁掉（同 HookView picker，
// 2026-11-18 用户实测形态）。
let pickerAnchor: HTMLElement | null = null
const pickerPos = ref({ top: 0, left: 0 })
const pickerRoot = ref<HTMLElement | null>(null)

async function openPicker(i: number, e: MouseEvent) {
  pickerIdx.value = i
  pickerSearch.value = ''
  pickerAnchor = e.currentTarget as HTMLElement
  placePicker()
  await sessionStore.fetchList()
  void sessionStore.fetchProjects()
}

/** fixed 定位收口：锚按钮下方展开；下方放不下且上方更宽裕则向上翻；
 * 上下都放不下时钳到视口内（列表自带 max-height 滚动兜底）。 */
function placePicker() {
  if (!pickerAnchor) return
  const r = pickerAnchor.getBoundingClientRect()
  pickerPos.value = {
    top: r.bottom + 6,
    left: Math.max(8, Math.min(r.left, window.innerWidth - 428)),
  }
}

watch(pickerIdx, async (v) => {
  if (v == null) return
  await nextTick()
  const el = pickerRoot.value
  if (!el || !pickerAnchor) return
  const h = el.offsetHeight
  const r = pickerAnchor.getBoundingClientRect()
  let top = r.bottom + 6
  if (top + h > window.innerHeight - 8 && r.top - h - 6 > 8) {
    top = Math.max(8, r.top - h - 6)
  }
  pickerPos.value = {
    top: Math.max(8, Math.min(top, Math.max(8, window.innerHeight - h - 8))),
    left: Math.max(8, Math.min(pickerPos.value.left, window.innerWidth - el.offsetWidth - 8)),
  }
})

function pickSid(sid: string) {
  if (pickerIdx.value == null) return
  forms.value[pickerIdx.value].dSessionKey = `agent:main:session:${sid}`
  // 程序化改字段不触发 DOM input 事件，卡片容器的 @input/@change 抓不到——
  // 必须手动置脏，否则 5s 轮询覆盖 forms 会把所选会话静默冲掉。
  markDirty()
  pickerIdx.value = null
}

/** picker 候选：标题 / sid / 首条消息按搜索串过滤。 */
const pickerRows = computed(() => {
  const q = pickerSearch.value.trim().toLowerCase()
  const all = sessionStore.sessions
  if (!q) return all
  return all.filter(
    s =>
      (s.title ?? '').toLowerCase().includes(q) ||
      (s.firstMessage ?? '').toLowerCase().includes(q) ||
      s.id.toLowerCase().includes(q),
  )
})

/** sid → 项目徽标文案（项目名；注册表缺席回退 📁，无项目 null）。 */
function projectBadgeOf(sid: string): string | null {
  const pid = sessionStore.sessions.find(s => s.id === sid)?.projectId
  if (!pid) return null
  return sessionStore.projectNameOf(pid) ?? '📁'
}

/** 当前 session_key 是否指向某会话（picker 高亮用）：`agent:main:session:<sid>` 形态。 */
function isPickedSid(sid: string): boolean {
  if (pickerIdx.value == null) return false
  return forms.value[pickerIdx.value].dSessionKey.trim() === `agent:main:session:${sid}`
}

// ---------------------------------------------------------------------------
// 只读总览辅助（泵统计）
// ---------------------------------------------------------------------------

function fmtNum(n: any): string {
  return typeof n === 'number' ? String(n) : '0'
}

function fmtPct(v: any): string {
  return typeof v === 'number' ? v.toFixed(0) : '-'
}

// 磁盘当前使用率展示口径：最高盘（与 sys disk 阈值判定的 None 臂同源语义）。
const maxDiskPercent = computed<number | null>(() => {
  const disks = sysCurrent.value?.disks
  if (!Array.isArray(disks) || disks.length === 0) return null
  return Math.max(...disks.map((d: any) => (typeof d?.used_percent === 'number' ? d.used_percent : 0)))
})

const STAT_CARDS = [
  { key: 'delivered_messages', label: '投递消息', cls: 'badge-success' },
  { key: 'delivered_events', label: '投递事件', cls: 'badge-success' },
  { key: 'webhook_received', label: 'Webhook 收到', cls: 'badge-info' },
  { key: 'webhook_unmatched', label: 'Webhook 未认领', cls: 'badge-neutral' },
  { key: 'proc_lines_matched', label: '进程行命中', cls: 'badge-info' },
  { key: 'proc_read_errors', label: '进程读失败', cls: 'badge-error' },
  { key: 'sys_triggered', label: '系统阈值触发', cls: 'badge-info' },
  { key: 'probe_flips', label: '探测翻转', cls: 'badge-info' },
  { key: 'deduped', label: '防抖折叠', cls: 'badge-info' },
  { key: 'dropped_cooldown', label: '冷却丢弃', cls: 'badge-warning' },
  { key: 'dropped_hourly', label: '小时窗丢弃', cls: 'badge-warning' },
  { key: 'dropped_estop', label: '急停丢弃', cls: 'badge-error' },
  { key: 'dropped_delivery', label: '投递失败', cls: 'badge-error' },
  { key: 'dropped_reload', label: '重载丢弃', cls: 'badge-neutral' },
  { key: 'reloads', label: '规则重载', cls: 'badge-info' },
] as const

onMounted(() => {
  load()
  void sessionStore.fetchList()
  pollTimer = setInterval(load, 5000)
})
onUnmounted(() => {
  if (pollTimer) clearInterval(pollTimer)
})
</script>

<template>
  <div class="page-events">
    <div class="page-header">
      <h2>事件</h2>
      <div class="page-header-actions">
        <button class="btn" data-testid="refresh-btn" @click="load">刷新</button>
      </div>
    </div>

    <div class="page-body">
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'overview' }" data-testid="events-tab-overview" @click="activeTab = 'overview'">总览</button>
        <button class="tab" :class="{ active: activeTab === 'rules' }" data-testid="events-tab-rules" @click="activeTab = 'rules'">规则设置</button>
      </div>

      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <template v-else-if="!available">
        <div class="card">
          <div class="card-body">
            <p class="empty-state" data-testid="events-unavailable" style="margin: 0;">
              事件泵未装配（events feature 未编译或网关版本较旧）——规则与统计面不可用。
            </p>
          </div>
        </div>
      </template>

      <!-- ===================== 总览（信息展示） ===================== -->
      <template v-else-if="activeTab === 'overview'">
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-body">
            <p style="font-size: var(--text-sm); color: var(--text-secondary); margin: 0;">
              出事主动叫人：多源事件按规则唤醒指定会话（<code>workspace/config/events.json</code> 热重载）。
              事件源：fs 文件系统（可选 <code>content_pattern</code> 新增行正则过滤）、webhook 外部 POST 推送（token 即凭据）、
              proc 后台任务输出行正则、sys 系统指标（CPU/内存/磁盘阈值滞回）、probe 外部端点存活探测（http/tcp/command 状态翻转触发）。
              三闸语义（冷却/小时窗/estop 冻结）由后端统一执行，页面只展示。
            </p>
          </div>
        </div>

        <!-- 泵统计 -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>泵统计</h3>
            <span style="font-size: var(--text-sm); color: var(--text-secondary);" data-testid="pending-summary">
              待投 buffer：{{ pending.length }} 条规则
            </span>
            <span
              v-if="sysCurrent"
              data-testid="sys-current"
              style="font-size: var(--text-sm); color: var(--text-secondary); margin-left: var(--space-3);"
            >
              sys 当前：CPU {{ fmtPct(sysCurrent.cpu) }}% · 内存 {{ fmtPct(sysCurrent.mem_percent) }}% · 磁盘最高 {{ fmtPct(maxDiskPercent) }}%
            </span>
          </div>
          <div class="card-body">
            <div style="display: flex; flex-wrap: wrap; gap: var(--space-2);">
              <span v-for="c in STAT_CARDS" :key="c.key" class="badge" :class="c.cls" :data-testid="'stat-' + c.key">
                {{ c.label }} {{ fmtNum(stats[c.key]) }}
              </span>
            </div>
            <div v-if="pending.length > 0" style="margin-top: var(--space-2); font-size: var(--text-xs); color: var(--text-muted);">
              <span v-for="p in pending" :key="p.rule_id" style="margin-right: var(--space-3);">
                {{ p.rule_id }}: 攒 {{ p.count }} 个事件（静默窗中）
              </span>
            </div>
          </div>
        </div>

        <!-- 规则表（只读总览） -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>事件规则</h3>
            <span style="font-size: var(--text-sm); color: var(--text-secondary);">{{ rules.length }} 条</span>
          </div>
          <div class="card-body">
            <div v-if="rules.length === 0" class="empty-state" data-testid="rules-empty">
              <p>暂无规则——切到「规则设置」TAB 新增</p>
            </div>
            <div v-else class="table-wrap">
              <table>
                <thead>
                  <tr><th>ID</th><th>监视</th><th>目标会话</th><th>三闸</th><th>状态</th><th>操作</th></tr>
                </thead>
                <tbody>
                  <tr v-for="(r, idx) in rules" :key="r.id" :data-testid="'rule-row-' + idx">
                    <td style="font-family: monospace;">{{ r.id }}</td>
                    <td style="max-width: 320px; font-family: monospace; font-size: var(--text-xs); word-break: break-all;">
                      <template v-if="r.detail">{{ r.detail }}</template>
                      <template v-else>{{ r.kind }}: {{ r.path }}<template v-if="r.glob"> ({{ r.glob }})</template></template>
                    </td>
                    <td style="max-width: 260px; font-family: monospace; font-size: var(--text-xs); word-break: break-all;">{{ r.session_key }}</td>
                    <td style="font-size: var(--text-xs); color: var(--text-secondary); white-space: nowrap;">
                      冷却 {{ r.cooldown_secs }}s · 合并 {{ r.merge_window_secs }}s · ≤{{ r.max_per_hour === 0 ? '∞' : r.max_per_hour }}/h
                    </td>
                    <td>
                      <span class="badge" :class="r.enabled ? 'badge-success' : 'badge-neutral'" :data-testid="'rule-status-' + idx">
                        {{ r.enabled ? '启用' : '停用' }}
                      </span>
                    </td>
                    <td style="white-space: nowrap;">
                      <button class="btn" style="font-size: var(--text-xs);" :data-testid="'toggle-' + idx" @click="toggleRule(r.id)">
                        {{ r.enabled ? '停用' : '启用' }}
                      </button>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- 收编：时间触发源（只读） -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>时间触发源（cron，只读）</h3>
            <span style="font-size: var(--text-sm); color: var(--text-secondary);">管理走 <router-link to="/tasks">任务页</router-link></span>
          </div>
          <div class="card-body">
            <div v-if="cronJobs.length === 0" class="empty-state"><p>暂无 cron 作业</p></div>
            <div v-else class="table-wrap">
              <table>
                <thead><tr><th>名称</th><th>排程</th><th>状态</th></tr></thead>
                <tbody>
                  <tr v-for="(j, idx) in cronJobs" :key="j.id || idx">
                    <td>{{ j.name || j.id }}</td>
                    <td style="font-family: monospace;">{{ j.cron || j.schedule || '-' }}</td>
                    <td><span class="badge" :class="j.enabled === false ? 'badge-neutral' : 'badge-success'">{{ j.enabled === false ? '停用' : '启用' }}</span></td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </template>

      <!-- ===================== 规则设置（表单编辑） ===================== -->
      <template v-else>
        <!-- 规则编辑（表单卡列表） -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>规则编辑（events.json）</h3>
            <div style="display: flex; gap: var(--space-2); align-items: center;">
              <button class="btn" data-testid="add-rule-btn" @click="addRule">+ 新增规则</button>
              <button class="btn btn-primary" data-testid="save-rules-btn" :disabled="saving" @click="saveRules">保存规则</button>
            </div>
          </div>
          <div class="card-body">
            <div
              v-if="saveError"
              class="ev-error-banner"
              data-testid="rules-error"
            >{{ saveError }}</div>
            <p style="font-size: var(--text-xs); color: var(--text-muted); margin: 0 0 var(--space-2);">
              模板占位符按源：<code v-pre>{count}</code> 合并计数恒可用；fs → <code v-pre>{paths}</code>/<code v-pre>{path}</code>；
              webhook / sys / probe → <code v-pre>{payload}</code>（webhook 载荷 ≤1KB、sys 为触发语句、probe 为翻转语句）；proc → <code v-pre>{line}</code>/<code v-pre>{job_id}</code>。
              数字字段留空 = 用后端缺省（冷却 60s / 合并窗 5s / 每小时 10 条 / 去重 500ms / 探测周期 60s / 探测超时 10s）。
            </p>
            <div v-if="forms.length === 0" class="empty-state" data-testid="editor-empty">
              <p>暂无规则——点「+ 新增规则」创建</p>
            </div>
            <div
              v-for="(f, i) in forms"
              :key="i"
              class="ev-rule-card"
              :data-testid="'rule-card-' + i"
              @input="markDirty"
              @change="markDirty"
            >
              <div class="ev-rule-head">
                <input
                  v-model="f.id"
                  class="form-input ev-id-input"
                  placeholder="规则 id（唯一）"
                  :data-testid="'rule-id-' + i"
                />
                <select v-if="!f.isUnknownKind" v-model="f.kind" class="form-input ev-kind-select" :data-testid="'rule-kind-' + i">
                  <option v-for="k in KINDS" :key="k" :value="k">{{ k }}</option>
                </select>
                <span v-else class="badge badge-warning">未知 kind：{{ f.kind || '?' }}</span>
                <label class="ev-enabled-label">
                  <input v-model="f.dEnabled" type="checkbox" :data-testid="'rule-enabled-' + i" />
                  启用
                </label>
                <button class="btn ev-del-btn" :data-testid="'rule-del-' + i" title="删除规则" @click="removeRule(i)">✕</button>
              </div>

              <!-- 未知 kind：降级原文编辑（不渲染假表单、不丢数据） -->
              <template v-if="f.isUnknownKind">
                <div class="ev-unknown-banner" data-testid="unknown-kind-banner">
                  ⚠ 前端不认识 kind「{{ f.kind || '?' }}」——本卡降级为原文编辑，保存原样透传（后端校验兜底；字段不会丢失）。
                </div>
                <textarea
                  v-model="f.rawJson"
                  class="ev-raw-editor"
                  spellcheck="false"
                  :data-testid="'unknown-rule-editor-' + i"
                ></textarea>
              </template>

              <!-- watch 段：按 kind 动态字段组 -->
              <template v-else>
                <div class="ev-section-title">监视源（watch）</div>
                <!-- fs -->
                <template v-if="f.kind === 'fs'">
                  <div class="ev-field">
                    <label>监视根 <span class="ev-req">*</span></label>
                    <input v-model="f.wPath" class="form-input" style="font-family: var(--font-mono);" placeholder="目录或文件，如 C:/logs/build" :data-testid="'fs-path-' + i" />
                  </div>
                  <div class="ev-field">
                    <label>文件名通配</label>
                    <input v-model="f.wGlob" class="form-input" style="font-family: var(--font-mono);" placeholder="*.log（留空 = 全收）" />
                  </div>
                  <div class="ev-field">
                    <label>去重窗口(ms)</label>
                    <input v-model="f.wDebounce" class="form-input" placeholder="留空 = 500" />
                  </div>
                  <div class="ev-field">
                    <label>新增行正则</label>
                    <input v-model="f.wContentPattern" class="form-input" style="font-family: var(--font-mono);" placeholder="如 (?i)fatal|panic——命中才触发（留空 = 变化即触发）" />
                  </div>
                </template>
                <!-- webhook -->
                <template v-else-if="f.kind === 'webhook'">
                  <div class="ev-field">
                    <label>调用方凭据 <span class="ev-req">*</span></label>
                    <input v-model="f.wToken" class="form-input" style="font-family: var(--font-mono);" placeholder="POST /api/events/webhook/&lt;token&gt; 的 token" />
                  </div>
                </template>
                <!-- proc -->
                <template v-else-if="f.kind === 'proc'">
                  <div class="ev-field">
                    <label>后台任务 id <span class="ev-req">*</span></label>
                    <input v-model="f.wJobId" class="form-input" placeholder="background_registry 坐标（spawn 结果里拿）" />
                  </div>
                  <div class="ev-field">
                    <label>输出行正则 <span class="ev-req">*</span></label>
                    <input v-model="f.wPattern" class="form-input" style="font-family: var(--font-mono);" placeholder="如 (?i)error|FAILED" />
                  </div>
                  <div class="ev-field">
                    <label>启动通知</label>
                    <label class="ev-enabled-label"><input v-model="f.wNotifyStart" type="checkbox" />任务运行中时补一条启动通知</label>
                  </div>
                </template>
                <!-- sys -->
                <template v-else-if="f.kind === 'sys'">
                  <div class="ev-field">
                    <label>指标 <span class="ev-req">*</span></label>
                    <select v-model="f.wMetric" class="form-input ev-kind-select">
                      <option value="" disabled>选择指标…</option>
                      <option value="cpu">cpu（CPU 使用率%）</option>
                      <option value="mem">mem（内存使用率%）</option>
                      <option value="disk">disk（磁盘使用率%，任一盘）</option>
                    </select>
                  </div>
                  <div class="ev-field">
                    <label>触发阈值 <span class="ev-req">*</span></label>
                    <input v-model="f.wAbove" class="form-input" placeholder="0-100 使用率%，如 90" />
                  </div>
                  <div class="ev-field">
                    <label>回落阈值</label>
                    <input v-model="f.wBelow" class="form-input" placeholder="留空 = 触发阈值 -10（滞回回落）" />
                  </div>
                  <div class="ev-field">
                    <label>挂载点定位</label>
                    <input v-model="f.wPath" class="form-input" style="font-family: var(--font-mono);" placeholder="disk 时可选：任意路径定位挂载点（留空 = 任一盘）" />
                  </div>
                </template>
                <!-- probe -->
                <template v-else-if="f.kind === 'probe'">
                  <div class="ev-field">
                    <label>探测形态 <span class="ev-req">*</span></label>
                    <select v-model="f.wProbeType" class="form-input ev-kind-select">
                      <option value="" disabled>选择形态…</option>
                      <option value="http">http（2xx = up）</option>
                      <option value="tcp">tcp（连接成功 = up）</option>
                      <option value="command">command（退出码 0 = up）</option>
                    </select>
                  </div>
                  <div class="ev-field">
                    <label>目标 <span class="ev-req">*</span></label>
                    <input v-model="f.wTarget" class="form-input" style="font-family: var(--font-mono);" placeholder="http=完整 URL / tcp=host:port / command=命令行" />
                  </div>
                  <div class="ev-field">
                    <label>探测周期(秒)</label>
                    <input v-model="f.wInterval" class="form-input" placeholder="留空 = 60（最小 5）" />
                  </div>
                  <div class="ev-field">
                    <label>探测超时(秒)</label>
                    <input v-model="f.wTimeout" class="form-input" placeholder="留空 = 10" />
                  </div>
                </template>

                <div class="ev-section-title">投递（deliver）</div>
                <div class="ev-field">
                  <label>目标会话 <span class="ev-req">*</span></label>
                  <div class="ev-session-row">
                    <input
                      v-model="f.dSessionKey"
                      class="form-input"
                      style="font-family: var(--font-mono); flex: 1;"
                      placeholder="agent:main:session:<会话id>（agent:/agents: 形态）"
                      :data-testid="'session-key-' + i"
                    />
                    <button class="btn" :data-testid="'pick-session-' + i" @click="openPicker(i, $event)">选会话 ▾</button>
                  </div>
                </div>
                <div class="ev-field">
                  <label>注入模板 <span class="ev-req">*</span></label>
                  <textarea
                    v-model="f.dPrompt"
                    class="form-input ev-prompt-editor"
                    spellcheck="false"
                    placeholder="唤醒会话时注入的提示词，可用占位符（见顶部说明）"
                  ></textarea>
                </div>
                <div class="ev-field-row">
                  <div class="ev-field ev-field--num">
                    <label>冷却(秒)</label>
                    <input v-model="f.dCooldown" class="form-input" placeholder="60（0=关）" />
                  </div>
                  <div class="ev-field ev-field--num">
                    <label>合并窗(秒)</label>
                    <input v-model="f.dMerge" class="form-input" placeholder="5（0=立即）" />
                  </div>
                  <div class="ev-field ev-field--num">
                    <label>每小时上限</label>
                    <input v-model="f.dMaxPerHour" class="form-input" placeholder="10（0=不限）" />
                  </div>
                </div>
              </template>
            </div>
            <!-- 会话 picker（BUG-20261122-004）：单实例，提升在规则卡 v-for
                 之外——template ref 在 v-for 作用域内会被 Vue 3 数组化，watch
                 定位算式读 offsetHeight 得 undefined → pickerPos 全 NaN →
                 向上翻转/视口钳制静默死码（锚点近视口底部时弹层 footer 不可
                 达）。picker 内容只依赖 pickerIdx/pickerRows，不引用卡内局部
                 变量，可安全提升。 -->
            <Teleport to="body">
              <div
                v-if="pickerIdx !== null"
                ref="pickerRoot"
                class="ev-picker"
                :style="{ top: pickerPos.top + 'px', left: pickerPos.left + 'px' }"
              >
                <input class="form-input" style="width: 100%;" v-model="pickerSearch" placeholder="按标题 / sid 搜索会话…" />
                <div class="ev-picker-list">
                  <div
                    v-for="s in pickerRows"
                    :key="s.id"
                    class="ev-picker-item"
                    :class="{ 'ev-picker-item--picked': isPickedSid(s.id) }"
                    @click="pickSid(s.id)"
                  >
                    <span class="ev-picker-item-title">{{ s.title || s.id }}</span>
                    <span v-if="s.projectId" class="ev-picker-item-badge">{{ projectBadgeOf(s.id) ?? '📁' }}</span>
                    <span class="ev-picker-item-sid">{{ s.id }}</span>
                  </div>
                  <div v-if="!pickerRows.length" style="padding: var(--space-3); color: var(--text-muted); font-size: var(--text-xs);">无匹配会话</div>
                </div>
                <div class="ev-picker-footer">
                  <span style="color: var(--text-muted); font-size: var(--text-xs);">单选——点击行即选定；群组形态（agents:）走手输</span>
                  <button class="btn btn-sm" @click="pickerIdx = null">关闭</button>
                </div>
              </div>
            </Teleport>
          </div>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
/* 保存错误横幅（校验失败 / 后端拒绝原文，回显保存钮下方） */
.ev-error-banner {
  padding: var(--space-2) var(--space-3);
  margin-bottom: var(--space-3);
  border-radius: var(--radius-md);
  font-size: var(--text-sm);
  background: var(--danger-bg, #fdecea);
  border: 1px solid var(--danger, #dc3545);
  white-space: pre-wrap;
  word-break: break-all;
}

/* 规则卡 */
.ev-rule-card {
  border: 1px solid var(--border-light);
  border-radius: var(--radius-md);
  padding: var(--space-3);
  margin-bottom: var(--space-3);
  background: var(--bg-secondary);
}
.ev-rule-head {
  display: flex;
  gap: var(--space-2);
  align-items: center;
  margin-bottom: var(--space-2);
}
.ev-id-input {
  width: 200px;
  flex-shrink: 0;
  font-family: var(--font-mono);
}
.ev-kind-select {
  width: 150px;
  flex-shrink: 0;
}
.ev-enabled-label {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: var(--text-sm);
  color: var(--text-secondary);
  white-space: nowrap;
  cursor: pointer;
}
.ev-del-btn {
  margin-left: auto;
  color: var(--danger);
  flex-shrink: 0;
}

.ev-section-title {
  font-weight: 600;
  font-size: var(--text-xs);
  color: var(--text-secondary);
  margin: var(--space-2) 0 var(--space-1);
}

.ev-field {
  display: flex;
  gap: var(--space-2);
  align-items: center;
  margin-bottom: var(--space-1);
}
.ev-field > label {
  width: 110px;
  flex-shrink: 0;
  font-size: var(--text-xs);
  color: var(--text-secondary);
  text-align: right;
}
.ev-field > .form-input {
  flex: 1;
  min-width: 0;
}
.ev-field-row {
  display: flex;
  gap: var(--space-3);
}
.ev-field--num {
  flex: 1;
}
.ev-req {
  color: var(--danger);
}

.ev-prompt-editor {
  min-height: 60px;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  resize: vertical;
}

/* 未知 kind 降级 */
.ev-unknown-banner {
  padding: var(--space-2) var(--space-3);
  border-radius: var(--radius-md);
  font-size: var(--text-xs);
  background: rgba(255, 193, 7, 0.12);
  border: 1px solid rgba(255, 193, 7, 0.45);
  margin-bottom: var(--space-2);
}
.ev-raw-editor {
  width: 100%;
  min-height: 140px;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  resize: vertical;
}

/* 会话 picker（deliver.session_key 单选） */
.ev-session-row {
  display: flex;
  gap: var(--space-2);
  align-items: center;
  flex: 1;
  position: relative;
  min-width: 0;
}
.ev-picker {
  /* Teleport + position:fixed（挂 body）：absolute 原版伸出卡片下缘被
     .card{overflow:hidden} 裁掉（同 HookView picker，2026-11-18 用户实测）。 */
  position: fixed;
  z-index: var(--z-overlay);
  width: min(420px, 92vw);
  max-height: calc(100vh - 16px);
  overflow-y: auto;
  background: var(--surface);
  border: 1px solid var(--border-light);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  padding: var(--space-2);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.ev-picker-list {
  max-height: min(260px, calc(100vh - 160px));
  overflow-y: auto;
  display: flex;
  flex-direction: column;
}
.ev-picker-item {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: 4px 6px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: var(--text-xs);
}
.ev-picker-item:hover { background: var(--surface-hover); }
.ev-picker-item--picked {
  background: var(--accent-muted, #e8f0fe);
  outline: 1px solid var(--accent);
}
.ev-picker-item-title {
  flex-shrink: 0;
  max-width: 55%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 500;
}
.ev-picker-item-badge {
  flex-shrink: 0;
  color: var(--text-muted);
}
.ev-picker-item-sid {
  flex: 1;
  color: var(--text-muted);
  font-family: var(--font-mono);
  font-size: 10px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  text-align: right;
}
.ev-picker-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
  border-top: 1px solid var(--border-light);
  padding-top: var(--space-2);
}
</style>
