<script setup lang="ts">
/**
 * Session sidebar — conversation list for the Dashboard chat
 * page. Reads/writes `useSessionStore`; selecting a row flips `currentId`,
 * which ChatPanel watches to reset + reload that conversation's history.
 * UI conventions follow `components/logs/SessionList.vue` (selected highlight,
 * relative time, first-message-as-title).
 *
 * L6++（2026-09-08）：对话/项目双分组。分组纯视图属性（会话行渲染与
 * 交互逻辑单源复用，组只是聚合键）：对话组=无 projectId（行为同现状，
 * pinned 置顶）；项目组=按 projectId 聚合（组头=折叠/计数/就地新建/
 * 溢出菜单）；孤儿组=projectId 在注册表已不存在的会话 → 隐式「已移除」
 * 灰组（纯前端派生，零后端状态：可浏览/可删，不可新建——发送由后端
 * 诚实报错）。归属不可变投影：会话行没有「移动到别的项目」操作。
 */
import { onMounted, computed, ref } from 'vue'
import { useSessionStore } from '../stores/session'
import { useToast } from '../composables/useToast'
// M5 (2026-09-05): 会话用量小字（sessions.list 回填的 tokens/cost）——
// 格式化与 ChatPanel 常驻条共用同一 helper。
import { fmtUsageLine as usage } from '../composables/useUsageFormat'
import ForkSessionModal from './ForkSessionModal.vue'
import ProjectCreateModal from './ProjectCreateModal.vue'
import type { SessionEntry } from '../composables/useChatApi'

const sessionStore = useSessionStore()
const toast = useToast()

// ---------------------------------------------------------------------------
// M6（devtool-upgrade 阶段 7）：会话 pin 快速槽。纯前端——pinned 集合存
// localStorage，pinned 会话置顶显示（组内保持原排序），后端 sessions.list
// 协议不动。删除会话时顺带清 pin，防残留 id 永久占顶。
// L6++：pin 语义=组内置顶（双分组 computed 各自内部排序，互不跨越组边界）。
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

/** 组内排序：pinned 置顶、其余保持 sessions.list 原序（稳定分组）。 */
function sortPinned(list: SessionEntry[]): SessionEntry[] {
  return [...list.filter(s => pinnedIds.value.has(s.id)), ...list.filter(s => !pinnedIds.value.has(s.id))]
}

// ---------------------------------------------------------------------------
// L6++：双分组派生。对话组 / 项目组（按注册表聚合）/ 孤儿组（注册表已无
// 此 pid 的残留绑定——「已移除」灰组，删完全部会话后自然消失）。
// ---------------------------------------------------------------------------
const chatSessions = computed(() => sortPinned(sessionStore.sessions.filter(s => !s.projectId)))

const orphanSessions = computed(() => {
  const known = new Set(sessionStore.projects.map(p => p.id))
  return sortPinned(sessionStore.sessions.filter(s => s.projectId && !known.has(s.projectId)))
})

interface GroupRow {
  /** 折叠持久化与菜单/重命名寻址键。 */
  key: string
  header: { name: string; available: boolean; orphan: boolean } | null
  items: SessionEntry[]
}

const displayGroups = computed<GroupRow[]>(() => {
  const rows: GroupRow[] = [{ key: '__chat', header: null, items: chatSessions.value }]
  for (const p of sessionStore.projects) {
    rows.push({
      key: p.id,
      header: { name: p.name, available: p.running !== false, orphan: false },
      items: sortPinned(sessionStore.sessions.filter(s => s.projectId === p.id)),
    })
  }
  if (orphanSessions.value.length > 0) {
    rows.push({ key: '__orphan', header: { name: '已移除', available: false, orphan: true }, items: orphanSessions.value })
  }
  return rows
})

// 组折叠：localStorage 持久（F-14 重启恢复）。对话组无组头不可折叠。
const COLLAPSE_KEY = 'nb_collapsed_projects'

function loadCollapsed(): Set<string> {
  try {
    const raw = JSON.parse(localStorage.getItem(COLLAPSE_KEY) || '[]')
    return new Set(Array.isArray(raw) ? raw.filter((x: unknown) => typeof x === 'string') : [])
  } catch {
    return new Set()
  }
}

