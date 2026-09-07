<script setup lang="ts">
/**
 * Session sidebar — conversation list for the Dashboard chat
 * page. Reads/writes `useSessionStore`; selecting a row flips `currentId`,
 * which ChatPanel watches to reset + reload that conversation's history.
 * UI conventions follow `components/logs/SessionList.vue` (selected highlight,
 * relative time, first-message-as-title).
 */
import { onMounted, computed, ref } from 'vue'
import { useSessionStore } from '../stores/session'
import { useToast } from '../composables/useToast'
// M5 (2026-09-05): 会话用量小字（sessions.list 回填的 tokens/cost）——
// 格式化与 ChatPanel 常驻条共用同一 helper。
import { fmtUsageLine as usage } from '../composables/useUsageFormat'
import ForkSessionModal from './ForkSessionModal.vue'

const sessionStore = useSessionStore()
const toast = useToast()

// ---------------------------------------------------------------------------
// M6（devtool-upgrade 阶段 7）：会话 pin 快速槽。纯前端——pinned 集合存
// localStorage，pinned 会话置顶显示（组内保持原排序），后端 sessions.list
// 协议不动。删除会话时顺带清 pin，防残留 id 永久占顶。
// ---------------------------------------------------------------------------
const PIN_KEY = 'nb_pinned_sessions'

function loadPinned(): Set<string> {
  try {
    const raw = JSON.parse(localStorage.getItem(PIN_KEY) || '[]')
    return new Set(Array.isArray(raw) ? raw.filter((x: unknown) => typeof x === 'string') : [])
  } catch {
    return new Set()
  }
}

const pinnedIds = ref<Set<string>>(loadPinned())

function persistPinned() {
  localStorage.setItem(PIN_KEY, JSON.stringify([...pinnedIds.value]))
}

function isPinned(id: string): boolean {
  return pinnedIds.value.has(id)
}

function togglePin(id: string, e: Event) {
  e.stopPropagation()
  if (pinnedIds.value.has(id)) {
    pinnedIds.value.delete(id)
  } else {
    pinnedIds.value.add(id)
  }
  // 换新 Set 触发 computed 重算（Set 内部变更不改变 ref 引用）。
  pinnedIds.value = new Set(pinnedIds.value)
  persistPinned()
}

function unpinIfDeleted(id: string) {
  if (!pinnedIds.value.has(id)) return
  pinnedIds.value.delete(id)
  pinnedIds.value = new Set(pinnedIds.value)
  persistPinned()
}

/** pinned 置顶、其余保持 sessions.list 原序（稳定分组）。 */
const sessions = computed(() => {
  const list = sessionStore.sessions
  return [...list.filter(s => pinnedIds.value.has(s.id)), ...list.filter(s => !pinnedIds.value.has(s.id))]
})

const currentId = computed(() => sessionStore.currentId)
// P3-1: fork dialog state (null = closed).
const forkTarget = ref<{ id: string; title: string } | null>(null)

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
  unpinIfDeleted(id)
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

/** P3-1: open the fork dialog for this session. */
function forkSession(s: { id: string; title?: string; firstMessage: string }, e: Event) {
  e.stopPropagation()
  forkTarget.value = { id: s.id, title: s.title || s.firstMessage || s.id.slice(0, 8) }
}

/** P3-1: fork done — refresh the list (force, the new session bypasses the
 * 5s cache) and switch to the new session. */
async function onForked(newSessionId: string) {
  forkTarget.value = null
  await sessionStore.fetchList(true)
  sessionStore.switchTo(newSessionId)
}

function title(s: { title?: string; firstMessage: string; id: string }): string {
  return s.title || s.firstMessage || s.id.slice(0, 8)
}

// ---------------------------------------------------------------------------
// E4 (2026-09-05): fork 血缘标记（sessions.list 回填 parent/parentTitle/
// forkedAtTurn）。父会话在列表里 → 可点击跳转；不在（已删/异源会话）→
// 纯文本标记不误导。
// ---------------------------------------------------------------------------

