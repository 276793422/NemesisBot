<script setup lang="ts">
import { ref, watch, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

// Hooks 钩子页（2026-08-29 自设置页迁移 + 双 TAB 重构）：
// - 总览：统计徽标 + 每事件只读明细 + 可编辑原文 + CC 5 事件文档表
// - 设置：扁平结构化编辑（每条钩子 = 触发工具[可空] + 命令 + 超时），
//   保存时每条各自成组写入 hooks.json（方案 B，语义与分组版等价）。
// 单一真相源 = hooks.json；两处编辑都走 hooks.set（后端语义校验兜底），
// 切换 TAB 即从磁盘刷新（最后写入者胜，用户已接受覆盖冲突）。

const { request } = useWSAPI()
const toast = useToast()

const EVENTS = [
  { id: 'PreToolUse', label: 'PreToolUse', hint: '工具执行前（安全 8 层闸之后）——可拦截工具调用' },
  { id: 'PostToolUse', label: 'PostToolUse', hint: '工具执行后——stderr 可追加反馈给模型（不撤销操作）' },
  { id: 'PostToolUseFailure', label: 'PostToolUseFailure', hint: '工具执行失败后（观察型：stderr 只记日志）' },
  { id: 'SessionStart', label: 'SessionStart', hint: '每会话首条 prompt 到达时（观察型，不阻断）' },
  { id: 'UserPromptSubmit', label: 'UserPromptSubmit', hint: '用户消息进入历史之前——可拦下 prompt' },
  { id: 'Stop', label: 'Stop', hint: '最终答案被接受后——可阻止收尾再答一轮（封顶 2 次）' },
  { id: 'SessionEnd', label: 'SessionEnd', hint: '会话被清理/删除时（观察型，无阻断语义）' },
  { id: 'PreCompact', label: 'PreCompact', hint: '上下文压缩前（观察型：不阻止压缩）' },
  { id: 'PostCompact', label: 'PostCompact', hint: '上下文压缩后（观察型）' },
] as const

interface HookEntry {
  /** 触发工具的 matcher（CC 工具名正则/子串；空 = 全部工具）。 */
  matcher: string
  command: string
  /** 秒；空串 = 后端默认 60。输入框绑定用字符串。 */
  timeout: string
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
const entries = ref<Record<string, HookEntry[]>>(emptyEntries())
const rawSaving = ref(false)
const editorSaving = ref(false)
const restarting = ref(false)

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
        matcher: typeof g?.matcher === 'string' ? g.matcher : '',
        command: typeof c?.command === 'string' ? c.command : '',
        timeout: c?.timeout != null ? String(c.timeout) : '',
      })),
    )
  }
  return out
}