const collapsed = ref<Set<string>>(loadCollapsed())

function persistCollapsed() {
  localStorage.setItem(COLLAPSE_KEY, JSON.stringify([...collapsed.value]))
}

function isCollapsed(key: string): boolean {
  return collapsed.value.has(key)
}

function toggleCollapse(key: string) {
  if (collapsed.value.has(key)) {
    collapsed.value.delete(key)
  } else {
    collapsed.value.add(key)
  }
  collapsed.value = new Set(collapsed.value)
  persistCollapsed()
}

// 组头溢出菜单（单开）+ 行内重命名。
const menuFor = ref<string | null>(null)
const renaming = ref<string | null>(null)
const renameBuf = ref('')

function toggleMenu(key: string) {
  menuFor.value = menuFor.value === key ? null : key
}

function startRename(key: string, currentName: string) {
  menuFor.value = null
  renaming.value = key
  renameBuf.value = currentName
}

async function commitRename() {
  const key = renaming.value
  const name = renameBuf.value.trim()
  renaming.value = null
  if (!key || !name || key.startsWith('__')) return
  try {
    await sessionStore.renameProject(key, name)
  } catch (e: any) {
    toast.error(typeof e === 'string' ? e : e?.message || '重命名失败')
  }
}

/** F10：移除项目——确认文案钉死「仅解除分组，不删除会话与项目目录内的
 *  任何文件」；确认后组头消失，其会话落「已移除」灰组。 */
async function removeProject(key: string, name: string) {
  menuFor.value = null
  if (!confirm(`移除项目「${name}」？仅解除分组，不删除会话与项目目录内的任何文件。`)) return
  try {
    await sessionStore.removeProject(key)
  } catch (e: any) {
    toast.error(typeof e === 'string' ? e : e?.message || '移除失败')
  }
}

// 新建项目 modal（F1）。
const showProjectModal = ref(false)

const currentId = computed(() => sessionStore.currentId)
// P3-1: fork dialog state (null = closed).
const forkTarget = ref<{ id: string; title: string } | null>(null)

onMounted(async () => {
  // Refresh the list when the sidebar opens (5s cache in fetchList).
  // Auto-select of the default session is handled in ChatView (always
  // mounted) — NOT here, because this onMounted only fires when the sidebar
  // opens, too late for the initial dashboard load.
  await sessionStore.fetchList()
  // L6++：项目注册表（组头数据源；失败静默退化，见 store 注释）。
  await sessionStore.fetchProjects()
})

function select(id: string) {
  sessionStore.switchTo(id)
}

async function newChat() {
  const sid = await sessionStore.create()
  if (!sid) toast.error('新建会话失败')
}

/** F2/F5：项目组内就地新建会话——创建时唯一表达归属的时刻（此后上行
 *  只带 session_id，归属由服务端裁决）。不可用组不渲染入口。 */
