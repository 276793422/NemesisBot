<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { useBoardChanged } from '../../composables/useBoardChanged'
import {
  PRIORITY_BADGE,
  PRIORITY_LABEL,
  STATUS_BADGE,
  TRANSITIONS,
  fmtTime,
  statusLabel,
} from './boardMeta'

// Issue 详情弹窗（W2 P3）：从 IssueListView 抽出为共享组件——看板（BoardKanban）
// 与列表（IssueListView）点击卡片/行都打开它。P3 增强：
// - 评论线程（parent_id 一层嵌套，回复 @ 作者）；
// - @提及辅助（订阅者/指派对象一键插入，后端 extract_mentions 产生 mentioned 通知）；
// - 附件（上传 base64 → attachment.add，下载 attachment.get → Blob）。
// 后端真相源：crates/nemesis-web/src/handlers/board.rs。

const { request } = useWSAPI()
const toast = useToast()

const props = defineProps<{
  issueId: number | null
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'changed'): void
}>()

const MAX_ATTACHMENT_BYTES = 8 * 1024 * 1024 // 与后端 MAX_ATTACHMENT_BYTES 对齐

interface Actor {
  kind: string
  id: string
}

interface Issue {
  id: number
  number: string
  title: string
  description: string
  status: string
  priority: number
  assignee: string | null
  assignee_id: string | null
  creator: Actor
  due_date: number | null
  position: number
  acceptance_criteria: string | null
  origin: { origin_type: string; origin_id: string } | null
  created_at: number
  updated_at: number
  comments?: CommentRow[]
  activity?: ActivityRow[]
  subscribers?: any[]
}

interface CommentRow {
  id: number
  author: Actor
  content: string
  parent_id: number | null
  ctype: string
  created_at: number
}

interface ActivityRow {
  id: number
  actor: Actor
  action: string
  details: string | null
  created_at: number
}

interface AttachmentRow {
  id: number
  issue_id: number
  filename: string
  storage_path: string
  size: number
  uploaded_by: Actor
  created_at: number
}

const detail = ref<Issue | null>(null)
const attachments = ref<AttachmentRow[]>([])
const busy = ref(false)

const newComment = ref('')
const replyTo = ref<CommentRow | null>(null)
// @提及辅助的候选集（订阅者 + 当前指派对象；插入后端能识别的 @id 记号）。
const mentionCandidates = computed<Actor[]>(() => {
  if (!detail.value) return []
  const seen = new Set<string>()
  const out: Actor[] = []
  for (const s of detail.value.subscribers || []) {
    const a: Actor = s.subscriber || s
    const key = `${a.kind}/${a.id}`
    if (!seen.has(key)) {
      seen.add(key)
      out.push(a)
    }
  }
  if (detail.value.assignee === 'worker' && detail.value.assignee_id) {
    const key = `worker/${detail.value.assignee_id}`
    if (!seen.has(key)) out.push({ kind: 'worker', id: detail.value.assignee_id })
  }
  return out
})

// 集群 worker 节点（指派/派发下拉；集群不可用时空列表 → 回退手输）。
interface ClusterNode {
  id: string
  name: string
  role: string
  online: boolean
}
const workerNodes = ref<ClusterNode[]>([])

const detailAssignType = ref('')
const detailAssignId = ref('')
const dispatchTarget = ref('')

const canDispatch = computed(
  () =>
    !!detail.value &&
    ['backlog', 'todo', 'in_progress', 'in_review'].includes(detail.value.status),
)

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

function assigneeLabel(issue: Issue): string {
  if (!issue.assignee) return '未指派'
  return issue.assignee === 'manager_self' ? 'manager（本机）' : `worker: ${issue.assignee_id}`
}

function allowedTargets(status: string): string[] {
  return TRANSITIONS[status] || []
}

// 评论线程（一层）：顶层评论 + 各自的回复。
const topLevelComments = computed<CommentRow[]>(() =>
  (detail.value?.comments || []).filter((c) => !c.parent_id),
)

function repliesOf(parentId: number): CommentRow[] {
  return (detail.value?.comments || []).filter((c) => c.parent_id === parentId)
}