/** 扁平条目 → hooks.json（方案 B：每条钩子各自成组；空事件不输出）。 */
function serializeEntries(): string {
  const hooks: Record<string, any> = {}
  for (const ev of EVENTS) {
    const list = entries.value[ev.id].filter(e => e.command.trim())
    if (!list.length) continue
    hooks[ev.id] = list.map(e => {
      const cmd: any = { type: 'command', command: e.command.trim() }
      const t = Number(e.timeout)
      if (e.timeout.trim() && Number.isFinite(t) && t > 0) cmd.timeout = t
      const group: any = { hooks: [cmd] }
      if (e.matcher.trim()) group.matcher = e.matcher.trim()
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
    toast.success(`已保存（${data?.summary?.total ?? 0} 个脚本）。重启 Agent 后生效`)
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

/** hooks.json 只在 Agent 启动时加载（agent_factory），保存后一键重启生效。 */
async function restartAgentForHooks() {
  restarting.value = true
  try {
    await request('agent', 'stop')
    await new Promise(r => setTimeout(r, 1000))
    await request('agent', 'start')
    toast.success('Agent 已重启，hooks 配置已生效')
  } catch (e: any) {
    toast.error('重启 Agent 失败: ' + (e?.message || e))
  }
  restarting.value = false
}

// 切换 TAB 即从磁盘刷新（丢弃另一 TAB 未保存的本地修改——最后写入者胜）。
watch(activeTab, () => {
  void loadHooks()
})

onMounted(loadHooks)
</script>

<template>
  <div class="page-hooks">
    <div class="page-header" style="display: flex; justify-content: space-between; align-items: center;">
      <h2>Hooks 钩子</h2>
      <button class="btn btn-sm" :disabled="restarting" @click="restartAgentForHooks">
        {{ restarting ? '重启中…' : '重启 Agent 生效' }}
      </button>
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
          <div class="card-header"><h3>已配置钩子<span v-if="totalCount" style="font-weight: 400; font-size: var(--text-sm); color: var(--text-muted);">　共 {{ totalCount }} 个脚本</span></h3></div>
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
                  <span style="flex: 1; font-family: var(--font-mono); font-size: var(--text-xs); word-break: break-all;">{{ e.command }}</span>
                  <span style="color: var(--text-muted); font-size: var(--text-xs);">超时 {{ e.timeout || 60 }}s</span>
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
              可直接编辑；保存走后端 CC 方言语义校验，校验失败不落盘。保存后「设置」页随之更新。
            </p>
          </div>
        </div>

        <div class="card">
          <div class="card-header"><h3>Hooks 钩子说明</h3></div>
          <div class="card-body">
            <p style="font-size: var(--text-sm); margin: 0 0 var(--space-3);">
              hooks.json 采用 Claude Code 方言：每个 hook 是一条子进程脚本（stdin 收单行 JSON 事件、
              env <code>CLAUDE_PROJECT_DIR</code>、cwd 为 workspace），九个事件映射到 Agent 内核钩点：
            </p>
            <table class="hooks-table">
              <thead><tr><th>CC 事件</th><th>触发时机</th><th>阻断语义（exit 2）</th></tr></thead>
              <tbody>
                <tr><td>SessionStart</td><td>每会话首条 prompt 到达时</td><td>观察型（不阻断）</td></tr>
                <tr><td>UserPromptSubmit</td><td>用户消息进入历史之前</td><td>拦下 prompt，模型永远看不到</td></tr>
                <tr><td>PreToolUse</td><td>工具执行前（安全 8 层闸之后）</td><td>拦截工具调用，stderr 作理由回灌模型</td></tr>
                <tr><td>PostToolUse</td><td>工具执行后（Forge 记录前）</td><td>stderr 追加到结果反馈模型（不撤销已执行操作）</td></tr>
                <tr><td>PostToolUseFailure</td><td>工具执行失败后</td><td>观察型（stderr 只记日志）</td></tr>
                <tr><td>Stop</td><td>最终答案被接受后、轮次结束前</td><td>阻止收尾，stderr 作反馈再答一轮（每轮封顶 2 次）</td></tr>
                <tr><td>SessionEnd</td><td>会话被清理/删除时</td><td>观察型（无阻断语义）</td></tr>
                <tr><td>PreCompact</td><td>上下文压缩前</td><td>观察型（不阻止压缩）</td></tr>
                <tr><td>PostCompact</td><td>上下文压缩后</td><td>观察型</td></tr>
              </tbody>
            </table>
            <p style="font-size: var(--text-sm); margin: var(--space-3) 0 0; color: var(--text-muted);">
              脚本协议：退出码 0 = 放行（PreToolUse/Stop 还认 stdout JSON <code>{"decision":"block","reason":…}</code>）；
              2 = 阻断；其他/超时/启动失败 = 非阻断错误（fail-open）。触发工具（matcher）用 CC 工具名
              （Bash/Edit/Write/Read/Grep，本系统工具名自动映射别名），留空 = 全命中。每条钩子可设
              <code>timeout</code> 秒（默认 60）。LLM 调用级钩子（K1b）无 CC 对应事件，不在此文件配置。
            </p>
          </div>
        </div>
      </div>

      <!-- ===================== 设置（结构化编辑） ===================== -->
      <div v-else>
        <p style="font-size: var(--text-sm); color: var(--text-secondary); margin: 0 0 var(--space-3);">
          每条钩子 = 触发工具（可选，留空对全部工具生效）+ 命令 + 超时。保存时后端做 CC 方言语义校验，
          校验失败不落盘；保存后点右上角「重启 Agent 生效」。
        </p>
        <div v-for="ev in EVENTS" :key="ev.id" class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header" style="justify-content: space-between;">
            <h3 style="margin: 0;">
              {{ ev.label }}
              <span v-if="entries[ev.id].length" style="font-weight: 400; font-size: var(--text-sm); color: var(--text-muted);">（{{ entries[ev.id].length }} 条）</span>
            </h3>
            <button class="btn btn-sm" @click="entries[ev.id].push({ matcher: '', command: '', timeout: '' })">+ 添加钩子</button>
          </div>
          <div class="card-body">
            <p style="font-size: var(--text-xs); color: var(--text-muted); margin: 0 0 var(--space-2);">{{ ev.hint }}</p>
            <div v-if="!entries[ev.id].length" style="color: var(--text-muted); font-size: var(--text-sm);">该事件暂无钩子</div>
            <div v-for="(e, i) in entries[ev.id]" :key="i" style="display: flex; gap: var(--space-2); align-items: center; margin-bottom: var(--space-2);">
              <input class="form-input" style="width: 170px; flex-shrink: 0;" v-model="e.matcher" placeholder="触发工具（空=全部）" />
              <input class="form-input" style="flex: 1; font-family: var(--font-mono);" v-model="e.command" placeholder="命令（如 python lint.py）" />
              <input class="form-input" style="width: 76px; flex-shrink: 0;" v-model="e.timeout" placeholder="60" />
              <span style="color: var(--text-muted); font-size: var(--text-xs); flex-shrink: 0;">秒</span>
              <button class="btn btn-sm" style="color: var(--danger); flex-shrink: 0;" @click="entries[ev.id].splice(i, 1)">✕</button>
            </div>
          </div>
        </div>
        <div class="card">
          <div class="card-body" style="display: flex; justify-content: flex-end; gap: var(--space-2); align-items: center;">
            <span style="color: var(--text-muted); font-size: var(--text-xs);">保存后需重启 Agent 生效</span>
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
/* CC 5-event mapping table */
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
