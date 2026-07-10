<script setup lang="ts">
/**
 * Session sidebar — ChatGPT-style conversation list for the Dashboard chat
 * page. Reads/writes `useSessionStore`; selecting a row flips `currentId`,
 * which ChatPanel watches to reset + reload that conversation's history.
 * UI conventions follow `components/logs/SessionList.vue` (selected highlight,
 * relative time, first-message-as-title).
 */
import { onMounted, computed } from 'vue'
import { useSessionStore } from '../stores/session'
import { useToast } from '../composables/useToast'

const sessionStore = useSessionStore()
const toast = useToast()
const sessions = computed(() => sessionStore.sessions)
const currentId = computed(() => sessionStore.currentId)

onMounted(async () => {
  // Refresh the list when the sidebar opens (5s cache in fetchList).
  // Auto-select of the default session is handled in ChatView (always
  // mounted) — NOT here, because this onMounted only fires when the sidebar
  // opens, too late for the initial dashboard load.
  await sessionStore.fetchList()
})

function select(id: string) {
  sessionStore.switchTo(id)
}

async function newChat() {
  const sid = await sessionStore.create()
  if (!sid) toast.error('新建会话失败')
}

async function del(id: string, e: Event) {
  e.stopPropagation()
  if (!confirm('删除这个会话？历史不可恢复。')) return
  await sessionStore.remove(id)
}

async function renameSession(s: { id: string; title?: string; firstMessage: string }, e: Event) {
  e.stopPropagation()
  const name = prompt('会话名称', s.title || s.firstMessage || '')
  if (name === null) return
  const trimmed = name.trim()
  if (!trimmed) return
  await sessionStore.rename(s.id, trimmed)
}

async function clearSession(s: { id: string; title?: string; firstMessage: string }, e: Event) {
  e.stopPropagation()
  if (!confirm(`清空「${s.title || s.firstMessage || s.id}」的所有消息？会话保留，历史清空。`)) return
  await sessionStore.clear(s.id)
}

async function exportSession(s: { id: string; title?: string; firstMessage: string }, e: Event) {
  e.stopPropagation()
  try {
    const resp = await sessionStore.exportSession(s.id)
    const blob = new Blob([JSON.stringify(resp.messages, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `session-${s.id}.json`
    a.click()
    URL.revokeObjectURL(url)
  } catch {
    toast.error('导出失败')
  }
}

function title(s: { title?: string; firstMessage: string; id: string }): string {
  return s.title || s.firstMessage || s.id.slice(0, 8)
}

function relTime(ts: string): string {
  if (!ts) return ''
  const d = new Date(ts)
  const diff = (Date.now() - d.getTime()) / 1000
  if (diff < 60) return '刚刚'
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`
  return d.toLocaleDateString('zh-CN')
}
</script>

<template>
  <div class="session-sidebar">
    <div class="sidebar-header">
      <span>会话</span>
      <button class="new-btn" @click="newChat" title="新对话">+ 新建</button>
    </div>
    <div class="session-list">
      <div
        v-for="s in sessions"
        :key="s.id"
        class="session-item"
        :class="{ active: s.id === currentId }"
        @click="select(s.id)"
      >
        <div class="session-title">{{ title(s) }}</div>
        <div class="session-meta">
          <span>{{ relTime(s.lastTime || s.startTime) }}</span>
          <button class="del-btn" @click="renameSession(s, $event)" title="重命名">✏</button>
          <button class="del-btn" @click="clearSession(s, $event)" title="清空消息">🗑</button>
          <button class="del-btn" @click="exportSession(s, $event)" title="导出">📥</button>
          <button class="del-btn" @click="del(s.id, $event)" title="删除会话">×</button>
        </div>
      </div>
      <div v-if="sessions.length === 0 && !sessionStore.listLoading" class="empty">
        暂无会话，点击「新建」开始
      </div>
    </div>
  </div>
</template>

<style scoped>
.session-sidebar {
  width: 260px;
  min-width: 260px;
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  background: var(--surface);
  height: 100%;
}
.sidebar-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px;
  border-bottom: 1px solid var(--border);
  font-weight: 600;
}
.new-btn {
  padding: 4px 10px;
  font-size: 12px;
  border: 1px solid var(--accent);
  border-radius: 4px;
  background: transparent;
  color: var(--accent);
  cursor: pointer;
}
.new-btn:hover {
  background: var(--accent-muted);
}
.session-list {
  flex: 1;
  overflow-y: auto;
  padding: 6px;
}
.session-item {
  padding: 10px;
  border-radius: 6px;
  cursor: pointer;
  margin-bottom: 4px;
  border-left: 3px solid transparent;
}
.session-item:hover {
  background: var(--bg-primary);
}
.session-item.active {
  background: var(--accent-muted);
  border-left-color: var(--accent);
}
.session-title {
  font-size: 13px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  margin-bottom: 4px;
}
.session-meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11px;
  color: var(--text-muted);
}
.del-btn {
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 0 4px;
  font-size: 16px;
  line-height: 1;
}
.del-btn:hover {
  color: #dc3545;
}
.empty {
  padding: 20px 12px;
  color: var(--text-muted);
  font-size: 13px;
  text-align: center;
}
</style>
