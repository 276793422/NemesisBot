<script setup lang="ts">
import { ref } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'

// T6 (U20): cross-session full-text search over session_logs (FTS5,
// CJK-bigram aware). Backend: logs.history_search / logs.history_reindex.
// session_key in hits is the jsonl file stem — exactly what
// logs.session_detail (SessionExplorer.selectSession) expects, so the
// locate button needs no key conversion.

interface HistoryHit {
  session_key: string
  seq: number
  role: string
  timestamp: string
  snippet: string
}

const emit = defineEmits<{
  (e: 'locate', sessionKey: string): void
}>()

const { request } = useWSAPI()

const query = ref('')
const limit = ref(20)
const hits = ref<HistoryHit[]>([])
const searched = ref(false)
const loading = ref(false)
const reindexing = ref(false)
const error = ref<string | null>(null)
const reindexInfo = ref<string | null>(null)

async function doSearch() {
  const q = query.value.trim()
  if (!q || loading.value) return
  loading.value = true
  error.value = null
  try {
    const res = await request('logs', 'history_search', { query: q, limit: limit.value })
    hits.value = res?.hits ?? []
    searched.value = true
  } catch (e: any) {
    error.value = String(e?.message ?? e)
    hits.value = []
  } finally {
    loading.value = false
  }
}

async function doReindex() {
  if (reindexing.value) return
  reindexing.value = true
  error.value = null
  try {
    const res = await request('logs', 'history_reindex', {})
    const n = res?.reindexed_sessions ?? 0
    reindexInfo.value = n > 0 ? `已重建 ${n} 个会话的索引` : '索引已是最新'
  } catch (e: any) {
    error.value = String(e?.message ?? e)
  } finally {
    reindexing.value = false
  }
}

function fmtFull(ts: string): string {
  const d = new Date(ts)
  if (isNaN(d.getTime())) return ts
  const p = (n: number) => n.toString().padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`
}

function roleIcon(role: string): string {
  return role === 'assistant' ? '🤖' : role === 'system' ? '⚙️' : '👤'
}

function locate(hit: HistoryHit) {
  emit('locate', hit.session_key)
}
</script>

<template>
  <div class="history-view">
    <!-- 查询条 -->
    <div class="history-toolbar">
      <input
        v-model="query"
        class="form-input history-input"
        type="text"
        placeholder="跨会话检索关键词（中英文均可，匹配消息原文）…"
        @keyup.enter="doSearch"
      />
      <select v-model.number="limit" class="form-select history-limit">
        <option :value="10">10 条</option>
        <option :value="20">20 条</option>
        <option :value="50">50 条</option>
        <option :value="100">100 条</option>
      </select>
      <button class="btn btn-sm btn-primary" :disabled="loading || !query.trim()" @click="doSearch">
        {{ loading ? '⟳ 检索中…' : '🔍 检索' }}
      </button>
      <button
        class="btn btn-sm btn-ghost"
        :disabled="reindexing"
        :title="reindexInfo ?? '增量重建全文索引（按文件改动跳过未变化的会话）'"
        @click="doReindex"
      >
        {{ reindexing ? '⟳ 重建中…' : '♻ 重建索引' }}
      </button>
      <span v-if="reindexInfo && !reindexing" class="reindex-info">{{ reindexInfo }}</span>
    </div>

    <div v-if="error" class="history-error">检索失败：{{ error }}</div>

    <!-- 结果表 -->
    <div class="history-table-wrap scrollable">
      <table>
        <thead>
          <tr>
            <th style="width: 170px;">时间</th>
            <th style="width: 70px;">角色</th>
            <th style="width: 220px;">会话</th>
            <th>命中片段</th>
            <th style="width: 80px;">定位</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(hit, i) in hits" :key="`${hit.session_key}-${hit.seq}-${i}`">
            <td class="cell-time">{{ fmtFull(hit.timestamp) }}</td>
            <td class="cell-role">{{ roleIcon(hit.role) }} {{ hit.role }}</td>
            <td class="cell-session" :title="hit.session_key">{{ hit.session_key }}</td>
            <td class="cell-snippet">{{ hit.snippet }}</td>
            <td>
              <button class="btn btn-sm btn-ghost" @click="locate(hit)">➤ 会话</button>
            </td>
          </tr>
          <tr v-if="hits.length === 0 && !loading">
            <td colspan="5" class="empty-state">
              <p v-if="!searched">输入关键词检索全部会话历史（FTS5 全文索引，支持中文）</p>
              <p v-else>没有命中「{{ query }}」的消息</p>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>

<style scoped>
.history-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-primary);
}

.history-toolbar {
  display: flex;
  gap: var(--space-2);
  padding: var(--space-3) var(--space-4);
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-light);
  align-items: center;
  flex-wrap: wrap;
}

.history-input {
  flex: 1;
  min-width: 240px;
}

.history-limit {
  width: auto;
}

.reindex-info {
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.history-error {
  padding: var(--space-2) var(--space-4);
  font-size: var(--text-sm);
  color: #ef4444;
  background: rgba(239, 68, 68, 0.08);
  border-bottom: 1px solid rgba(239, 68, 68, 0.2);
}

.history-table-wrap {
  flex: 1;
  overflow: auto;
}

table {
  width: 100%;
  border-collapse: collapse;
}

th, td {
  padding: var(--space-2) var(--space-3);
  text-align: left;
  border-bottom: 1px solid var(--border-light);
  font-size: var(--text-sm);
  vertical-align: top;
}

th {
  background: var(--bg-secondary);
  font-weight: 600;
  font-size: var(--text-xs);
  text-transform: uppercase;
  color: var(--text-muted);
  position: sticky;
  top: 0;
  z-index: 1;
}

tbody tr:hover { background: var(--bg-hover); }

.cell-time {
  font-family: monospace;
  font-size: var(--text-xs);
  color: var(--text-muted);
  white-space: nowrap;
}

.cell-role {
  font-size: var(--text-xs);
  white-space: nowrap;
}

.cell-session {
  font-family: monospace;
  font-size: var(--text-xs);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 220px;
}

.cell-snippet {
  font-size: var(--text-sm);
  white-space: pre-wrap;
  word-break: break-word;
}

.scrollable { overflow-y: auto; }
</style>
