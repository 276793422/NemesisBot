<script setup lang="ts">
import { ref, computed, watch, onMounted, onBeforeUnmount } from 'vue'
import { useRouter } from 'vue-router'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { useSessionStore } from '../stores/session'

const { request } = useWSAPI()
const toast = useToast()
const router = useRouter()
const sessionStore = useSessionStore()

defineProps<{ embedded?: boolean }>()

// --- Records (run history) modal state ---
const showRecordsModal = ref(false)
const recordsJob = ref<any>(null)

function openRecords(job: any) {
  recordsJob.value = job
  showRecordsModal.value = true
}

// Extract the conversation id from a session_key (`agent:main:session:<sid>` → `<sid>`).
function sessionIdFromKey(key: string): string {
  const prefix = 'agent:main:session:'
  return key.startsWith(prefix) ? key.slice(prefix.length) : ''
}

// Jump to the targeted conversation: switch the active session + go to chat.
async function openConversation(sessionKey: string) {
  const sid = sessionIdFromKey(sessionKey)
  if (!sid) { toast.warn('该任务未指定目标会话'); return }
  await sessionStore.fetchList() // make sure the sidebar lists it
  sessionStore.switchTo(sid)
  showRecordsModal.value = false
  router.push({ name: 'chat' })
}

const activeTab = ref('boot')
const bootContent = ref('')
const heartbeatContent = ref('')
const cronJobs = ref<any[]>([])
const loading = ref(true)
const editing = ref(false)
const editContent = ref('')

// --- Cron form state ---
interface CronForm {
  id: string | null
  name: string
  preset: 'daily' | 'weekly' | 'monthly' | 'minutes' | 'custom'
  time: string
  weekday: number
  monthDay: number
  everyMinutes: number
  cron: string
  sessionKey: string
  prompt: string
  enabled: boolean
}
const showCronModal = ref(false)
const cronForm = ref<CronForm>(defaultCronForm())
const preview = ref<any>(null)
const sessions = ref<any[]>([])
const activePersona = ref('')
let previewTimer: any = null

function defaultCronForm(): CronForm {
  return {
    id: null, name: '', preset: 'daily', time: '09:00',
    weekday: 1, monthDay: 1, everyMinutes: 5,
    cron: '0 9 * * *', sessionKey: '', prompt: '', enabled: true,
  }
}

const WEEKDAYS = ['周日', '周一', '周二', '周三', '周四', '周五', '周六']
const PRESETS = [
  { key: 'daily', label: '每天' },
  { key: 'weekly', label: '每周' },
  { key: 'monthly', label: '每月' },
  { key: 'minutes', label: '每 N 分钟' },
  { key: 'custom', label: '自定义' },
] as const

function pad(n: number | string): string {
  const s = String(n)
  return s.length === 1 ? '0' + s : s
}

// Parse a time field, defaulting only when truly absent/invalid.
// NOTE: must NOT use `||` — 0 is a valid hour (midnight) and minute, but JS
// treats 0 as falsy so `parseInt("00") || 9` wrongly became 9.
function parseTimePart(s: string | undefined, def: number): number {
  const n = parseInt((s || '').trim())
  return Number.isNaN(n) ? def : n
}

// Derive a cron expression from the form's preset + time fields.
function buildCronExpr(f: CronForm): string {
  const [hh, mm] = (f.time || '09:00').split(':')
  const H = parseTimePart(hh, 9)
  const M = parseTimePart(mm, 0)
  switch (f.preset) {
    case 'daily': return `${M} ${H} * * *`
    case 'weekly': return `${M} ${H} * * ${f.weekday}`
    case 'monthly': return `${M} ${H} ${f.monthDay} * *`
    case 'minutes': return `*/${f.everyMinutes && f.everyMinutes > 0 ? f.everyMinutes : 1} * * * *`
    case 'custom': return (f.cron || '').trim()
  }
}

