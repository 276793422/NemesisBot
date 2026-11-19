<script setup lang="ts">
import { ref, watch, computed, onMounted, nextTick } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { useSessionStore } from '../stores/session'

// Hooks 钩子页（2026-08-29 自设置页迁移 + 双 TAB 重构）：
// - 总览：统计徽标 + 每事件只读明细 + 可编辑原文 + 方言事件文档表
// - 设置：扁平结构化编辑（每条钩子 = 执行体类型[command/prompt，仅 Stop
//   可选提示词型，2026-11-19 统一存储] + 触发工具[可空] + 命令/提示词 +
//   超时 + 作用会话[可选，2026-11-16 会话圈定]），保存时每条各自成组写入
//   hooks.json（方案 B，语义与分组版等价）。
// 单一真相源 = hooks.json（command/prompt 两型统一存储；agent hooks 工具
// 写的会话圈定组也在此文件）；两处编辑都走 hooks.set（后端语义校验兜底），
// 切换 TAB 即从磁盘刷新（最后写入者胜，用户已接受覆盖冲突）。

const { request } = useWSAPI()
const toast = useToast()
// 会话圈定（HOOK 会话维度，2026-11-16）：选择器数据源复用全局 session
// store（侧栏同源，5s 缓存内不重复拉取）；按标题展示、按裸 sid 落盘。
const sessionStore = useSessionStore()

const EVENTS = [
  { id: 'PreToolUse', label: 'PreToolUse', hint: '工具执行前（安全 8 层闸之后）——可拦截工具调用' },
  { id: 'PostToolUse', label: 'PostToolUse', hint: '工具执行后——stderr 可追加反馈给模型（不撤销操作）' },
  { id: 'PostToolUseFailure', label: 'PostToolUseFailure', hint: '工具执行失败后（观察型：stderr 只记日志）' },
  { id: 'SessionStart', label: 'SessionStart', hint: '每会话首条 prompt 到达时（观察型，不阻断）' },
  { id: 'UserPromptSubmit', label: 'UserPromptSubmit', hint: '用户消息进入历史之前——可拦下 prompt' },
  { id: 'Stop', label: 'Stop', hint: '最终答案被接受后——可阻止收尾再答一轮（封顶 2 次）；提示词型钩子（LLM 执行体）在此钩点注入提示词开新轮（无封顶）' },
  { id: 'SessionEnd', label: 'SessionEnd', hint: '会话被清理/删除时（观察型，无阻断语义）' },
  { id: 'PreCompact', label: 'PreCompact', hint: '上下文压缩前（观察型：不阻止压缩）' },
  { id: 'PostCompact', label: 'PostCompact', hint: '上下文压缩后（观察型）' },
  { id: 'SubagentStart', label: 'SubagentStart', hint: '子代理启动前（本地 spawn）——exit 2 可拒绝 spawn' },
  { id: 'SubagentStop', label: 'SubagentStop', hint: '子代理任务完成后（观察型；任务已跑完，无阻断语义）' },
  { id: 'Notification', label: 'Notification', hint: '审批/提问边界通知（纯观察型：提问发起、安全审批发起）' },
] as const

interface HookEntry {
  /** 执行体类型（2026-11-19 统一存储二型）：command=子进程脚本；
   * prompt=提示词注入（仅 Stop 事件合法）。 */
  type: 'command' | 'prompt'
  /** 触发工具的 matcher（工具名正则/子串；空 = 全部工具）。 */
  matcher: string
  command: string
  /** prompt 型执行体文本（type='prompt' 时非空）。 */
  prompt: string
  /** 秒；空串 = 后端默认 60。输入框绑定用字符串。 */
  timeout: string
  /** 会话圈定（2026-11-16）：裸 sid 列表；空 = 全部会话（缺省兼容）。 */
  sessions: string[]
}

function emptyEntries(): Record<string, HookEntry[]> {
  const out: Record<string, HookEntry[]> = {}
  for (const ev of EVENTS) out[ev.id] = []
  return out
}