async function loadWorkerNodes() {
  try {
    const r = await request('cluster', 'nodes.list', {})
    workerNodes.value = (r?.nodes || []).filter((n: any) => n.role === 'worker')
  } catch {
    workerNodes.value = []
  }
}

async function loadDetail(silent = false) {
  if (!props.issueId) return
  try {
    const r = await request('board', 'issue.get', { id: props.issueId })
    detail.value = r?.issue || null
    if (!silent) {
      // 仅显式加载重置指派下拉/派发目标；后台静默换新不动用户正在编辑的表单态。
      detailAssignType.value = ''
      detailAssignId.value = ''
      dispatchTarget.value =
        detail.value?.assignee === 'worker' ? detail.value?.assignee_id || '' : ''
    }
  } catch (e: any) {
    if (!silent) {
      toast.error('加载详情失败: ' + e)
      emit('close')
    }
  }
}

async function loadAttachments() {
  if (!props.issueId) return
  try {
    const r = await request('board', 'attachment.list', { issue_id: props.issueId })
    attachments.value = r?.attachments || []
  } catch {
    attachments.value = []
  }
}

watch(
  () => props.issueId,
  (id) => {
    if (id != null) {
      detail.value = null
      newComment.value = ''
      replyTo.value = null
      attachments.value = []
      loadDetail()
      loadAttachments()
      loadWorkerNodes()
    } else {
      // 关闭（父组件置空 issueId）→ 清 detail 让根 v-if="detail" 收起弹层。
      detail.value = null
    }
  },
  { immediate: true },
)

// 看板数据变化推送（W2.5 SSE board-changed）：弹窗打开期间外部写入
// （CLI 跨进程 / 其他标签页 / 集群回调 / autopilot）→ 详情静默换新，
// 不闪 loading、失败不打扰。自己的写操作也会触发一次（幂等 GET，无害）。
useBoardChanged(() => {
  if (props.issueId != null && detail.value) {
    loadDetail(true)
    loadAttachments()
  }
})

function changed() {
  emit('changed')
}

async function transition(to: string) {
  if (!detail.value || busy.value) return
  busy.value = true
  try {
    await request('board', 'issue.status', { id: detail.value.id, status: to })
    toast.success(`状态已改为「${statusLabel(to)}」`)
    await loadDetail()
    changed()
  } catch (e: any) {
    toast.error('状态转移失败: ' + e)
  } finally {
    busy.value = false
  }
}

async function assign(clear: boolean) {
  if (!detail.value || busy.value) return
  busy.value = true
  try {
    const payload: any = { id: detail.value.id }
    if (!clear) {
      payload.assignee_type = detailAssignType.value
      payload.assignee_id =
        detailAssignType.value === 'manager_self' ? 'local' : detailAssignId.value.trim()
    }
    await request('board', 'issue.assign', payload)
    toast.success(clear ? '已清空指派' : '已指派')
    detailAssignType.value = ''
    detailAssignId.value = ''
    await loadDetail()
    changed()
  } catch (e: any) {
    toast.error('指派失败: ' + e)
  } finally {
    busy.value = false
  }
}

function insertMention(a: Actor) {
  const token = a.kind === 'worker' ? a.id : `${a.id}`
  const cur = newComment.value
  newComment.value = cur && !cur.endsWith(' ') ? `${cur} @${token} ` : `${cur}@${token} `
}

function startReply(c: CommentRow) {
  replyTo.value = c
  // 回复自带 @ 作者前缀（后端 extract_mentions 产生 mentioned 通知）。
  const token = `@${c.author.id} `
  if (!newComment.value.startsWith(token)) {
    newComment.value = token + newComment.value
  }
}

function cancelReply() {
  replyTo.value = null
  newComment.value = newComment.value.replace(/^@\S+\s*/, '')
}

async function submitComment() {
  if (!detail.value || !newComment.value.trim()) return
  busy.value = true
  try {
    const payload: any = {
      issue_id: detail.value.id,
      content: newComment.value.trim(),
    }
    if (replyTo.value) payload.parent_id = replyTo.value.id
    await request('board', 'comment.add', payload)
    newComment.value = ''
    replyTo.value = null
    await loadDetail()
    changed()
  } catch (e: any) {
    toast.error('评论失败: ' + e)
  } finally {
    busy.value = false
  }
}

