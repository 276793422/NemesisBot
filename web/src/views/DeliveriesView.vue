<script setup lang="ts">
// 交付审核页（P4 人审闭环，2026-10-25）：分支/评审/档案交付的账本查询 +
// 裁决（确认/拒绝）+ PR 发布（P3 外循环 delivery.publish）。
// 数据全部来自 WSAPI：deliveries.list/patch（账本桥）+ board.delivery.publish
// （发布桥，实现在 nemesisbot 侧，走 agent loop 完整 dispatch 瀑布）。
// 列表不带 patch 体（单条可达 1MB）——详情模态按需拉 deliveries.patch。
// 裁决语义（后端状态机权威）：rejected = 终态不可再 confirm；confirm 幂等；
// 发布幂等（payload 已带 pr_number 短路回 already_published）。
import { computed, ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

interface DeliveryRow {
  id: string
  run_id: string | null
  kind: string // branch / review / dossier
  idempotency_key: string
  status: string // recorded / delivered / confirmed / rejected / published
  created_at: string
  delivered_at: string | null
  confirmed_at: string | null
  rejected_at: string | null
  reject_reason: string | null
  notify_state: string
  notify_attempts: number
  notify_last_error: string | null
  branch: string | null
  node: string | null
  files_changed: number | null
  insertions: number | null
  deletions: number | null
  patch_truncated: boolean | null
  patch_path: string | null
  patch_present: boolean
  commit: string | null
  head_tree: string | null
  base_tree: string | null
  pr_number: number | null
  pr_url: string | null
  published_at: string | null
}

interface PublishOutcome {
  task_id?: string
  status?: string // published / already_published / failed
  pr_number?: number | null
  pr_url?: string | null
  commit?: string | null
  branch?: string
  detail?: string
}

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const loadError = ref('')
const deliveries = ref<DeliveryRow[]>([])

const filterStatus = ref('all')
const filterKind = ref('all')

const STATUS_LABELS: Record<string, string> = {
  recorded: '已记账',
  delivered: '已送达',
  confirmed: '已确认',
  rejected: '已拒绝',
  published: '已发布',
}

const KIND_LABELS: Record<string, string> = {
  branch: '分支交付',
  review: '评审结论',
  dossier: '任务档案',
}

const filtered = computed(() =>
  deliveries.value.filter(
    d =>
      (filterStatus.value === 'all' || d.status === filterStatus.value) &&
      (filterKind.value === 'all' || d.kind === filterKind.value),
  ),
)

function statusBadge(s: string): { cls: string; text: string } {
  switch (s) {
    case 'confirmed': return { cls: 'badge-success', text: STATUS_LABELS[s] ?? s }
    case 'published': return { cls: 'badge-info', text: STATUS_LABELS[s] ?? s }
    case 'rejected': return { cls: 'badge-danger', text: STATUS_LABELS[s] ?? s }
    case 'delivered': return { cls: 'badge-warning', text: STATUS_LABELS[s] ?? s }
    default: return { cls: 'badge-neutral', text: STATUS_LABELS[s] ?? s }
  }
}

function kindLabel(k: string): string {
  return KIND_LABELS[k] ?? k
}

// 可裁决 = 未进终态（rejected/published 均终态；confirmed 只剩拒绝反转合法）。
function decidable(d: DeliveryRow): boolean {
  return d.status === 'recorded' || d.status === 'delivered' || d.status === 'confirmed'
}

// 可发布 = 分支交付且未发布未拒绝（recorded/delivered/confirmed）。
function publishable(d: DeliveryRow): boolean {
  return d.kind === 'branch' && d.status !== 'rejected' && d.status !== 'published'
}

function formatTs(ts: string | null): string {
  if (!ts) return '—'
  const d = new Date(ts)
  return isNaN(d.getTime()) ? ts : d.toLocaleString()
}

function errMsg(reason: unknown): string {
  if (reason == null) return ''
  if (typeof reason === 'string') return reason
  const e = reason as { message?: string }
  return e.message ?? String(reason)
}

async function refreshAll() {
  loading.value = true
  loadError.value = ''
  try {
    const res = await request('deliveries', 'list', { limit: 50 })
    deliveries.value = Array.isArray(res?.deliveries) ? res.deliveries : []
  } catch (e) {
    loadError.value = errMsg(e)
  } finally {
    loading.value = false
  }
}

onMounted(refreshAll)

// ── 确认 / 拒绝 ──────────────────────────────────────────────────────
const busyKey = ref('')
const showReject = ref(false)
const rejectKey = ref('')
const rejectReason = ref('')

function openReject(d: DeliveryRow) {
  rejectKey.value = d.idempotency_key
  rejectReason.value = ''
  showReject.value = true
}

async function submitConfirm(d: DeliveryRow) {
  if (busyKey.value) return
  busyKey.value = d.idempotency_key
  try {
    const res = await request('deliveries', 'confirm', { idempotency_key: d.idempotency_key })
    toast.success(res?.note ?? '交付已确认')
    await refreshAll()
  } catch (e) {
    toast.error(errMsg(e))
  } finally {
    busyKey.value = ''
  }
}

async function submitReject() {
  const key = rejectKey.value
  if (!key || busyKey.value) return
  busyKey.value = key
  try {
    const res = await request('deliveries', 'reject', {
      idempotency_key: key,
      reason: rejectReason.value.trim() || '(未填写原因)',
    })
    toast.success(res?.note ?? '交付已拒绝')
    showReject.value = false
    await refreshAll()
  } catch (e) {
    toast.error(errMsg(e))
  } finally {
    busyKey.value = ''
  }
}

// ── PR 发布（board.delivery.publish；发布是重活，给 10 分钟超时） ──────
const showPublish = ref(false)
const pubTaskId = ref('')
const pubConnector = ref('gitea')
const pubOwner = ref('')
const pubRepo = ref('')
const pubBase = ref('main')
const pubCopyPath = ref('')
const pubError = ref('')
const publishing = ref(false)
// 最近一次发布结果（成功/幂等/失败 都在页面诚实展示，不只靠 toast）
const lastOutcome = ref<PublishOutcome | null>(null)
const lastOutcomeKey = ref('')

function openPublish(d: DeliveryRow) {
  // branch 交付幂等键形态 = `{task_id}:branch`（P2 交付管线钉死）。
  pubTaskId.value = d.idempotency_key.replace(/:branch$/, '')
  pubConnector.value = 'gitea'
  pubOwner.value = ''
  pubRepo.value = ''
  pubBase.value = 'main'
  pubCopyPath.value = ''
  pubError.value = ''
  showPublish.value = true
}

function validatePublishForm(): string {
  if (!pubConnector.value.trim()) return '请填写 connector id'
  if (!pubOwner.value.trim()) return '请填写 owner（仓库归属者）'
  if (!pubRepo.value.trim()) return '请填写 repo（仓库名）'
  if (!pubBase.value.trim()) return '请填写 base（目标分支）'
  return ''
}

async function submitPublish() {
  pubError.value = ''
  const invalid = validatePublishForm()
  if (invalid) {
    pubError.value = invalid
    return
  }
  if (publishing.value) return
  publishing.value = true
  try {
    const data: Record<string, string> = {
      task_id: pubTaskId.value.trim(),
      connector: pubConnector.value.trim(),
      owner: pubOwner.value.trim(),
      repo: pubRepo.value.trim(),
      base: pubBase.value.trim(),
    }
    if (pubCopyPath.value.trim()) data.copy_path = pubCopyPath.value.trim()
    const outcome: PublishOutcome = await request('board', 'delivery.publish', data, 600_000)
    lastOutcome.value = outcome
    lastOutcomeKey.value = data.task_id
    if (outcome.status === 'published') {
      toast.success(`已发布：PR #${outcome.pr_number ?? '?'}`)
      showPublish.value = false
    } else if (outcome.status === 'already_published') {
      toast.success(outcome.detail ?? '该交付已发布（幂等跳过）')
      showPublish.value = false
    } else {
      pubError.value = outcome.detail ?? '发布失败（详情见响应）'
    }
    await refreshAll()
  } catch (e) {
    pubError.value = errMsg(e)
  } finally {
    publishing.value = false
  }
}

// ── 详情模态（patch 按需拉取） ────────────────────────────────────────
const showDetail = ref(false)
const detailRow = ref<DeliveryRow | null>(null)
const detailPatch = ref('')
const detailPatchError = ref('')
const detailPatchLoading = ref(false)

async function openDetail(d: DeliveryRow) {
  detailRow.value = d
  detailPatch.value = ''
  detailPatchError.value = ''
  showDetail.value = true
  if (!d.patch_present) return
  detailPatchLoading.value = true
  try {
    const res = await request('deliveries', 'patch', { idempotency_key: d.idempotency_key })
    detailPatch.value = typeof res?.patch === 'string' ? res.patch : ''
  } catch (e) {
    detailPatchError.value = errMsg(e)
  } finally {
    detailPatchLoading.value = false
  }
}
</script>

<template>
  <div class="page-deliveries">
    <div class="page-header">
      <h2>交付审核</h2>
      <span v-if="!loadError" class="count-badge">{{ filtered.length }} / {{ deliveries.length }} 条</span>
      <span style="flex: 1;"></span>
      <button class="btn btn-sm" data-test="deliveries-refresh" :disabled="loading" @click="refreshAll">刷新</button>
    </div>
    <div class="page-body">

      <div v-if="loadError" class="card">
        <div class="card-body load-error" data-test="load-error">
          无法获取交付列表（deliveries.list）— {{ loadError }}
          <button class="btn btn-sm" style="margin-left: var(--space-2);" :disabled="loading" @click="refreshAll">重试</button>
        </div>
      </div>

      <template v-else>
        <!-- 过滤器 -->
        <div class="filter-bar" data-test="filter-bar">
          <label>状态
            <select v-model="filterStatus" data-test="filter-status" class="filter-select">
              <option value="all">全部</option>
              <option v-for="(label, s) in STATUS_LABELS" :key="s" :value="s">{{ label }}</option>
            </select>
          </label>
          <label>种类
            <select v-model="filterKind" data-test="filter-kind" class="filter-select">
              <option value="all">全部</option>
              <option v-for="(label, k) in KIND_LABELS" :key="k" :value="k">{{ label }}</option>
            </select>
          </label>
          <span class="filter-hint">裁决语义：拒绝 = 终态不可撤销；确认幂等；发布走完整安全管线（高危会弹审批卡）。</span>
        </div>

        <!-- 最近一次发布结果（成功/失败都诚实留在页面上） -->
        <div
          v-if="lastOutcome"
          class="card"
          data-test="publish-outcome"
          :class="{ 'outcome-failed': lastOutcome.status === 'failed' }"
        >
          <div class="card-body outcome-body">
            <strong>
              {{ lastOutcome.status === 'published' ? `已发布 PR #${lastOutcome.pr_number}`
                : lastOutcome.status === 'already_published' ? '已发布过（幂等跳过）'
                : '发布失败' }}
            </strong>
            <a v-if="lastOutcome.pr_url" :href="lastOutcome.pr_url" target="_blank" rel="noopener" style="margin-left: var(--space-2);">{{ lastOutcome.pr_url }}</a>
            <div class="outcome-detail">{{ lastOutcome.detail }}</div>
          </div>
        </div>

        <!-- 列表 -->
        <div class="card">
          <div class="card-body">
            <div v-if="loading" style="color: var(--text-secondary); font-size: var(--text-sm);">加载中…</div>
            <div v-else-if="filtered.length === 0" style="color: var(--text-secondary); font-size: var(--text-sm);" data-test="empty-list">
              暂无交付记录。任务走交付管线（分支/评审/档案）后在这里出现。
            </div>
            <table v-else class="dl-table" data-test="dl-table">
              <thead>
                <tr>
                  <th>种类</th>
                  <th>状态</th>
                  <th>交付物</th>
                  <th>节点</th>
                  <th>变更</th>
                  <th>时间</th>
                  <th style="width: 260px;">操作</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="d in filtered" :key="d.idempotency_key" data-test="dl-row">
                  <td><span class="badge badge-neutral">{{ kindLabel(d.kind) }}</span></td>
                  <td>
                    <span class="badge" :class="statusBadge(d.status).cls" data-test="dl-status">{{ statusBadge(d.status).text }}</span>
                    <a
                      v-if="d.pr_url"
                      :href="d.pr_url"
                      target="_blank"
                      rel="noopener"
                      class="pr-link"
                      data-test="dl-pr-link"
                    >PR #{{ d.pr_number }}</a>
                  </td>
                  <td>
                    <code class="dl-key" :title="d.idempotency_key">{{ d.branch || d.idempotency_key }}</code>
                  </td>
                  <td>{{ d.node || '—' }}</td>
                  <td class="dl-diffstat">
                    <template v-if="d.files_changed != null">
                      <span class="diffstat-files">{{ d.files_changed }} 文件</span>
                      <span class="diffstat-add">+{{ d.insertions ?? 0 }}</span>
                      <span class="diffstat-del">-{{ d.deletions ?? 0 }}</span>
                    </template>
                    <template v-else>—</template>
                  </td>
                  <td class="dl-ts">{{ formatTs(d.created_at) }}</td>
                  <td class="dl-actions">
                    <button class="btn btn-sm" data-test="dl-detail" @click="openDetail(d)">详情</button>
                    <button
                      v-if="decidable(d) && d.status !== 'confirmed'"
                      class="btn btn-sm primary"
                      data-test="dl-confirm"
                      :disabled="busyKey === d.idempotency_key"
                      @click="submitConfirm(d)"
                    >确认</button>
                    <button
                      v-if="decidable(d)"
                      class="btn btn-sm danger"
                      data-test="dl-reject"
                      :disabled="busyKey === d.idempotency_key"
                      @click="openReject(d)"
                    >拒绝</button>
                    <button
                      v-if="publishable(d)"
                      class="btn btn-sm"
                      data-test="dl-publish"
                      @click="openPublish(d)"
                    >发布 PR</button>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>
      </template>

      <!-- 拒绝原因模态 -->
      <div v-if="showReject" class="modal-backdrop" data-test="reject-modal" @click.self="showReject = false">
        <div class="modal dl-modal">
          <div class="modal-header"><h3 style="margin: 0;">拒绝交付</h3></div>
          <div class="modal-body">
            <p style="margin-top: 0; font-size: var(--text-sm);">
              拒绝是<strong>终态</strong>（不可再确认或发布），原因留痕审计链。键：
              <code>{{ rejectKey }}</code>
            </p>
            <textarea
              v-model="rejectReason"
              class="dl-textarea"
              data-test="reject-reason"
              rows="3"
              placeholder="拒绝原因（留空记为「未填写原因」）"
            ></textarea>
          </div>
          <div class="modal-footer">
            <button class="btn" data-test="reject-cancel" @click="showReject = false">取消</button>
            <button class="btn danger" data-test="reject-submit" :disabled="!!busyKey" @click="submitReject">确认拒绝</button>
          </div>
        </div>
      </div>

      <!-- 发布模态（PR 参数表单） -->
      <div v-if="showPublish" class="modal-backdrop" data-test="publish-modal" @click.self="publishing ? null : (showPublish = false)">
        <div class="modal dl-modal">
          <div class="modal-header"><h3 style="margin: 0;">发布交付 PR</h3></div>
          <div class="modal-body">
            <p style="margin-top: 0; font-size: var(--text-sm);">
              从任务副本推送交付分支 <code>nemesis/task-{{ pubTaskId }}</code> 并创建 PR。
              前置核验（基线一致/patch 完整/未拒绝）在后端执行；发布幂等——重复点击不会重复建 PR。
            </p>
            <div class="form-grid">
              <label>task_id<input v-model="pubTaskId" data-test="pub-task-id" disabled></label>
              <label>connector<input v-model="pubConnector" data-test="pub-connector" placeholder="gitea"></label>
              <label>owner *<input v-model="pubOwner" data-test="pub-owner" placeholder="仓库归属者/组织"></label>
              <label>repo *<input v-model="pubRepo" data-test="pub-repo" placeholder="仓库名"></label>
              <label>base *<input v-model="pubBase" data-test="pub-base" placeholder="main"></label>
              <label>copy_path（缺省走派发链）<input v-model="pubCopyPath" data-test="pub-copy-path" placeholder="任务副本目录绝对路径（可选）"></label>
            </div>
            <div v-if="pubError" class="publish-error" data-test="publish-error">{{ pubError }}</div>
          </div>
          <div class="modal-footer">
            <button class="btn" data-test="publish-cancel" :disabled="publishing" @click="showPublish = false">取消</button>
            <button class="btn primary" data-test="publish-submit" :disabled="publishing" @click="submitPublish">
              {{ publishing ? '发布中…（走安全管线，高危操作会弹审批卡）' : '发布' }}
            </button>
          </div>
        </div>
      </div>

      <!-- 详情模态（patch 按需拉取） -->
      <div v-if="showDetail" class="modal-backdrop" data-test="detail-modal" @click.self="showDetail = false">
        <div class="modal dl-modal dl-detail-modal">
          <div class="modal-header">
            <h3 style="margin: 0;">交付详情</h3>
            <span class="badge" :class="statusBadge(detailRow?.status ?? '').cls">{{ statusBadge(detailRow?.status ?? '').text }}</span>
          </div>
          <div v-if="detailRow" class="modal-body">
            <table class="detail-table" data-test="detail-table">
              <tbody>
                <tr><th>幂等键</th><td><code>{{ detailRow.idempotency_key }}</code></td></tr>
                <tr><th>种类</th><td>{{ kindLabel(detailRow.kind) }}</td></tr>
                <tr v-if="detailRow.run_id"><th>执行 ID</th><td><code>{{ detailRow.run_id }}</code></td></tr>
                <tr v-if="detailRow.branch"><th>分支</th><td><code>{{ detailRow.branch }}</code></td></tr>
                <tr v-if="detailRow.commit"><th>commit</th><td><code>{{ detailRow.commit }}</code></td></tr>
                <tr v-if="detailRow.head_tree"><th>head tree</th><td><code>{{ detailRow.head_tree }}</code></td></tr>
                <tr v-if="detailRow.base_tree"><th>base tree</th><td><code>{{ detailRow.base_tree }}</code></td></tr>
                <tr v-if="detailRow.pr_url"><th>PR</th><td><a :href="detailRow.pr_url" target="_blank" rel="noopener">PR #{{ detailRow.pr_number }}</a>（{{ formatTs(detailRow.published_at) }}）</td></tr>
                <tr><th>创建</th><td>{{ formatTs(detailRow.created_at) }}</td></tr>
                <tr v-if="detailRow.delivered_at"><th>送达</th><td>{{ formatTs(detailRow.delivered_at) }}</td></tr>
                <tr v-if="detailRow.confirmed_at"><th>确认</th><td>{{ formatTs(detailRow.confirmed_at) }}</td></tr>
                <tr v-if="detailRow.rejected_at"><th>拒绝</th><td>{{ formatTs(detailRow.rejected_at) }} — {{ detailRow.reject_reason }}</td></tr>
                <tr><th>通知态</th><td>{{ detailRow.notify_state }}<template v-if="detailRow.notify_attempts > 0">（失败 {{ detailRow.notify_attempts }} 次<span v-if="detailRow.notify_last_error">：{{ detailRow.notify_last_error }}</span>）</template></td></tr>
                <tr v-if="detailRow.patch_truncated"><th>patch</th><td class="load-error">已截断（超 1MB 上限）——截断交付不可发布</td></tr>
                <tr v-else-if="detailRow.patch_path"><th>patch 留底</th><td><code>{{ detailRow.patch_path }}</code>（worker 侧）</td></tr>
              </tbody>
            </table>
            <template v-if="detailRow.patch_present">
              <h4 style="margin-bottom: var(--space-1);">Patch</h4>
              <div v-if="detailPatchLoading" style="color: var(--text-secondary); font-size: var(--text-sm);">加载 patch…</div>
              <div v-else-if="detailPatchError" class="load-error">{{ detailPatchError }}</div>
              <pre v-else class="patch-view" data-test="patch-view">{{ detailPatch }}</pre>
            </template>
          </div>
          <div class="modal-footer">
            <button class="btn" data-test="detail-close" @click="showDetail = false">关闭</button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.load-error {
  color: var(--error, #ef4444);
  font-size: var(--text-sm);
  word-break: break-all;
}

.count-badge {
  font-size: var(--text-sm);
  color: var(--text-secondary);
}

.filter-bar {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-3);
  flex-wrap: wrap;
  font-size: var(--text-sm);
}

.filter-select {
  margin-left: var(--space-1);
  padding: 2px 6px;
  font-size: var(--text-sm);
}

.filter-hint {
  color: var(--text-muted);
  font-size: var(--text-xs);
}

.outcome-body { font-size: var(--text-sm); }
.outcome-failed { border-color: rgba(239, 68, 68, 0.4); }
.outcome-detail {
  margin-top: var(--space-1);
  color: var(--text-secondary);
  word-break: break-all;
}

.dl-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-sm);
}