const activeTab = ref<'overview' | 'editor'>('overview')
const rawJson = ref('')
const hooksExists = ref(false)
const hooksValid = ref(true)
const hooksError = ref<string | null>(null)
// 冷启动（复核三修 2026-09-24）：Agent 进程启动时无桥可热更——hooks.json
// 缺失 / 解析失败 / 零脚本任一形态，load_from_dir 都不建桥（cc_hooks.rs
// 既有语义）。此后首次保存只落盘，需重启一次才进热更态。取页面加载时
// 的文件状态作代理：保存不改它（无桥会话内保存永远无桥）。
const coldStart = ref(false)
const entries = ref<Record<string, HookEntry[]>>(emptyEntries())
const rawSaving = ref(false)
const editorSaving = ref(false)

const totalCount = computed(() =>
  EVENTS.reduce((n, ev) => n + entries.value[ev.id].length, 0),
)

/** 解析 hooks.json 原文为扁平条目。兼容 {"hooks":{...}} 与裸顶层两种形态；
 * 解析失败（非法 JSON）→ 空条目（总览的错误横幅会说明，由用户改原文）。 */
function parseEntries(content: string): Record<string, HookEntry[]> {
  const out = emptyEntries()
  if (!content.trim()) return out
  let data: any
  try {
    data = JSON.parse(content)
  } catch {
    return out
  }
  const hooks = data?.hooks ?? data ?? {}
  for (const ev of EVENTS) {
    const groups = Array.isArray(hooks[ev.id]) ? hooks[ev.id] : []
    out[ev.id] = groups.flatMap((g: any) =>
      (Array.isArray(g?.hooks) ? g.hooks : []).map((c: any) => ({
        // 二型（统一存储）：type 缺省/未知按内容兜底回 command（后端
        // parse_cc_hooks 是 loud 拒绝，这里宽容展示由保存时后端把关）。
        type: c?.type === 'prompt' ? 'prompt' : 'command',
        matcher: typeof g?.matcher === 'string' ? g.matcher : '',
        command: typeof c?.command === 'string' ? c.command : '',
        prompt: typeof c?.prompt === 'string' ? c.prompt : '',
        timeout: c?.timeout != null ? String(c.timeout) : '',
        sessions: Array.isArray(g?.scope?.sessions)
          ? g.scope.sessions.filter((s: any) => typeof s === 'string' && s.trim())
          : [],
      })),
    )
  }
  return out
}

/** 扁平条目 → hooks.json（方案 B：每条钩子各自成组；空事件不输出）。
 * 二型统一存储：type 恒输出；prompt 型不带 command/timeout。 */
function serializeEntries(): string {
  const hooks: Record<string, any> = {}
  for (const ev of EVENTS) {
    const list = entries.value[ev.id].filter(e =>
      e.type === 'prompt' ? e.prompt.trim() : e.command.trim(),
    )
    if (!list.length) continue
    hooks[ev.id] = list.map(e => {
      const cmd: any =
        e.type === 'prompt'
          ? { type: 'prompt', prompt: e.prompt.trim() }
          : { type: 'command', command: e.command.trim() }
      const t = Number(e.timeout)
      if (e.type === 'command' && e.timeout.trim() && Number.isFinite(t) && t > 0) cmd.timeout = t
      const group: any = { hooks: [cmd] }
      if (e.matcher.trim()) group.matcher = e.matcher.trim()
      const ss = e.sessions.map(s => s.trim()).filter(Boolean)
      if (ss.length) group.scope = { sessions: ss }
      return group
    })
  }
  return JSON.stringify({ hooks }, null, 2)
}

async function loadHooks() {
  try {
    const data = await request('hooks', 'get')
    rawJson.value = data?.content ?? ''
    hooksExists.value = !!data?.exists
    hooksValid.value = data?.valid !== false
    hooksError.value = data?.error || null
    coldStart.value =
      !data?.exists || data?.valid === false || (data?.summary?.total ?? 0) === 0
    entries.value = parseEntries(rawJson.value)
  } catch (e: any) {
    toast.error('加载 hooks.json 失败: ' + e)
  }
}

/** 保存：前端先做 JSON 语法自检（语义校验由后端 parse_cc_hooks 把关，
 * 校验失败不落盘）。总览与设置页共用同一保存通道。 */