async function updatePriority(p: number) {
  if (!detail.value) return
  try {
    await request('board', 'issue.update', { id: detail.value.id, priority: p })
    toast.success('优先级已更新')
    await loadDetail()
    changed()
  } catch (e: any) {
    toast.error('更新失败: ' + e)
  }
}

async function dispatchIssue() {
  if (!detail.value || !dispatchTarget.value || busy.value) return
  busy.value = true
  try {
    const r = await request('board', 'issue.dispatch', {
      id: detail.value.id,
      target: dispatchTarget.value,
    })
    toast.success(`已派发（task ${(r?.task_id || '').slice(0, 8)}…），等 worker 回报后自动写回`)
    dispatchTarget.value = ''
    await loadDetail()
    changed()
  } catch (e: any) {
    toast.error('派发失败: ' + e)
  } finally {
    busy.value = false
  }
}

// --- 附件（W2 P3）---
function onFilePick(ev: Event) {
  const input = ev.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = '' // 允许重复选同一文件
  if (!file || !detail.value) return
  if (file.size > MAX_ATTACHMENT_BYTES) {
    toast.error(`附件过大（上限 ${fmtSize(MAX_ATTACHMENT_BYTES)}）`)
    return
  }
  uploadAttachment(file)
}

async function uploadAttachment(file: File) {
  if (!detail.value || busy.value) return
  busy.value = true
  try {
    const buf = await file.arrayBuffer()
    let binary = ''
    const bytes = new Uint8Array(buf)
    for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i])
    const content = btoa(binary)
    await request('board', 'attachment.add', {
      issue_id: detail.value.id,
      filename: file.name,
      content,
    })
    toast.success(`已上传 ${file.name}`)
    await loadAttachments()
  } catch (e: any) {
    toast.error('上传失败: ' + e)
  } finally {
    busy.value = false
  }
}

async function downloadAttachment(a: AttachmentRow) {
  try {
    const r = await request('board', 'attachment.get', { id: a.id })
    const b64: string = r?.content || ''
    const bin = atob(b64)
    const bytes = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
    const url = URL.createObjectURL(new Blob([bytes]))
    const link = document.createElement('a')
    link.href = url
    link.download = a.filename
    link.click()
    URL.revokeObjectURL(url)
  } catch (e: any) {
    toast.error('下载失败: ' + e)
  }
}
</script>