// Reverse: detect a preset from a stored cron expression (best-effort; falls
// back to 'custom' showing the raw expr).
function detectPreset(expr: string): Partial<CronForm> {
  const parts = (expr || '').trim().split(/\s+/)
  if (parts.length === 5) {
    const [min, hour, day, month, dow] = parts
    if (min.startsWith('*/') && hour === '*' && day === '*' && month === '*' && dow === '*') {
      return { preset: 'minutes', everyMinutes: parseInt(min.slice(2)) || 1 }
    }
    if (day === '*' && month === '*' && dow === '*') {
      return { preset: 'daily', time: `${pad(hour)}:${pad(min)}` }
    }
    if (day === '*' && month === '*' && dow !== '*') {
      const wd = parseInt(dow)
      return { preset: 'weekly', time: `${pad(hour)}:${pad(min)}`, weekday: isNaN(wd) ? 1 : wd }
    }
    if (day !== '*' && month === '*' && dow === '*') {
      const md = parseInt(day)
      return { preset: 'monthly', time: `${pad(hour)}:${pad(min)}`, monthDay: isNaN(md) ? 1 : md }
    }
  }
  return { preset: 'custom', cron: (expr || '').trim() }
}

const builtCron = computed(() => buildCronExpr(cronForm.value))

// Live preview (debounced) of the derived cron expression.
watch(builtCron, (expr) => {
  if (!showCronModal.value) return
  clearTimeout(previewTimer)
  previewTimer = setTimeout(async () => {
    if (!expr) { preview.value = null; return }
    try {
      preview.value = await request('tasks', 'cron.preview', { cron: expr })
    } catch (e: any) {
      preview.value = { valid: false, description: String(e?.message || e) }
    }
  }, 300)
})

function formatMs(ms: any): string {
  if (!ms || typeof ms !== 'number') return '—'
  try { return new Date(ms).toLocaleString() } catch { return '—' }
}

function sessionLabel(job: any): string {
  if (!job.session_key) return ''
  const tail = String(job.session_key).split(':').pop()
  return tail || job.session_key
}

async function loadBoot() {
  try {
    const data = await request('tasks', 'boot.get')
    bootContent.value = data?.content || ''
  } catch { /* ignore */ }
}

async function loadHeartbeat() {
  try {
    const data = await request('tasks', 'heartbeat.get')
    heartbeatContent.value = data?.content || ''
  } catch { /* ignore */ }
}

async function loadCronJobs() {
  try {
    const data = await request('tasks', 'cron.list')
    cronJobs.value = data?.jobs || []
  } catch { /* ignore */ }
}

async function loadSessions() {
  try {
    const r = await request('logs', 'session_list', { limit: 100, offset: 0 })
    sessions.value = ((r?.sessions as any[]) || []).filter(
      (s) => s.session_key && String(s.session_key).startsWith('agent:main:session:')
    )
  } catch { sessions.value = [] }
}

async function loadActivePersona() {
  try {
    const r = await request('persona', 'current')
    activePersona.value = r?.name || '默认'
  } catch { activePersona.value = '默认' }
}

function startEdit() {
  if (activeTab.value === 'boot') editContent.value = bootContent.value
  else editContent.value = heartbeatContent.value
  editing.value = true
}

