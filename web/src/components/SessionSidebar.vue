<script setup lang="ts">
/**
 * Session sidebar — ChatGPT-style conversation list with smooth animations.
 * Reads/writes `useSessionStore`; selecting a row flips `currentId`,
 * which ChatPanel watches to reset + reload that conversation's history.
 */
import { onMounted, computed } from 'vue'
import { useSessionStore } from '../stores/session'
import { useToast } from '../composables/useToast'

const sessionStore = useSessionStore()
const toast = useToast()
const sessions = computed(() => sessionStore.sessions)
const currentId = computed(() => sessionStore.currentId)

onMounted(async () => {
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
  <aside class="session-sidebar" :class="{ collapsed: !sessionStore.showSidebar }">
    <!-- Header -->
    <div class="sidebar-header">
      <span class="header-title">会话</span>
      <div class="header-actions">
        <button class="new-btn" @click="newChat" title="新对话">+ 新建</button>
        <button
          class="collapse-btn"
          :title="sessionStore.showSidebar ? '收起' : '展开'"
          @click="sessionStore.toggleSidebar()"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <line x1="18" y1="6" x2="6" y2="18" v-if="sessionStore.showSidebar"/>
            <line x1="6" y1="6" x2="18" y2="18" v-if="sessionStore.showSidebar"/>
            <polyline points="9,6 15,12 9,18" v-else/>
          </svg>
        </button>
      </div>
    </div>

    <!-- Session List -->
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
          <div class="session-actions">
            <button class="action-btn" @click="renameSession(s, $event)" title="重命名">✏</button>
            <button class="action-btn" @click="clearSession(s, $event)" title="清空消息">🗑</button>
            <button class="action-btn" @click="exportSession(s, $event)" title="导出">📥</button>
            <button class="action-btn danger" @click="del(s.id, $event)" title="删除会话">×</button>
          </div>
        </div>
      </div>
      <div v-if="sessions.length === 0 && !sessionStore.listLoading" class="empty">
        暂无会话，点击「新建」开始
      </div>
    </div>
  </aside>
</template>

<style scoped>
/* ===== Session Sidebar ===== */
.session-sidebar {
  width: 260px;
  min-width: 260px;
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  background: var(--surface);
  height: 100%;
  overflow: hidden;
  transition: width var(--duration-normal) var(--ease-out),
              min-width var(--duration-normal) var(--ease-out),
              opacity var(--duration-normal) var(--ease-out);
}

/* Collapsed state */
.session-sidebar.collapsed {
  width: 0;
  min-width: 0;
  opacity: 0;
  pointer-events: none;
}

/* ===== Header ===== */
.sidebar-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--space-3) var(--space-4);
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
  overflow: hidden;
  white-space: nowrap;
}

.header-title {
  font-size: var(--text-sm);
  font-weight: 600;
  color: var(--text);
  opacity: 1;
  transition: opacity var(--duration-fast);
}

.session-sidebar.collapsed .header-title {
  opacity: 0;
}

.header-actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.new-btn {
  padding: 4px 10px;
  font-size: 12px;
  border: 1px solid var(--accent);
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--accent);
  cursor: pointer;
  transition: all var(--duration-fast);
  white-space: nowrap;
  font-family: var(--font-sans);
  font-weight: 500;
}

.new-btn:hover {
  background: var(--accent-muted);
  transform: translateY(-1px);
}

.collapse-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--surface);
  color: var(--text-muted);
  cursor: pointer;
  transition: all var(--duration-fast);
  flex-shrink: 0;
  padding: 0;
}

.collapse-btn:hover {
  border-color: var(--accent);
  color: var(--accent);
  background: var(--accent-muted);
}

.collapse-btn svg {
  width: 14px;
  height: 14px;
}

/* ===== Session List ===== */
.session-list {
  flex: 1;
  overflow-y: auto;
  padding: var(--space-2);
  overflow-x: hidden;
}

.session-item {
  padding: var(--space-3);
  border-radius: var(--radius-md);
  cursor: pointer;
  margin-bottom: var(--space-1);
  border-left: 3px solid transparent;
  transition: all var(--duration-fast);
  position: relative;
  overflow: hidden;
}

.session-item:hover {
  background: var(--bg-primary);
  transform: translateX(2px);
}

.session-item.active {
  background: var(--accent-muted);
  border-left-color: var(--accent);
  box-shadow: 0 0 0 1px rgba(232, 112, 90, 0.1);
}

/* Stagger animation for list items */
.session-item {
  animation: slideInRight var(--duration-normal) var(--ease-out) backwards;
  animation-delay: calc(var(--index, 0) * 30ms);
}

@keyframes slideInRight {
  from {
    opacity: 0;
    transform: translateX(-20px);
  }
  to {
    opacity: 1;
    transform: translateX(0);
  }
}

.session-title {
  font-size: var(--text-sm);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  margin-bottom: var(--space-1);
  color: var(--text);
  font-weight: 500;
  transition: color var(--duration-fast);
}

.session-item.active .session-title {
  color: var(--accent);
  font-weight: 600;
}

.session-meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.session-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  opacity: 0;
  transform: translateX(8px);
  transition: all var(--duration-fast);
}

.session-item:hover .session-actions {
  opacity: 1;
  transform: translateX(0);
}

.action-btn {
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 2px 6px;
  font-size: 14px;
  line-height: 1;
  border-radius: var(--radius-sm);
  transition: all var(--duration-fast);
}

.action-btn:hover {
  color: var(--accent);
  background: var(--accent-muted);
  transform: scale(1.1);
}

.action-btn.danger:hover {
  color: var(--error);
  background: var(--error-bg);
}

.empty {
  padding: var(--space-6) var(--space-3);
  color: var(--text-muted);
  font-size: var(--text-sm);
  text-align: center;
  animation: fadeInUp var(--duration-normal) var(--ease-out);
}

/* ===== Responsive ===== */
@media (max-width: 768px) {
  .session-sidebar {
    position: fixed;
    top: 0;
    left: 0;
    bottom: 0;
    z-index: calc(var(--z-sidebar) + 1);
    box-shadow: var(--shadow-lg);
  }

  .session-sidebar.collapsed {
    transform: translateX(-100%);
    opacity: 1;
    pointer-events: none;
  }
}
</style>