<template>
  <div v-if="detail" class="modal-backdrop" @click.self="emit('close')">
    <div class="modal" style="max-width: 720px;">
      <div class="modal-header">
        <h3><code>{{ detail.number }}</code> {{ detail.title }}</h3>
      </div>
      <div class="modal-body">
        <!-- 元信息 -->
        <div class="detail-meta">
          <span class="badge" :class="STATUS_BADGE[detail.status]">{{ statusLabel(detail.status) }}</span>
          <span class="badge" :class="PRIORITY_BADGE[detail.priority] || 'badge-neutral'">P{{ detail.priority }} {{ PRIORITY_LABEL[detail.priority] || '' }}</span>
          <span class="muted">指派：{{ assigneeLabel(detail) }}</span>
          <span class="muted">创建者：{{ detail.creator.kind }}/{{ detail.creator.id }}</span>
          <span class="muted" v-if="detail.origin">来源：{{ detail.origin.origin_type }}/{{ detail.origin.origin_id }}</span>
          <span class="muted">更新：{{ fmtTime(detail.updated_at) }}</span>
        </div>

        <!-- 状态转移 -->
        <div class="form-group" v-if="allowedTargets(detail.status).length">
          <label class="form-label">状态转移</label>
          <div class="assign-row">
            <button
              v-for="t in allowedTargets(detail.status)"
              :key="t"
              class="btn btn-sm"
              :disabled="busy"
              @click="transition(t)"
            >→ {{ statusLabel(t) }}</button>
          </div>
        </div>
        <div class="form-group" v-else>
          <p class="muted">终态 issue 不可再转移。</p>
        </div>

        <!-- 优先级 -->
        <div class="form-group">
          <label class="form-label">优先级</label>
          <div class="assign-row">
            <button
              v-for="(_, p) in PRIORITY_LABEL"
              :key="p"
              class="btn btn-sm"
              :class="{ 'btn-primary': Number(p) === detail.priority }"
              :disabled="busy"
              @click="updatePriority(Number(p))"
            >P{{ p }} {{ PRIORITY_LABEL[Number(p)] }}</button>
          </div>
        </div>

        <!-- 指派 -->
        <div class="form-group">
          <label class="form-label">指派</label>
          <div class="assign-row">
            <select v-model="detailAssignType" class="form-select" style="max-width: 180px;">
              <option value="">选择…</option>
              <option value="manager_self">manager（本机）</option>
              <option value="worker">worker 节点</option>
            </select>
            <select
              v-if="detailAssignType === 'worker' && workerNodes.length"
              class="form-select"
              v-model="detailAssignId"
              style="max-width: 260px;"
            >
              <option value="">选择 worker…</option>
              <option v-for="n in workerNodes" :key="n.id" :value="n.id">{{ `${n.name} (${n.id})${n.online ? '' : ' · 离线'}` }}</option>
            </select>
            <input
              v-else-if="detailAssignType === 'worker'"
              class="form-input"
              v-model="detailAssignId"
              placeholder="节点 id"
              style="max-width: 200px;"
            />
            <button class="btn btn-sm" :disabled="busy || !detailAssignType" @click="assign(false)">指派</button>
            <button class="btn btn-sm btn-ghost" :disabled="busy || !detail.assignee" @click="assign(true)">清空</button>
          </div>
        </div>

        <!-- 派发执行（W2 P2 派发链路） -->
        <div class="form-group" v-if="canDispatch">
          <label class="form-label">派发执行</label>
          <div class="assign-row">
            <select
              class="form-select"
              v-model="dispatchTarget"
              style="max-width: 260px;"
            >
              <option value="">选择 worker…</option>
              <option v-for="n in workerNodes" :key="n.id" :value="n.id">{{ `${n.name} (${n.id})${n.online ? '' : ' · 离线'}` }}</option>
            </select>
            <button
              class="btn btn-sm btn-primary"
              :disabled="busy || !dispatchTarget"
              title="把 issue 作为任务派给 worker 执行，结果自动写回评论"
              @click="dispatchIssue"
            >派发 →</button>
            <span class="muted">派发后 issue 转进行中；worker 回报 → 自动写结果评论并转评审中</span>
          </div>
        </div>

        <!-- 描述 / 验收 -->
        <div class="form-group" v-if="detail.description">
          <label class="form-label">描述</label>
          <pre class="detail-pre">{{ detail.description }}</pre>
        </div>
        <div class="form-group" v-if="detail.acceptance_criteria">
          <label class="form-label">验收标准</label>
          <pre class="detail-pre">{{ detail.acceptance_criteria }}</pre>
        </div>

        <!-- 评论（W2 P3：线程 + @提及） -->
        <div class="form-group">
          <label class="form-label">评论（{{ topLevelComments.length }}）</label>
          <div v-if="topLevelComments.length === 0" class="muted" style="padding: var(--space-2) 0;">暂无评论</div>
          <div v-for="c in topLevelComments" :key="c.id" class="comment-item">
            <div class="comment-head">
              <strong>{{ c.author.kind }}/{{ c.author.id }}</strong>
              <span v-if="c.ctype !== 'comment'" class="badge badge-neutral">{{ c.ctype }}</span>
              <span class="muted">{{ fmtTime(c.created_at) }}</span>
              <button v-if="c.ctype === 'comment'" class="btn btn-xs btn-ghost" @click="startReply(c)">回复</button>
            </div>
            <div class="comment-body">{{ c.content }}</div>
            <!-- 一层回复 -->
            <div v-for="r in repliesOf(c.id)" :key="r.id" class="comment-item reply-item">
              <div class="comment-head">
                <strong>{{ r.author.kind }}/{{ r.author.id }}</strong>
                <span class="muted">{{ fmtTime(r.created_at) }}</span>
              </div>
              <div class="comment-body">{{ r.content }}</div>
            </div>
          </div>
          <div v-if="replyTo" class="replying-hint">
            回复 {{ replyTo.author.kind }}/{{ replyTo.author.id }}
            <button class="btn btn-xs btn-ghost" @click="cancelReply">取消</button>
          </div>
          <div v-if="mentionCandidates.length" class="mention-row">
            <span class="muted">@提及：</span>
            <button
              v-for="a in mentionCandidates"
              :key="`${a.kind}/${a.id}`"
              class="btn btn-xs btn-ghost"
              :title="`插入 @${a.id}`"
              @click="insertMention(a)"
            >@{{ a.id }}</button>
          </div>
          <textarea class="form-textarea" v-model="newComment" style="min-height: 60px; margin-top: var(--space-2);" placeholder="添加评论…（@节点id 可提及）"></textarea>
          <button class="btn btn-sm btn-primary" style="margin-top: var(--space-2);" :disabled="busy || !newComment.trim()" @click="submitComment">发表评论</button>
        </div>

        <!-- 附件（W2 P3） -->
        <div class="form-group">
          <label class="form-label">附件（{{ attachments.length }}）</label>
          <div v-if="attachments.length === 0" class="muted" style="padding: var(--space-2) 0;">暂无附件</div>
          <div v-for="a in attachments" :key="a.id" class="attachment-item">
            <span class="attachment-name">{{ a.filename }}</span>
            <span class="muted">{{ fmtSize(a.size) }} · {{ fmtTime(a.created_at) }}</span>
            <button class="btn btn-xs" @click="downloadAttachment(a)">下载</button>
          </div>
          <label class="btn btn-sm" style="margin-top: var(--space-2); cursor: pointer;">
            上传附件
            <input type="file" style="display: none;" @change="onFilePick" />
          </label>
          <span class="muted" style="margin-left: var(--space-2);">≤ 8 MB</span>
        </div>

        <!-- 活动时间线 -->
        <div class="form-group">
          <label class="form-label">活动</label>
          <div v-if="(detail.activity || []).length === 0" class="muted">暂无活动</div>
          <div v-for="a in (detail.activity as ActivityRow[])" :key="a.id" class="activity-item">
            <span class="muted">{{ fmtTime(a.created_at) }}</span>
            <span><strong>{{ a.actor.kind }}/{{ a.actor.id }}</strong> {{ a.action }}</span>
            <span v-if="a.details" class="muted activity-details">{{ a.details }}</span>
          </div>
        </div>
      </div>
      <div class="modal-footer">
        <button class="btn" @click="emit('close')">关闭</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.muted {
  color: var(--text-muted);
  font-size: var(--text-sm);
}
.assign-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
}
.detail-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-4);
}
.detail-pre {
  white-space: pre-wrap;
  background: var(--bg-secondary);
  border-radius: var(--radius-md);
  padding: var(--space-3);
  font-size: var(--text-sm);
  max-height: 200px;
  overflow-y: auto;
}
.comment-item {
  border-left: 2px solid var(--border-color, var(--border));
  padding: var(--space-2) var(--space-3);
  margin-bottom: var(--space-2);
}
.reply-item {
  margin-top: var(--space-2);
  margin-bottom: 0;
}
.comment-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-sm);
  margin-bottom: var(--space-1);
}
.comment-body {
  font-size: var(--text-sm);
  white-space: pre-wrap;
}
.replying-hint {
  font-size: var(--text-sm);
  color: var(--text-muted);
  margin-top: var(--space-2);
  display: flex;
  align-items: center;
  gap: var(--space-2);
}
.mention-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--space-1);
  margin-top: var(--space-2);
  font-size: var(--text-sm);
}
.attachment-item {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-sm);
  padding: var(--space-1) 0;
}
.attachment-name {
  font-weight: 500;
}
.activity-item {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  font-size: var(--text-sm);
  padding: var(--space-1) 0;
}
.activity-details {
  word-break: break-all;
}
</style>