async function saveContent(content: string, label: string) {
  try {
    JSON.parse(content)
  } catch (e: any) {
    toast.error('JSON 语法错误，未保存: ' + e?.message || e)
    return false
  }
  try {
    const data = await request('hooks', 'set', { content })
    hooksExists.value = true
    hooksValid.value = true
    hooksError.value = null
    // 冷启动（Agent 启动时无桥）首次保存：文件已落盘但无热更载体，需重启
    // 一次；之后（桥已存在）保存即下条消息生效。
    toast.success(
      coldStart.value
        ? `已保存（${data?.summary?.total ?? 0} 个钩子）。首次添加：重启 Agent 后生效，之后保存即下条消息生效`
        : `已保存（${data?.summary?.total ?? 0} 个钩子）。保存后下条消息生效`,
    )
    return true
  } catch (e: any) {
    // 后端语义校验拒绝 —— 错误串就是 parse 详情，文件未动。
    toast.error('保存被拒（文件未写入）: ' + e)
    return false
  }
}

async function saveRaw() {
  rawSaving.value = true
  const ok = await saveContent(rawJson.value, '原文')
  if (ok) entries.value = parseEntries(rawJson.value)
  rawSaving.value = false
}

async function saveEditor() {
  editorSaving.value = true
  const content = serializeEntries()
  const ok = await saveContent(content, '结构化')
  if (ok) {
    rawJson.value = content
    entries.value = parseEntries(content)
  }
  editorSaving.value = false
}

// 切换 TAB 即从磁盘刷新（丢弃另一 TAB 未保存的本地修改——最后写入者胜）。
watch(activeTab, () => {
  void loadHooks()
})

// ---------------------------------------------------------------------------
// 作用会话选择器（2026-11-16 会话圈定）：按标题搜、按裸 sid 存。
// ---------------------------------------------------------------------------

const pickerFor = ref<{ ev: string; idx: number } | null>(null)
const pickerSearch = ref('')
const pickerDraft = ref<string[]>([])
// 弹层坐标（Teleport + position:fixed）：absolute 原版挂在卡片内行上，
// 弹层伸出卡片下缘被 .card{overflow:hidden} 裁掉（2026-11-18 用户实测：
// 底部卡片 picker 只露搜索框一条，列表完全看不见）。
let pickerAnchor: HTMLElement | null = null
const pickerPos = ref({ top: 0, left: 0 })
const pickerRoot = ref<HTMLElement | null>(null)