async function newChatIn(projectId: string) {
  const sid = await sessionStore.create(undefined, projectId)
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
 *  5s cache) and switch to the new session. */
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
    <div class="session-list" @click="menuFor = null">
      <template v-for="grp in displayGroups" :key="grp.key">
        <!-- 组头（对话组无组头）：折叠 / 名称 / 计数 / 就地新建 / 溢出菜单 -->
        <div v-if="grp.header" class="group-header" :class="{ unavailable: !grp.header.available, orphan: grp.header.orphan }">
          <span class="caret" @click.stop="toggleCollapse(grp.key)">{{ isCollapsed(grp.key) ? '▸' : '▾' }}</span>
          <template v-if="renaming === grp.key">
            <input
              v-model="renameBuf"
              class="rename-input"
              @click.stop
              @keyup.enter="commitRename"
              @blur="commitRename"
            />
          </template>
          <template v-else>
            <span class="group-name" :title="grp.header.name">{{ grp.header.name }}</span>
            <span
              v-if="!grp.header.available"
              class="group-warn"
              :title="grp.header.orphan ? '项目已移除（仅解除分组，文件未删）' : '目录不可用'"
            >⚠</span>
            <span class="group-count">({{ grp.items.length }})</span>
          </template>
          <span class="group-actions" @click.stop>
            <button
              v-if="grp.header.available"
              class="del-btn add-in-group"
              @click="newChatIn(grp.key)"
              :title="`在「${grp.header.name}」新建会话`"
            >＋</button>
            <button
              v-if="!grp.header.orphan"
              class="del-btn"
              @click="toggleMenu(grp.key)"
              title="项目管理"
            >⋯</button>
          </span>
          <div v-if="menuFor === grp.key" class="group-menu" @click.stop>
            <button @click="startRename(grp.key, grp.header.name)">重命名</button>
            <button class="danger" @click="removeProject(grp.key, grp.header.name)">移除项目</button>
          </div>
        </div>
        <!-- 会话行（对话组 / 项目组 / 孤儿组单源复用；折叠时不渲染） -->
        <template v-if="!isCollapsed(grp.key)">
          <div
            v-for="s in grp.items"
            :key="s.id"
            class="session-item"
            :class="{ active: s.id === currentId, orphaned: grp.header?.orphan }"
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
        </template>
      </template>
      <div v-if="chatSessions.length === 0 && !sessionStore.listLoading" class="empty">
        暂无会话，点击「新建」开始
      </div>
      <!-- L6++：项目区锚 + 区底新建入口（零项目时也无组头、入口仍可达，F1） -->
      <div class="proj-section-bar">
        <span>项目</span>
      </div>
      <button class="new-btn project-create-btn" @click="showProjectModal = true" title="新建项目">＋ 新建项目</button>
    </div>
    <!-- P3-1: session fork dialog -->
    <ForkSessionModal
      v-if="forkTarget"
      :session-id="forkTarget.id"
      :session-title="forkTarget.title"
      @close="forkTarget = null"
      @forked="onForked"
    />
    <!-- L6++ F1: 新建项目弹窗 -->
    <ProjectCreateModal v-if="showProjectModal" @close="showProjectModal = false" />
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
/* L6++：孤儿组会话弱化（可浏览/可删；视觉上与活动组区分） */
.session-item.orphaned {
  opacity: 0.6;
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
/* L6++：项目区锚 + 组头 */
.proj-section-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 6px 4px;
  margin-top: 6px;
  border-top: 1px solid var(--border);
  font-size: 12px;
  font-weight: 600;
  color: var(--text-muted);
}
.project-create-btn {
  display: block;
  width: 100%;
  margin-top: 4px;
}
.group-header {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 6px 4px;
  margin-top: 2px;
  font-size: 12px;
  font-weight: 600;
  cursor: default;
  position: relative;
}
.group-header .caret {
  cursor: pointer;
  color: var(--text-muted);
  font-size: 10px;
  width: 12px;
}
.group-header .group-name {
  flex: 1;
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.group-header.unavailable .group-name {
  color: var(--text-muted);
  text-decoration: line-through;
}
.group-header.orphan .group-name {
  color: var(--text-muted);
}
.group-warn {
  color: #e0a800;
  font-size: 11px;
}
.group-count {
  color: var(--text-muted);
  font-weight: 400;
  font-size: 11px;
}
.group-actions {
  display: flex;
  align-items: center;
}
.add-in-group {
  font-size: 12px;
  color: var(--accent);
}
.group-menu {
  position: absolute;
  top: 100%;
  right: 0;
  z-index: 30;
  min-width: 96px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.25);
  overflow: hidden;
}
.group-menu button {
  display: block;
  width: 100%;
  padding: 6px 10px;
  border: none;
  background: none;
  text-align: left;
  font-size: 12px;
  color: inherit;
  cursor: pointer;
}
.group-menu button:hover {
  background: var(--bg-primary);
}
.group-menu button.danger:hover {
  color: #dc3545;
}
.rename-input {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  padding: 2px 4px;
  border: 1px solid var(--accent);
  border-radius: 3px;
  background: var(--bg-primary);
  color: inherit;
}
</style>