.dl-table th,
.dl-table td {
  text-align: left;
  padding: var(--space-2) var(--space-2);
  border-bottom: 1px solid var(--border);
  vertical-align: middle;
}

.dl-key {
  font-family: var(--font-mono, monospace);
  font-size: var(--text-xs);
  max-width: 260px;
  display: inline-block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  vertical-align: middle;
}

.dl-diffstat { white-space: nowrap; }
.diffstat-files { color: var(--text-secondary); margin-right: var(--space-1); }
.diffstat-add { color: #10b981; margin-right: var(--space-1); }
.diffstat-del { color: #ef4444; }

.dl-ts {
  font-family: var(--font-mono, monospace);
  font-size: var(--text-xs);
  color: var(--text-secondary);
  white-space: nowrap;
}

.dl-actions {
  display: flex;
  gap: var(--space-1);
  flex-wrap: wrap;
}

.dl-actions .btn.danger {
  color: #ef4444;
}

.pr-link { margin-left: var(--space-1); font-size: var(--text-xs); }

.modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

.dl-modal { width: 560px; max-width: 92vw; }
.dl-detail-modal { width: 760px; max-height: 86vh; overflow: auto; }

.dl-textarea {
  width: 100%;
  resize: vertical;
  font-size: var(--text-sm);
}

.form-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--space-2) var(--space-3);
  font-size: var(--text-sm);
}

.form-grid label {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.form-grid input {
  padding: 4px 8px;
  font-size: var(--text-sm);
}

.form-grid input:disabled { opacity: 0.6; }

.publish-error {
  margin-top: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border: 1px solid rgba(239, 68, 68, 0.4);
  background: rgba(239, 68, 68, 0.08);
  border-radius: var(--radius-md);
  color: var(--error, #ef4444);
  font-size: var(--text-sm);
  word-break: break-all;
}

.detail-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-sm);
  margin-bottom: var(--space-3);
}

.detail-table th {
  text-align: left;
  color: var(--text-secondary);
  font-weight: normal;
  width: 110px;
  padding: var(--space-1) var(--space-2) var(--space-1) 0;
  vertical-align: top;
}

.detail-table td {
  padding: var(--space-1) 0;
  word-break: break-all;
}

.patch-view {
  max-height: 420px;
  overflow: auto;
  padding: var(--space-3);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--bg-secondary, rgba(0, 0, 0, 0.03));
  font-family: var(--font-mono, monospace);
  font-size: var(--text-xs);
  white-space: pre;
}
</style>