async function openPicker(evId: string, idx: number, e: MouseEvent) {
  pickerFor.value = { ev: evId, idx }
  pickerSearch.value = ''
  pickerDraft.value = [...entries.value[evId][idx].sessions]
  pickerAnchor = e.currentTarget as HTMLElement
  // 初始坐标先按锚按钮下方落（多数场景够用），watch 里 nextTick 量高后收口。
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

watch(pickerFor, async (v) => {
  if (!v) return
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

function togglePickerSid(sid: string) {
  const i = pickerDraft.value.indexOf(sid)
  if (i >= 0) pickerDraft.value.splice(i, 1)
  else pickerDraft.value.push(sid)
}

function applyPicker() {
  if (!pickerFor.value) return
  entries.value[pickerFor.value.ev][pickerFor.value.idx].sessions = [...pickerDraft.value]
  pickerFor.value = null
}

/** 选择器候选：标题 / sid / 首条消息按搜索串过滤。 */
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

/** sid → 显示标题（列表缺席/无标题回退裸 sid——诚实不编造）。 */
function sessionTitle(sid: string): string {
  return sessionStore.sessions.find(s => s.id === sid)?.title ?? sid
}

/** sid → 项目徽标文案（项目名；注册表缺席回退 📁，无项目 null）。 */
function projectBadgeOf(sid: string): string | null {
  const pid = sessionStore.sessions.find(s => s.id === sid)?.projectId
  if (!pid) return null
  return sessionStore.projectNameOf(pid) ?? '📁'
}

/** 编辑行作用会话按钮文案。 */
function scopeLabel(e: HookEntry): string {
  return e.sessions.length ? `已圈 ${e.sessions.length} 个会话` : '全部会话'
}

/** 总览明细行作用域徽标：`全部会话` / `N 个会话: 标题 +k`（未知 sid 原样显示）。 */
function scopeBadge(e: HookEntry): string {
  if (!e.sessions.length) return '全部会话'
  const first = sessionTitle(e.sessions[0])
  return e.sessions.length === 1
    ? first
    : `${e.sessions.length} 个会话: ${first} +${e.sessions.length - 1}`
}

onMounted(() => {
  void loadHooks()
  void sessionStore.fetchList()
})
</script>

<template>
  <div class="page-hooks">
    <div class="page-header" style="display: flex; justify-content: space-between; align-items: center;">
      <h2>Hooks 钩子</h2>
    </div>
    <div class="page-body">
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'overview' }" @click="activeTab = 'overview'">总览</button>
        <button class="tab" :class="{ active: activeTab === 'editor' }" @click="activeTab = 'editor'">设置</button>
      </div>

      <div v-if="!hooksValid" class="hooks-banner hooks-banner--error">
        磁盘上的 hooks.json 解析失败（Agent 启动时已跳过加载，fail-open）：{{ hooksError }}<br>
        请修正后保存即可恢复。
      </div>
      <div v-else-if="!hooksExists" class="hooks-banner hooks-banner--info">
        尚未配置 hooks.json —— 保存后创建文件。
      </div>

      <!-- ===================== 总览 ===================== -->
      <div v-if="activeTab === 'overview'">
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>已配置钩子<span v-if="totalCount" style="font-weight: 400; font-size: var(--text-sm); color: var(--text-muted);">　共 {{ totalCount }} 条</span></h3></div>
          <div class="card-body">
            <div class="hooks-stats">
              <div v-for="ev in EVENTS" :key="ev.id" class="hooks-stat" :class="{ 'hooks-stat--empty': !entries[ev.id].length }">
                <div class="hooks-stat-name">{{ ev.label }}</div>
                <div class="hooks-stat-count">{{ entries[ev.id].length }}</div>
              </div>
            </div>
            <template v-for="ev in EVENTS" :key="ev.id">
              <div v-if="entries[ev.id].length" style="margin-top: var(--space-3);">
                <div style="font-weight: 600; font-size: var(--text-sm); margin-bottom: var(--space-1);">{{ ev.label }}</div>
                <div v-for="(e, i) in entries[ev.id]" :key="i" class="hooks-detail-row">
                  <span class="hooks-detail-matcher">{{ e.matcher || '全部工具' }}</span>
                  <span class="hooks-detail-scope" :title="e.sessions.length ? e.sessions.join('、') : ''">{{ scopeBadge(e) }}</span>
                  <span v-if="e.type === 'prompt'" class="hooks-detail-type">提示词</span>
                  <span style="flex: 1; font-family: var(--font-mono); font-size: var(--text-xs); word-break: break-all;">{{ e.type === 'prompt' ? e.prompt : e.command }}</span>
                  <span v-if="e.type === 'command'" style="color: var(--text-muted); font-size: var(--text-xs);">超时 {{ e.timeout || 60 }}s</span>
                </div>
              </div>
            </template>
            <p v-if="totalCount === 0" class="empty-state" style="padding: var(--space-4); text-align: center; color: var(--text-muted);">
              暂无已配置的钩子——切到「设置」TAB 添加。
            </p>
          </div>
        </div>

        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>hooks.json 原文</h3>
            <div style="display: flex; gap: var(--space-2);">
              <button class="btn btn-sm" @click="loadHooks" :disabled="rawSaving">重载</button>
              <button class="btn btn-sm btn-primary" @click="saveRaw" :disabled="rawSaving">
                {{ rawSaving ? '保存中…' : '保存原文' }}
              </button>
            </div>
          </div>
          <div class="card-body">
            <textarea class="form-textarea" style="min-height: 40vh; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="rawJson"></textarea>
            <p class="form-hint" style="margin-top: var(--space-2);">
              可直接编辑；保存走后端方言语义校验，校验失败不落盘。保存后「设置」页随之更新。
            </p>
          </div>
        </div>

        <div class="card">
          <div class="card-header"><h3>Hooks 钩子说明</h3></div>
          <div class="card-body">
            <p style="font-size: var(--text-sm); margin: 0 0 var(--space-3);">
              hooks.json 方言：每个 hook 有两种执行体（<code>type</code> 字段）——
              <strong>command 型</strong> = 子进程脚本（stdin 收单行 JSON 事件、
              env <code>CLAUDE_PROJECT_DIR</code>、cwd 为 workspace）；<strong>prompt 型</strong> =
              提示词注入（仅 Stop 事件）。十二个事件映射到 Agent 内核钩点：
            </p>
            <table class="hooks-table">
              <thead><tr><th>事件</th><th>触发时机</th><th>阻断语义（exit 2）</th></tr></thead>
              <tbody>
                <tr><td>SessionStart</td><td>每会话首条 prompt 到达时</td><td>观察型（不阻断）</td></tr>
                <tr><td>UserPromptSubmit</td><td>用户消息进入历史之前</td><td>拦下 prompt，模型永远看不到</td></tr>
                <tr><td>PreToolUse</td><td>工具执行前（安全 8 层闸之后）</td><td>拦截工具调用，stderr 作理由回灌模型</td></tr>
                <tr><td>PostToolUse</td><td>工具执行后（Forge 记录前）</td><td>stderr 追加到结果反馈模型（不撤销已执行操作）</td></tr>
                <tr><td>PostToolUseFailure</td><td>工具执行失败后</td><td>观察型（stderr 只记日志）</td></tr>
                <tr><td>Stop</td><td>最终答案被接受后、轮次结束前</td><td>阻止收尾，stderr 作反馈再答一轮（每轮封顶 2 次）；提示词型钩子注入提示词开新轮（无封顶）</td></tr>
                <tr><td>SessionEnd</td><td>会话被清理/删除时</td><td>观察型（无阻断语义）</td></tr>
                <tr><td>PreCompact</td><td>上下文压缩前</td><td>观察型（不阻止压缩）</td></tr>
                <tr><td>PostCompact</td><td>上下文压缩后</td><td>观察型</td></tr>
                <tr><td>SubagentStart</td><td>子代理启动前（本地 spawn）</td><td>拒绝 spawn，stderr 作理由</td></tr>
                <tr><td>SubagentStop</td><td>子代理任务完成后</td><td>观察型（任务已跑完，无阻断语义）</td></tr>
                <tr><td>Notification</td><td>审批/提问边界通知</td><td>观察型</td></tr>
              </tbody>
            </table>
            <p style="font-size: var(--text-sm); margin: var(--space-3) 0 0; color: var(--text-muted);">
              脚本协议：退出码 0 = 放行（PreToolUse/Stop 还认 stdout JSON <code>{"decision":"block","reason":…}</code>）；
              2 = 阻断；其他/超时/启动失败 = 非阻断错误（fail-open）。触发工具（matcher）用工具名
              （Bash/Edit/Write/Read/Grep，本系统工具名自动映射别名），留空 = 全命中。command 型可设
              <code>timeout</code> 秒（默认 60）；任何钩子可加
              <code>scope: {"sessions": ["<会话id>"]}</code> 圈定作用会话（「设置」TAB 按标题勾选、
              落盘存会话 id；不设 = 全部会话生效）。LLM 调用级钩子（K1b）无对应事件，不在此文件配置。
            </p>
            <p style="font-size: var(--text-sm); margin: var(--space-3) 0 0; color: var(--text-muted);">
              提示词型钩子（<code>type: "prompt"</code>，LLM 执行体，仅 Stop 事件）：钩子可以不是脚本而是
              一段<em>提示词</em>——每轮对话收尾后，该提示词作为新的用户消息注入，开启下一轮对话，
              <strong>没有轮数上限</strong>（例如「继续检查代码是否正确，确认是否有问题」即可让模型
              长期持续自查）。两型统一存放在本文件（hooks.json，2026-11-19 统一存储）：「设置」TAB
              对 Stop 事件可切换命令型/提示词型；Agent 也可在对话内用 <code>hooks</code> 工具给当前
              会话设置（只能给自己设，落盘为会话圈定组）——对 Agent 说「每轮结束后继续检查代码」即可。
              停止方式：删除该钩子、急停开关，或删除对应会话。
            </p>
          </div>
        </div>
      </div>

      <!-- ===================== 设置（结构化编辑） ===================== -->
      <div v-else>
        <p style="font-size: var(--text-sm); color: var(--text-secondary); margin: 0 0 var(--space-3);">
          每条钩子 = 执行体类型（仅 Stop 事件可选提示词型）+ 触发工具（可选，留空对全部工具生效）+
          命令/提示词 + 超时（命令型）+ 作用会话（可选，不选 = 全部会话生效）。保存时后端做方言
          语义校验，校验失败不落盘；保存后下条消息自动生效（桥内热更；Agent 启动时无钩子配置的
          首次保存需重启一次）。
        </p>
        <div v-for="ev in EVENTS" :key="ev.id" class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header" style="justify-content: space-between;">
            <h3 style="margin: 0;">
              {{ ev.label }}
              <span v-if="entries[ev.id].length" style="font-weight: 400; font-size: var(--text-sm); color: var(--text-muted);">（{{ entries[ev.id].length }} 条）</span>
            </h3>
            <button class="btn btn-sm" @click="entries[ev.id].push({ type: 'command', matcher: '', command: '', prompt: '', timeout: '', sessions: [] })">+ 添加钩子</button>
          </div>
          <div class="card-body">
            <p style="font-size: var(--text-xs); color: var(--text-muted); margin: 0 0 var(--space-2);">{{ ev.hint }}</p>
            <div v-if="!entries[ev.id].length" style="color: var(--text-muted); font-size: var(--text-sm);">该事件暂无钩子</div>
            <div v-for="(e, i) in entries[ev.id]" :key="i" style="margin-bottom: var(--space-2);">
              <div style="display: flex; gap: var(--space-2); align-items: center;">
                <!-- 二型切换（统一存储，仅 Stop 事件开放提示词型——唯一有
                     「turn 结束注入」时机的钩点）。 -->
                <select v-if="ev.id === 'Stop'" class="form-input" style="width: 96px; flex-shrink: 0;" v-model="e.type">
                  <option value="command">命令型</option>
                  <option value="prompt">提示词型</option>
                </select>
                <input class="form-input" style="width: 170px; flex-shrink: 0;" v-model="e.matcher" placeholder="触发工具（空=全部）" />
                <textarea
                  v-if="e.type === 'prompt'"
                  class="form-textarea"
                  style="flex: 1; min-height: 52px; font-family: var(--font-mono);"
                  v-model="e.prompt"
                  placeholder="提示词（每轮结束后注入开新轮，如：继续检查代码是否正确，确认是否有问题）"
                />
                <input
                  v-else
                  class="form-input"
                  style="flex: 1; font-family: var(--font-mono);"
                  v-model="e.command"
                  placeholder="命令（如 python lint.py）"
                />
                <template v-if="e.type === 'command'">
                  <input class="form-input" style="width: 76px; flex-shrink: 0;" v-model="e.timeout" placeholder="60" />
                  <span style="color: var(--text-muted); font-size: var(--text-xs); flex-shrink: 0;">秒</span>
                </template>
                <button class="btn btn-sm" style="color: var(--danger); flex-shrink: 0;" @click="entries[ev.id].splice(i, 1)">✕</button>
              </div>
              <div style="display: flex; gap: var(--space-2); align-items: center; margin-top: var(--space-1); position: relative;">
                <button
                  class="btn btn-sm hooks-scope-btn"
                  :class="{ 'hooks-scope-btn--scoped': e.sessions.length }"
                  @click="openPicker(ev.id, i, $event)"
                >
                  {{ scopeLabel(e) }} ▾
                </button>
                <span style="color: var(--text-muted); font-size: var(--text-xs);">作用会话（不选 = 全部会话生效）</span>
                <Teleport to="body">
                  <div
                    v-if="pickerFor && pickerFor.ev === ev.id && pickerFor.idx === i"
                    ref="pickerRoot"
                    class="hooks-scope-picker"
                    :style="{ top: pickerPos.top + 'px', left: pickerPos.left + 'px' }"
                  >
                    <input class="form-input" style="width: 100%;" v-model="pickerSearch" placeholder="按标题 / sid 搜索会话…" />
                    <div class="hooks-scope-list">
                      <label v-for="s in pickerRows" :key="s.id" class="hooks-scope-item">
                        <input type="checkbox" :checked="pickerDraft.includes(s.id)" @change="togglePickerSid(s.id)" />
                        <span class="hooks-scope-item-title">{{ s.title || s.id }}</span>
                        <span v-if="s.projectId" class="hooks-scope-item-badge">{{ projectBadgeOf(s.id) ?? '📁' }}</span>
                        <span class="hooks-scope-item-sid">{{ s.id }}</span>
                      </label>
                      <div v-if="!pickerRows.length" style="padding: var(--space-3); color: var(--text-muted); font-size: var(--text-xs);">无匹配会话</div>
                    </div>
                    <div class="hooks-scope-picker-footer">
                      <span style="color: var(--text-muted); font-size: var(--text-xs);">已选 {{ pickerDraft.length }} 个（不选 = 全部会话）</span>
                      <div style="display: flex; gap: var(--space-2);">
                        <button class="btn btn-sm" @click="pickerDraft = []">清空</button>
                        <button class="btn btn-sm btn-primary" @click="applyPicker">完成</button>
                      </div>
                    </div>
                  </div>
                </Teleport>
              </div>
            </div>
          </div>
        </div>
        <div class="card">
          <div class="card-body" style="display: flex; justify-content: flex-end; gap: var(--space-2); align-items: center;">
            <span style="color: var(--text-muted); font-size: var(--text-xs);">保存后下条消息生效（首次添加需重启）</span>
            <button class="btn btn-primary" @click="saveEditor" :disabled="editorSaving">
              {{ editorSaving ? '保存中…' : '保存全部钩子' }}
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* Hooks event mapping table */
.hooks-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-sm);
}
.hooks-table th,
.hooks-table td {
  padding: 6px 10px;
  border: 1px solid var(--border-light);
  text-align: left;
  vertical-align: top;
}
.hooks-table th {
  background: var(--bg-secondary);
  font-weight: 600;
}
.hooks-table td:first-child {
  white-space: nowrap;
  font-family: var(--font-mono);
}