async function saveContent() {
  const cmd = activeTab.value === 'boot' ? 'boot.save' : 'heartbeat.save'
  try {
    await request('tasks', cmd, { content: editContent.value })
    toast.success('已保存')
    if (activeTab.value === 'boot') bootContent.value = editContent.value
    else heartbeatContent.value = editContent.value
    editing.value = false
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

function openAddCron() {
  cronForm.value = defaultCronForm()
  preview.value = null
  loadSessions()
  showCronModal.value = true
}

function openEditCron(job: any) {
  const det = detectPreset(job.cron)
  cronForm.value = {
    id: job.id, name: job.name || '', preset: (det.preset as any) || 'custom',
    time: (det.time as string) || '09:00',
    weekday: (det.weekday as number) ?? 1, monthDay: (det.monthDay as number) ?? 1,
    everyMinutes: (det.everyMinutes as number) ?? 5,
    cron: det.preset === 'custom' ? (det.cron as string) || job.cron || '' : job.cron || '',
    sessionKey: job.session_key || '', prompt: job.prompt || '', enabled: job.enabled !== false,
  }
  preview.value = null
  loadSessions()
  showCronModal.value = true
}

async function saveCronJob() {
  if (!cronForm.value.name.trim()) { toast.warn('请填写任务名称'); return }
  const expr = buildCronExpr(cronForm.value)
  if (!expr) { toast.warn('调度配置无效'); return }
  const payload: any = {
    name: cronForm.value.name.trim(),
    cron: expr,
    channel: 'web',
    session_key: cronForm.value.sessionKey || '',
    prompt: cronForm.value.prompt,
    enabled: cronForm.value.enabled,
  }
  try {
    if (cronForm.value.id) {
      payload.id = cronForm.value.id
      await request('tasks', 'cron.update', payload)
      toast.success('已更新')
    } else {
      await request('tasks', 'cron.add', payload)
      toast.success('已添加')
    }
    showCronModal.value = false
    await loadCronJobs()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

async function toggleCron(job: any) {
  try {
    await request('tasks', 'cron.toggle', { id: job.id })
    await loadCronJobs()
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

async function runCronNow(job: any) {
  try {
    await request('tasks', 'cron.run', { id: job.id })
    toast.success(`已触发「${job.name}」`)
    await loadCronJobs()
  } catch (e: any) {
    toast.error('触发失败: ' + e)
  }
}

async function deleteCronJob(id: string, name: string) {
  if (!confirm(`确定删除定时任务「${name}」？`)) return
  try {
    await request('tasks', 'cron.delete', { id })
    toast.success('已删除')
    await loadCronJobs()
  } catch (e: any) {
    toast.error('删除失败: ' + e)
  }
}

function switchTab(tab: string) {
  activeTab.value = tab
  editing.value = false
}

onMounted(async () => {
  await Promise.all([loadBoot(), loadHeartbeat(), loadCronJobs(), loadActivePersona()])
  loading.value = false
})

onBeforeUnmount(() => clearTimeout(previewTimer))
</script>

<template>
  <div :class="embedded ? 'tasks-embed' : 'page-tasks'">
    <div v-if="!embedded" class="page-header"><h2>任务管理</h2></div>
    <div :class="embedded ? '' : 'page-body'">
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'boot' }" @click="switchTab('boot')">启动任务</button>
        <button class="tab" :class="{ active: activeTab === 'heartbeat' }" @click="switchTab('heartbeat')">心跳任务</button>
        <button class="tab" :class="{ active: activeTab === 'cron' }" @click="switchTab('cron')">定时任务</button>
      </div>

      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <!-- Boot/Heartbeat editor -->
      <div v-if="!loading && (activeTab === 'boot' || activeTab === 'heartbeat')">
        <div class="card">
          <div class="card-header">
            <h3>{{ activeTab === 'boot' ? 'BOOT.md' : 'HEARTBEAT.md' }}</h3>
            <div style="display: flex; gap: var(--space-2);">
              <template v-if="!editing">
                <button class="btn btn-sm" @click="startEdit">编辑</button>
              </template>
              <template v-else>
                <button class="btn btn-sm" @click="editing = false">取消</button>
                <button class="btn btn-sm btn-primary" @click="saveContent">保存</button>
              </template>
            </div>
          </div>
          <div class="card-body">
            <p style="color: var(--text-muted); font-size: var(--text-sm); margin-bottom: var(--space-3);">
              {{ activeTab === 'boot' ? 'Agent 每次启动时执行的检查清单' : 'Agent 心跳轮询时执行的任务' }}
            </p>
            <div v-if="editing">
              <textarea class="form-textarea" style="min-height: 60vh; font-family: var(--font-mono); font-size: var(--text-sm);" v-model="editContent"></textarea>
            </div>
            <div v-else class="markdown-body">
              <pre style="white-space: pre-wrap;">{{ (activeTab === 'boot' ? bootContent : heartbeatContent) || '（空文件）' }}</pre>
            </div>
          </div>
        </div>
      </div>

      <!-- Cron jobs -->
      <div v-if="!loading && activeTab === 'cron'">
        <div style="display: flex; justify-content: flex-end; margin-bottom: var(--space-4);">
          <button class="btn btn-primary" @click="openAddCron">+ 添加任务</button>
        </div>

        <div v-if="cronJobs.length === 0" class="empty-state">
          <h3>暂无定时任务</h3>
          <p>点击「添加任务」创建定时任务，到点自动在指定对话里执行</p>
        </div>

        <div v-if="cronJobs.length > 0" class="table-wrap">
          <table>
            <thead>
              <tr>
                <th>名称</th><th>运行时间</th><th>目标会话</th><th>状态</th>
                <th>下次运行</th><th>上次运行</th><th>操作</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="job in cronJobs" :key="job.id">
                <td style="font-weight: 500;">{{ job.name }}</td>
                <td>
                  <div v-if="job.description">{{ job.description }}</div>
                  <code v-else>{{ job.cron }}</code>
                </td>
                <td>
                  <span v-if="job.session_key" class="badge badge-info">💬 {{ sessionLabel(job) }}</span>
                  <span v-else style="color: var(--text-muted);">—</span>
                </td>
                <td>
                  <span class="badge" :class="job.enabled ? 'badge-success' : 'badge-neutral'">{{ job.enabled ? '启用' : '暂停' }}</span>
                  <div v-if="job.last_status === 'error'" style="color: var(--error); font-size: var(--text-xs); margin-top: 2px;" :title="job.last_error">⚠ 出错</div>
                </td>
                <td style="font-size: var(--text-sm); color: var(--text-secondary);">{{ formatMs(job.next_run_at_ms) }}</td>
                <td style="font-size: var(--text-sm); color: var(--text-muted);">{{ formatMs(job.last_run_at_ms) }}</td>
                <td>
                  <div style="display: flex; gap: var(--space-1); flex-wrap: wrap;">
                    <button class="btn btn-sm" :class="job.enabled ? 'btn-ghost' : 'btn-success'" @click="toggleCron(job)" :title="job.enabled ? '暂停' : '启用'">{{ job.enabled ? '⏸' : '▶启' }}</button>
                    <button class="btn btn-sm" @click="runCronNow(job)" title="立即运行">⚡</button>
                    <button class="btn btn-sm" @click="openRecords(job)" title="运行记录">📋</button>
                    <button class="btn btn-sm" @click="openEditCron(job)" title="编辑">✏</button>
                    <button class="btn btn-sm btn-danger" @click="deleteCronJob(job.id, job.name)" title="删除">🗑</button>
                  </div>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>

    <!-- Add/Edit cron modal -->
    <div v-if="showCronModal" class="modal-backdrop" @click.self="showCronModal = false">
      <div class="modal" style="max-width: 560px;">
        <div class="modal-header">
          <h3>{{ cronForm.id ? '编辑定时任务' : '新建定时任务' }}</h3>
        </div>
        <div class="modal-body">
          <div class="form-group">
            <label class="form-label">名称 *</label>
            <input class="form-input" v-model="cronForm.name" placeholder="例如：每天9点汇报工作进度">
          </div>

          <div class="form-group">
            <label class="form-label">什么时候运行</label>
            <div class="preset-chips">
              <button
                v-for="p in PRESETS" :key="p.key"
                class="btn btn-sm preset-chip"
                :class="{ 'btn-primary': cronForm.preset === p.key }"
                @click="cronForm.preset = p.key"
              >{{ p.label }}</button>
            </div>

            <!-- daily / weekly / monthly: pick a time -->
            <div v-if="['daily','weekly','monthly'].includes(cronForm.preset)" class="preset-row">
              <input class="form-input" type="time" v-model="cronForm.time" style="max-width: 140px;">
              <select v-if="cronForm.preset === 'weekly'" class="form-select" v-model.number="cronForm.weekday" style="max-width: 140px;">
                <option v-for="(d, i) in WEEKDAYS" :key="i" :value="i">{{ d }}</option>
              </select>
              <select v-if="cronForm.preset === 'monthly'" class="form-select" v-model.number="cronForm.monthDay" style="max-width: 140px;">
                <option v-for="d in 28" :key="d" :value="d">{{ d }} 号</option>
              </select>
            </div>

            <!-- minutes -->
            <div v-if="cronForm.preset === 'minutes'" class="preset-row">
              <span style="color: var(--text-secondary);">每</span>
              <input class="form-input" type="number" min="1" max="59" v-model.number="cronForm.everyMinutes" style="max-width: 100px;">
              <span style="color: var(--text-secondary);">分钟运行一次</span>
            </div>

            <!-- custom -->
            <input v-if="cronForm.preset === 'custom'" class="form-input" v-model="cronForm.cron" placeholder="0 9 * * 1-5" style="margin-top: var(--space-2);">
          </div>

          <!-- live preview -->
          <div class="cron-preview" v-if="preview">
            <code>{{ builtCron }}</code>
            <span v-if="preview.valid" style="color: var(--success);">{{ preview.description }}</span>
            <span v-else style="color: var(--error);">{{ preview.description }}</span>
            <span v-if="preview.valid && preview.next_run_at_ms" style="color: var(--text-muted);">下次：{{ formatMs(preview.next_run_at_ms) }}</span>
          </div>

          <div class="form-group">
            <label class="form-label">发到哪个会话</label>
            <select class="form-select" v-model="cronForm.sessionKey">
              <option value="">不指定（仅记录，不投递到对话）</option>
              <option v-for="s in sessions" :key="s.session_key" :value="s.session_key">
                {{ s.title || s.firstMessage || s.id }} · {{ s.lastTime || '' }}
              </option>
            </select>
            <p class="form-hint">选择一个已有对话：到点 bot 会在该对话里接着聊（标签开着则实时弹出，关了则写进历史，重开可见）。</p>
          </div>

          <div class="form-group">
            <label class="form-label">人格</label>
            <input class="form-input" :value="`当前激活：${activePersona}（定时任务沿用）`" disabled>
          </div>

          <div class="form-group">
            <label class="form-label">提示词</label>
            <textarea class="form-textarea" v-model="cronForm.prompt" placeholder="到点要对 bot 说什么，例如：总结今天的工作并列出明天计划" style="min-height: 90px;"></textarea>
          </div>

          <div class="form-group" style="display: flex; align-items: center; gap: var(--space-2);">
            <div class="toggle" :class="{ active: cronForm.enabled }" @click="cronForm.enabled = !cronForm.enabled"></div>
            <span style="color: var(--text-secondary);">{{ cronForm.enabled ? '创建后立即启用' : '创建后保持暂停' }}</span>
          </div>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="showCronModal = false">取消</button>
          <button class="btn btn-primary" @click="saveCronJob">{{ cronForm.id ? '保存' : '添加' }}</button>
        </div>
      </div>
    </div>

    <!-- Run records modal -->
    <div v-if="showRecordsModal && recordsJob" class="modal-backdrop" @click.self="showRecordsModal = false">
      <div class="modal" style="max-width: 580px;">
        <div class="modal-header"><h3>运行记录 · {{ recordsJob.name }}</h3></div>
        <div class="modal-body">
          <p style="color: var(--text-muted); font-size: var(--text-sm); margin-bottom: var(--space-3);">
            {{ recordsJob.description || recordsJob.cron }} · 目标会话：{{ recordsJob.session_key ? sessionIdFromKey(recordsJob.session_key) : '未指定' }}
          </p>
          <div v-if="recordsJob.session_key" style="margin-bottom: var(--space-3);">
            <button class="btn btn-sm btn-primary" @click="openConversation(recordsJob.session_key)">💬 打开目标会话</button>
            <span style="color: var(--text-muted); font-size: var(--text-xs); margin-left: var(--space-2);">在那条对话里能看到 bot 的回复（带 🕒 标记）</span>
          </div>
          <div v-if="!recordsJob.history || recordsJob.history.length === 0" class="empty-state" style="padding: var(--space-4);">
            <p>暂无运行记录（任务还没触发过）</p>
          </div>
          <div v-else class="table-wrap">
            <table>
              <thead><tr><th>时间</th><th>状态</th><th>错误</th></tr></thead>
              <tbody>
                <tr v-for="(r, i) in [...recordsJob.history].reverse()" :key="i">
                  <td style="font-size: var(--text-sm); color: var(--text-secondary);">{{ formatMs(r.at_ms) }}</td>
                  <td>
                    <span class="badge" :class="r.status === 'error' ? 'badge-error' : 'badge-success'">{{ r.status === 'error' ? '失败' : (r.status === 'executed' ? '手动' : '成功') }}</span>
                  </td>
                  <td style="font-size: var(--text-xs); color: var(--error); max-width: 260px; word-break: break-all;">{{ r.error || '—' }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="showRecordsModal = false">关闭</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.preset-chips {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  margin-bottom: var(--space-2);
}
.preset-chip {
  opacity: 0.7;
}
.preset-chip.btn-primary {
  opacity: 1;
}
.preset-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-top: var(--space-2);
}
.cron-preview {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  margin-bottom: var(--space-3);
  background: var(--bg-secondary);
  border-radius: var(--radius-md);
  font-size: var(--text-sm);
}
.cron-preview code {
  background: var(--surface);
  padding: 2px 6px;
  border-radius: var(--radius-sm);
}
</style>