type ForkRef = { id: string; title?: string; firstMessage: string; parent?: string; parentTitle?: string; forkedAtTurn?: number }

/** 父会话 sid（`agent:main:session:{sid}` → `{sid}`；异形 key 原样返回）。 */
function parentSid(s: ForkRef): string {
  return s.parent?.replace(/^agent:main:session:/, '') ?? ''
}

function parentInList(s: ForkRef): boolean {
  return !!s.parent && sessionStore.sessions.some(x => x.id === parentSid(s))
}

function forkLabel(s: ForkRef): string {
  const base = s.parentTitle || parentSid(s)
  return s.forkedAtTurn ? `分叉自「${base}」· 第 ${s.forkedAtTurn} 轮` : `分叉自「${base}」`
}

function goParent(s: ForkRef) {
  if (!parentInList(s)) return
  sessionStore.switchTo(parentSid(s))
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
        <div class="session-title">
          <!-- M6: pinned 会话标题前缀标记（置顶行的视觉识别） -->
          <span v-if="isPinned(s.id)" class="pin-flag" title="已置顶">📌</span>
          {{ title(s) }}
        </div>
        <!-- E4: fork 血缘标记（父在列表可点击跳转；不在则纯文本） -->
        <div
          v-if="s.parent"
          class="session-fork-line"
          :class="{ link: parentInList(s) }"
          :title="parentInList(s) ? '跳转到父会话' : '父会话不在列表中'"
          @click.stop="goParent(s)"
        >↳ {{ forkLabel(s) }}</div>
        <!-- M5: 会话用量（tokens/cost，sessions.list 回填；无记录不占位） -->
        <div v-if="usage(s)" class="session-usage">{{ usage(s) }}</div>
        <div class="session-meta">
          <span>{{ relTime(s.lastTime || s.startTime) }}</span>
          <button class="del-btn pin-btn" :class="{ pinned: isPinned(s.id) }" @click="togglePin(s.id, $event)" :title="isPinned(s.id) ? '取消置顶' : '置顶会话'">📌</button>
          <button class="del-btn" @click="renameSession(s, $event)" title="重命名">✏</button>
          <button class="del-btn" @click="clearSession(s, $event)" title="清空消息">🗑</button>
          <button class="del-btn" @click="exportSession(s, $event)" title="导出">📥</button>
          <button class="del-btn" @click="forkSession(s, $event)" title="分叉（从某一轮另开分支）">⑂</button>
          <button class="del-btn" @click="del(s.id, $event)" title="删除会话">×</button>
        </div>
      </div>
      <div v-if="sessions.length === 0 && !sessionStore.listLoading" class="empty">
        暂无会话，点击「新建」开始
      </div>
    </div>
    <!-- P3-1: session fork dialog -->
    <ForkSessionModal
      v-if="forkTarget"
      :session-id="forkTarget.id"
      :session-title="forkTarget.title"
      @close="forkTarget = null"
      @forked="onForked"
    />
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
.session-usage {
  font-size: 11px;
  color: var(--text-muted);
  opacity: 0.85;
  margin-bottom: 4px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
/* E4: fork 血缘标记（缩进 + 弱化；父在列表时 hover 下划线示可点） */
.session-fork-line {
  font-size: 11px;
  color: var(--text-muted);
  padding-left: 8px;
  margin-bottom: 4px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.session-fork-line.link {
  cursor: pointer;
}
.session-fork-line.link:hover {
  color: var(--accent);
  text-decoration: underline;
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
/* M6: 会话 pin 快速槽——置顶标记 + pin 按钮（常驻弱化、pinned 高亮） */
.pin-flag {
  font-size: 10px;
  margin-right: 2px;
}
.pin-btn {
  font-size: 11px;
  opacity: 0.55;
}
.pin-btn.pinned {
  opacity: 1;
}
.pin-btn:hover {
  color: var(--accent);
  opacity: 1;
}
.empty {
  padding: 20px 12px;
  color: var(--text-muted);
  font-size: 13px;
  text-align: center;
}
</style>
