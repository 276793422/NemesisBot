<script setup lang="ts">
// 会话回收站（Logs Dashboard 第 3 页签，2026-10-30）：sessions.trash_*
// 四命令的只读+恢复视图。进入页签（组件挂载，v-else-if 链）即拉取列表。
//
// 恢复冲突流（拍板②）：无 new_sid 恢复若撞同 id 存活会话，后端回结构化
// `{conflict:true, suggested_sid}`（条目不消耗）→ 弹改名提示模态，输入框
// 预填建议 sid（`{原sid}-restored-{ms}`），确认后带 new_sid 重试。
//
// 彻底删除（单条）与清空回收站都过二次确认；purge 不可逆——移入站的
// 文件直接消失。TTL 90 天自动清扫（logs.trash 配置，0=关）在 sweeper。
import { ref, onMounted } from 'vue'
import { useChatApi } from '../../composables/useChatApi'
import { useToast } from '../../composables/useToast'

// 条目数上报（LogsTabs 页签角标）。
const emit = defineEmits<{ (e: 'counted', n: number): void }>()

const api = useChatApi()
const toast = useToast()

interface TrashRow {
  entry: string
  original_session_key: string
  display_id: string
  title: string | null
  deleted_at_ms: number
  message_count: number
  bytes: number
  source: string
  orphan: boolean
}

const entries = ref<TrashRow[]>([])
const loading = ref(false)
const loadError = ref('')

const SOURCE_LABELS: Record<string, string> = {
  single: '单个删除',
  batch: '批量删除',
  clear: '清空消息',
  unknown: '未知来源',
}

function rowTitle(r: TrashRow): string {
  return r.title || r.display_id || r.entry
}

function fmtTime(ms: number): string {
  if (!ms) return '—'
  const d = new Date(ms)
  return isNaN(d.getTime()) ? String(ms) : d.toLocaleString('zh-CN', { hour12: false })
}

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

async function load() {
  loading.value = true
  loadError.value = ''
  try {
    const res = await api.trashList()
    entries.value = Array.isArray(res?.entries) ? res.entries : []
    emit('counted', entries.value.length)
  } catch (e: any) {
    loadError.value = String(e?.message ?? e)
  } finally {
    loading.value = false
  }
}

// ---------------------------------------------------------------------------
// 恢复（含拍板② 冲突改名流）
// ---------------------------------------------------------------------------

const conflictTarget = ref<TrashRow | null>(null)
const conflictSidBuf = ref('')
const restoreBusy = ref(false)

async function doRestore(r: TrashRow, newSid?: string) {
  restoreBusy.value = true
  try {
    const res = (await api.trashRestore(r.entry, newSid)) as Record<string, unknown>
    if (res?.conflict === true) {
      // 结构化冲突：目标 sid 已有存活会话。条目不消耗——弹改名提示，
      // 输入框预填后端建议 sid（拍板②「提示界面允许改名恢复」）。
      conflictTarget.value = r
      conflictSidBuf.value = String(res.suggested_sid ?? `${r.display_id}-restored`)
      return
    }
    const renamed = res?.renamed === true
    toast.success(renamed
      ? `已改名恢复为「${res.restored}」`
      : `已恢复「${rowTitle(r)}」`)
    conflictTarget.value = null
    await load()
  } catch (e: any) {
    toast.error('恢复失败: ' + String(e?.message ?? e))
  } finally {
    restoreBusy.value = false
  }
}

function commitConflictRestore() {
  const t = conflictTarget.value
  if (!t) return
  const sid = conflictSidBuf.value.trim()
  if (!sid) {
    toast.error('会话 ID 不能为空')
    return
  }
  void doRestore(t, sid)
}

// ---------------------------------------------------------------------------
// 彻底删除（单条 / 清空）——不可逆，二次确认
// ---------------------------------------------------------------------------