/* 总览统计徽标 */
.hooks-stats {
  display: flex;
  gap: var(--space-3);
  flex-wrap: wrap;
}
.hooks-stat {
  flex: 1;
  min-width: 120px;
  padding: var(--space-2) var(--space-3);
  border: 1px solid var(--border-light);
  border-radius: var(--radius-md);
  background: var(--bg-secondary);
  text-align: center;
}
.hooks-stat--empty { opacity: 0.55; }
.hooks-stat-name { font-size: var(--text-xs); color: var(--text-secondary); }
.hooks-stat-count { font-size: var(--text-lg); font-weight: 600; }

/* 总览只读明细行 */
.hooks-detail-row {
  display: flex;
  gap: var(--space-3);
  align-items: baseline;
  padding: 4px 0;
  border-bottom: 1px dashed var(--border-light);
}
.hooks-detail-row:last-child { border-bottom: none; }
.hooks-detail-matcher {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  padding: 1px 6px;
  border: 1px solid var(--border-light);
  border-radius: var(--radius-sm);
  background: var(--bg-secondary);
  white-space: nowrap;
}
/* 类型徽标（提示词型，统一存储二型 2026-11-19）——实线框与 matcher 区分。 */
.hooks-detail-type {
  font-size: var(--text-xs);
  padding: 1px 6px;
  border: 1px solid var(--accent);
  color: var(--accent);
  border-radius: var(--radius-sm);
  white-space: nowrap;
}

/* 作用会话徽标（scope，2026-11-16）——虚线框与 matcher 实线框区分。 */
.hooks-detail-scope {
  font-size: var(--text-xs);
  color: var(--text-secondary);
  background: var(--bg-secondary);
  border: 1px dashed var(--border-light);
  border-radius: var(--radius-sm);
  padding: 1px 6px;
  white-space: nowrap;
}

/* 作用会话选择器（编辑行弹层） */
.hooks-scope-btn--scoped {
  color: var(--accent);
  border-color: var(--accent);
}
.hooks-scope-picker {
  /* fixed + Teleport（body）：脱离 .card 的 overflow:hidden 裁剪域；
     坐标由 placePicker/watch 按锚按钮实时计算。 */
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
.hooks-scope-list {
  max-height: min(260px, calc(100vh - 160px));
  overflow-y: auto;
  display: flex;
  flex-direction: column;
}
.hooks-scope-item {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: 4px 6px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: var(--text-xs);
}
.hooks-scope-item:hover { background: var(--surface-hover); }
.hooks-scope-item-title {
  flex-shrink: 0;
  max-width: 55%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 500;
}
.hooks-scope-item-badge {
  flex-shrink: 0;
  color: var(--text-muted);
}
.hooks-scope-item-sid {
  flex: 1;
  color: var(--text-muted);
  font-family: var(--font-mono);
  font-size: 10px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  text-align: right;
}
.hooks-scope-picker-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
  border-top: 1px solid var(--border-light);
  padding-top: var(--space-2);
}

/* 状态横幅 */
.hooks-banner {
  padding: var(--space-3);
  margin-bottom: var(--space-3);
  border-radius: var(--radius-md);
  font-size: var(--text-sm);
}
.hooks-banner--error {
  background: var(--danger-bg, #fdecea);
  border: 1px solid var(--danger, #dc3545);
}
.hooks-banner--info {
  background: var(--accent-muted, #e8f0fe);
  border: 1px solid var(--border-light);
}
</style>