const confirmPurge = ref<TrashRow | null>(null)
const confirmPurgeAll = ref(false)
const purgeBusy = ref(false)

async function doPurge() {
  const r = confirmPurge.value
  if (!r) return
  purgeBusy.value = true
  try {
    await api.trashPurge(r.entry)
    toast.success(`已彻底删除「${rowTitle(r)}」`)
    confirmPurge.value = null
    await load()
  } catch (e: any) {
    toast.error('彻底删除失败: ' + String(e?.message ?? e))
  } finally {
    purgeBusy.value = false
  }
}

async function doPurgeAll() {
  purgeBusy.value = true
  try {
    const res = await api.trashPurgeAll()
    toast.success(`已清空回收站（${res?.purged ?? 0} 条）`)
    confirmPurgeAll.value = false
    await load()
  } catch (e: any) {
    toast.error('清空失败: ' + String(e?.message ?? e))
  } finally {
    purgeBusy.value = false
  }
}

onMounted(load)
</script>

<template>
  <div class="trash-view">
    <!-- 工具条 -->
    <div class="trash-toolbar">
      <span class="count-info">共 {{ entries.length }} 条 · 90 天后自动清除（logs.trash 可调）</span>
      <div class="filter-actions">
        <button class="btn btn-sm" data-test="trash-refresh" :disabled="loading" @click="load">刷新</button>
        <button
          v-if="entries.length > 0"
          class="btn btn-sm btn-danger"
          data-test="trash-purge-all-btn"
          :disabled="purgeBusy"
          @click="confirmPurgeAll = true"
        >清空回收站</button>
      </div>
    </div>

    <!-- 表格 -->
    <div class="trash-table-wrap">
      <div v-if="loadError" class="trash-error" data-test="trash-error">
        回收站加载失败（sessions.trash_list）— {{ loadError }}
      </div>
      <div v-else-if="!loading && entries.length === 0" class="empty-state" data-test="trash-empty">
        <p>回收站是空的。删除的会话会先移到这里（保留 90 天），可随时恢复。</p>
      </div>
      <table v-else data-test="trash-table">
        <thead>
          <tr>
            <th>标题</th>
            <th style="width: 180px;">原会话 ID</th>
            <th style="width: 160px;">删除时间</th>
            <th style="width: 90px;">条数</th>
            <th style="width: 90px;">大小</th>
            <th style="width: 100px;">来源</th>
            <th style="width: 150px;">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="r in entries" :key="r.entry" data-test="trash-row">
            <td class="cell-title" :title="r.original_session_key">
              {{ rowTitle(r) }}
              <span v-if="r.orphan" class="badge badge-warn orphan-badge" title="清单缺失（可能曾手动清理）——按目录名还原">孤儿</span>
            </td>
            <td class="cell-mono">{{ r.display_id }}</td>
            <td>{{ fmtTime(r.deleted_at_ms) }}</td>
            <td>{{ r.message_count }}</td>
            <td>{{ fmtSize(r.bytes) }}</td>
            <td><span class="badge badge-info">{{ SOURCE_LABELS[r.source] ?? r.source }}</span></td>
            <td>
              <button
                class="btn btn-sm"
                data-test="trash-restore-btn"
                :disabled="restoreBusy"
                @click="doRestore(r)"
              >恢复</button>
              <button
                class="btn btn-sm btn-danger"
                data-test="trash-purge-btn"
                :disabled="purgeBusy"
                @click="confirmPurge = r"
              >彻底删除</button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 恢复冲突改名模态（拍板②：提示界面允许改名恢复） -->
    <div v-if="conflictTarget" class="modal-backdrop" data-test="trash-conflict-modal" @click.self="conflictTarget = null">
      <div class="modal" style="max-width: 480px;">
        <div class="modal-header"><h3>会话 ID 冲突</h3></div>
        <div class="modal-body">
          <p>
            已存在同 ID 的会话「<strong>{{ conflictTarget.display_id }}</strong>」，直接恢复会覆盖它。
            改一个新 ID 恢复（回收站里的原内容不受影响）：
          </p>
          <input
            v-model="conflictSidBuf"
            class="form-input"
            data-test="trash-conflict-sid"
            placeholder="新会话 ID"
            @keyup.enter="commitConflictRestore"
          />
        </div>
        <div class="modal-footer">
          <button class="btn" data-test="trash-conflict-cancel" @click="conflictTarget = null">取消</button>
          <button
            class="btn btn-primary"
            data-test="trash-conflict-confirm"
            :disabled="restoreBusy"
            @click="commitConflictRestore"
          >改名恢复</button>
        </div>
      </div>
    </div>

    <!-- 彻底删除二次确认 -->
    <div v-if="confirmPurge" class="modal-backdrop" data-test="trash-purge-modal" @click.self="confirmPurge = null">
      <div class="modal" style="max-width: 480px;">
        <div class="modal-header"><h3>确认彻底删除</h3></div>
        <div class="modal-body">
          <p>将永久删除回收站条目「<strong>{{ rowTitle(confirmPurge) }}</strong>」的全部文件（{{ confirmPurge.message_count }} 条消息）。</p>
          <p class="danger-note">此操作不可逆——恢复入口也会随之消失。</p>
        </div>
        <div class="modal-footer">
          <button class="btn" data-test="trash-purge-cancel" @click="confirmPurge = null">取消</button>
          <button class="btn btn-danger" data-test="trash-purge-confirm" :disabled="purgeBusy" @click="doPurge">
            {{ purgeBusy ? '删除中…' : '彻底删除' }}
          </button>
        </div>
      </div>
    </div>

    <!-- 清空回收站二次确认 -->
    <div v-if="confirmPurgeAll" class="modal-backdrop" data-test="trash-purge-all-modal" @click.self="confirmPurgeAll = false">
      <div class="modal" style="max-width: 480px;">
        <div class="modal-header"><h3>确认清空回收站</h3></div>
        <div class="modal-body">
          <p>将永久删除回收站里的全部 <strong>{{ entries.length }}</strong> 条会话记录。</p>
          <p class="danger-note">此操作不可逆。</p>
        </div>
        <div class="modal-footer">
          <button class="btn" data-test="trash-purge-all-cancel" @click="confirmPurgeAll = false">取消</button>
          <button class="btn btn-danger" data-test="trash-purge-all-confirm" :disabled="purgeBusy" @click="doPurgeAll">
            {{ purgeBusy ? '清空中…' : '全部彻底删除' }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.trash-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-primary);
}

.trash-toolbar {
  display: flex;
  gap: var(--space-4);
  padding: var(--space-3) var(--space-4);
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-light);
  align-items: center;
}

.count-info {
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.filter-actions {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.trash-table-wrap {
  flex: 1;
  overflow: auto;
}

.trash-error {
  margin: var(--space-4);
  padding: var(--space-3);
  border: 1px solid rgba(239, 68, 68, 0.4);
  background: rgba(239, 68, 68, 0.08);
  border-radius: var(--radius-md);
  color: var(--text);
  font-size: var(--text-sm);
}

.cell-title {
  max-width: 320px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cell-mono {
  font-family: var(--font-mono, monospace);
  font-size: var(--text-xs);
}

.orphan-badge {
  margin-left: var(--space-1);
}

.danger-note {
  color: #ef4444;
  font-size: var(--text-sm);
}

table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-sm);
}

th, td {
  padding: var(--space-2) var(--space-3);
  text-align: left;
  border-bottom: 1px solid var(--border-light);
}

th {
  font-weight: 600;
  color: var(--text-muted);
  font-size: var(--text-xs);
  position: sticky;
  top: 0;
  background: var(--bg-primary);
}

td button + button {
  margin-left: var(--space-1);
}
</style>
