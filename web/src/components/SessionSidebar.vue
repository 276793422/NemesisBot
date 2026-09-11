<script setup lang="ts">
/**
 * Session sidebar — conversation list for the Dashboard chat page.
 * Reads/writes `useSessionStore`; selecting a row flips `currentId`,
 * which ChatPanel watches to reset + reload that conversation's history.
 * UI conventions follow `components/logs/SessionList.vue` (selected highlight,
 * relative time, first-message-as-title).
 *
 * 2026-09-08 三块布局改版：顶部功能按钮区（新建对话 / 定时 / Skills /
 * 工作区）→ 中部项目区（项目=目录节点，组头折叠/计数/就地新建/溢出菜单）
 * → 底部对话区（纯用户↔AI 会话）→ 末尾「已移除」灰组。会话行的全部操作
 * 收进 hover 显现的「⋯」菜单（替代旧的 6 个常驻按钮）；项目行菜单含
 * 打开目录（projects.open_dir，后端只放行注册表已知路径）。
 *
 * L6++（2026-09-08）：对话/项目双分组。分组纯视图属性（会话行渲染与
 * 交互逻辑单源复用，组只是聚合键）：对话组=无 projectId；项目组=按
 * projectId 聚合；孤儿组=projectId 在注册表已不存在的会话 → 隐式
 * 「已移除」灰组（纯前端派生，零后端状态：可浏览/可删，不可新建——
 * 发送由后端诚实报错）。归属不可变投影：会话行没有「移动到别的项目」操作。
 */
import { onMounted, computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useSessionStore } from '../stores/session'
import { useToast } from '../composables/useToast'
import { useChatApi } from '../composables/useChatApi'
import { useFileTreePanel } from '../composables/useFileTreePanel'
// M5 (2026-09-05): 会话用量小字（sessions.list 回填的 tokens/cost）——
// 格式化与 ChatPanel 常驻条共用同一 helper。
import { fmtUsageLine as usage } from '../composables/useUsageFormat'
import ForkSessionModal from './ForkSessionModal.vue'
import ProjectCreateModal from './ProjectCreateModal.vue'
import type { SessionEntry } from '../composables/useChatApi'

const sessionStore = useSessionStore()
const toast = useToast()
const router = useRouter()
const { openProjectDir } = useChatApi()
const fileTree = useFileTreePanel()

// ---------------------------------------------------------------------------
// 顶部功能区：新建对话 / 定时任务 / Skills / 工作区（文件树开合开关）
// ---------------------------------------------------------------------------

async function newChat() {
  const sid = await sessionStore.create()
  if (!sid) toast.error('新建会话失败')
}

function gotoPage(path: string) {
  router.push(path)
}

// ---------------------------------------------------------------------------
// M6（devtool-upgrade 阶段 7）：会话 pin。纯前端——pinned 集合存
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

function togglePin(id: string) {
  closeMenus()
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

/** 组内排序：pinned 置顶（互相对序=列表原序不变），其余按当前排序键。 */
function sortSessions(list: SessionEntry[]): SessionEntry[] {
  return [
    ...list.filter(s => pinnedIds.value.has(s.id)),
    ...list.filter(s => !pinnedIds.value.has(s.id)).sort(cmpSessions),
  ]
}

// ---------------------------------------------------------------------------
// 会话排序键（纯前端偏好，localStorage 持久；sessions.list 行自带
// startTime/lastTime，零后端改动）：recent=最后活动新→旧（默认）/
// created=创建时间新→旧 / name=标题 zh-CN 字典序。入口=对话区标题条
// 循环按钮。
// ---------------------------------------------------------------------------
type SessionSort = 'recent' | 'created' | 'name'
const SORT_KEY = 'nb_session_sort'
const SORT_LABELS: Record<SessionSort, string> = { recent: '最近', created: '创建', name: '名称' }

function loadSort(): SessionSort {
  const v = localStorage.getItem(SORT_KEY)
  return v === 'created' || v === 'name' ? v : 'recent'
}

const sessionSort = ref<SessionSort>(loadSort())

function cycleSort() {
  const order: SessionSort[] = ['recent', 'created', 'name']
  sessionSort.value = order[(order.indexOf(sessionSort.value) + 1) % order.length]
  localStorage.setItem(SORT_KEY, sessionSort.value)
}

function cmpSessions(a: SessionEntry, b: SessionEntry): number {
  if (sessionSort.value === 'name') {
    return title(a).localeCompare(title(b), 'zh-CN')
  }
  const key = sessionSort.value === 'created' ? 'startTime' : 'lastTime'
  const ta = a[key] || ''
  const tb = b[key] || ''
  if (ta === tb) return 0 // 含双方皆空 → 稳定原序
  return ta > tb ? -1 : 1 // 降序（新→旧）；空值自然沉底
}

// ---------------------------------------------------------------------------
// B（2026-09-08 验收轮）：项目组置顶。与 M6 会话 pin 同构——纯前端偏好，
// localStorage `nb_pinned_projects` 持久；置顶项目组排在项目区最前
// （组间相对序保持注册表序不变）。移除项目成功后顺带清 pin，防残留 id。
// ---------------------------------------------------------------------------
const PROJ_PIN_KEY = 'nb_pinned_projects'

function loadPinnedProjects(): Set<string> {
  try {
    const raw = JSON.parse(localStorage.getItem(PROJ_PIN_KEY) || '[]')
    return new Set(Array.isArray(raw) ? raw.filter((x: unknown) => typeof x === 'string') : [])
  } catch {
    return new Set()
  }
}

const pinnedProjectIds = ref<Set<string>>(loadPinnedProjects())

function persistPinnedProjects() {
  localStorage.setItem(PROJ_PIN_KEY, JSON.stringify([...pinnedProjectIds.value]))
}

function isProjectPinned(key: string): boolean {
  return pinnedProjectIds.value.has(key)
}

function toggleProjectPin(key: string) {
  closeMenus()
  if (pinnedProjectIds.value.has(key)) {
    pinnedProjectIds.value.delete(key)
  } else {
    pinnedProjectIds.value.add(key)
  }
  // 换新 Set 触发 displayGroups 重算（同 M6 注释）。
  pinnedProjectIds.value = new Set(pinnedProjectIds.value)
  persistPinnedProjects()
}

/** 移除项目成功后清 pin（注册表已无此 id，残留只会占位）。 */
function unpinProjectIfRemoved(key: string) {
  if (!pinnedProjectIds.value.has(key)) return
  pinnedProjectIds.value.delete(key)
  pinnedProjectIds.value = new Set(pinnedProjectIds.value)
  persistPinnedProjects()
}

// ---------------------------------------------------------------------------
// L6++：双分组派生。项目区（按注册表聚合，每个注册项目一个组头，含空
// 项目）/ 对话区（无 projectId）/ 孤儿组（注册表已无此 pid 的残留绑定
// ——「已移除」灰组，删完全部会话后自然消失）。展示顺序：项目在上、
// 对话在下、孤儿垫底。
// ---------------------------------------------------------------------------
const chatSessions = computed(() => sortSessions(sessionStore.sessions.filter(s => !s.projectId)))

const orphanSessions = computed(() => {
  const known = new Set(sessionStore.projects.map(p => p.id))
  return sortSessions(sessionStore.sessions.filter(s => s.projectId && !known.has(s.projectId)))
})

interface GroupRow {
  /** 折叠持久化与菜单/重命名寻址键。 */
  key: string
  header: { name: string; available: boolean; orphan: boolean } | null
  items: SessionEntry[]
}

/** 展示顺序：置顶项目组（组内保持注册序）→ 未置顶项目组（注册序）→
 *  对话组 → 孤儿组（垫底）。对话组无组头——区块标题条由模板按 key
 *  注入（项目区标题条在循环外，含新建入口）。 */
const displayGroups = computed<GroupRow[]>(() => {
  const all: GroupRow[] = sessionStore.projects.map(p => ({
    key: p.id,
    header: { name: p.name, available: p.running !== false, orphan: false },
    items: sortSessions(sessionStore.sessions.filter(s => s.projectId === p.id)),
  }))
  const rows = [
    ...all.filter(g => pinnedProjectIds.value.has(g.key)),
    ...all.filter(g => !pinnedProjectIds.value.has(g.key)),
  ]
  rows.push({ key: '__chat', header: null, items: chatSessions.value })
  if (orphanSessions.value.length > 0) {
    rows.push({ key: '__orphan', header: { name: '已移除', available: false, orphan: true }, items: orphanSessions.value })
  }
  return rows
})

// 组折叠：localStorage 持久（F-14 重启恢复）。
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

// ---------------------------------------------------------------------------
// C（2026-09-08 验收轮）：组内会话折叠展示。每组默认只渲染前
// SHOW_MORE_CAP 条（sortSessions 已置顶优先，pinned 天然占先），尾部
// 「显示更多 (N)」展开 / 收起。纯视图状态（不持久化，重启回默认收拢），
// expandedGroups 按组 key 记忆（含 __chat / __orphan）。
// ---------------------------------------------------------------------------
const SHOW_MORE_CAP = 8
const expandedGroups = ref<Set<string>>(new Set())

function isExpanded(key: string): boolean {
  return expandedGroups.value.has(key)
}

function toggleExpanded(key: string) {
  const next = new Set(expandedGroups.value)
  if (next.has(key)) {
    next.delete(key)
  } else {
    next.add(key)
  }
  expandedGroups.value = next
}

/** 组内可见行：未展开时截前 SHOW_MORE_CAP 条（计数用 grp.items 全量）。 */
function visibleItems(grp: GroupRow): SessionEntry[] {
  return isExpanded(grp.key) ? grp.items : grp.items.slice(0, SHOW_MORE_CAP)
}

// ---------------------------------------------------------------------------
// 溢出菜单（单开互斥）：项目行菜单 + 会话行菜单共用一个开槽。
// ---------------------------------------------------------------------------
type MenuRef = { kind: 'group' | 'row'; key: string } | null
const menu = ref<MenuRef>(null)

function toggleGroupMenu(key: string) {
  menu.value = menu.value?.kind === 'group' && menu.value.key === key ? null : { kind: 'group', key }
}

function toggleRowMenu(id: string) {
  menu.value = menu.value?.kind === 'row' && menu.value.key === id ? null : { kind: 'row', key: id }
}

function closeMenus() {
  menu.value = null
}

// 组头行内重命名。
const renaming = ref<string | null>(null)
const renameBuf = ref('')

function startRename(key: string, currentName: string) {
  closeMenus()
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
  closeMenus()
  if (!confirm(`移除项目「${name}」？仅解除分组，不删除会话与项目目录内的任何文件。`)) return
  try {
    await sessionStore.removeProject(key)
    unpinProjectIfRemoved(key)
  } catch (e: any) {
    toast.error(typeof e === 'string' ? e : e?.message || '移除失败')
  }
}

/** 在系统文件管理器中打开项目目录（后端只放行注册表已知路径）。 */
async function openDir(key: string) {
  closeMenus()
  try {
    await openProjectDir(key)
  } catch (e: any) {
    toast.error(typeof e === 'string' ? e : e?.message || '打开目录失败')
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

/** F2/F5：项目组内就地新建会话——创建时唯一表达归属的时刻（此后上行
 *  只带 session_id，归属由服务端裁决）。不可用组不渲染入口。 */
async function newChatIn(projectId: string) {
  closeMenus()
  const sid = await sessionStore.create(undefined, projectId)
  if (!sid) toast.error('新建会话失败')
}

async function del(id: string) {
  closeMenus()
  if (!confirm('删除这个会话？历史不可恢复。')) return
  unpinIfDeleted(id)
  await sessionStore.remove(id)
}

async function renameSession(s: { id: string; title?: string; firstMessage: string }) {
  closeMenus()
  const name = prompt('会话名称', s.title || s.firstMessage || '')
  if (name === null) return
  const trimmed = name.trim()
  if (!trimmed) return
  await sessionStore.rename(s.id, trimmed)
}

async function clearSession(s: { id: string; title?: string; firstMessage: string }) {
  closeMenus()
  if (!confirm(`清空「${s.title || s.firstMessage || s.id}」的所有消息？会话保留，历史清空。`)) return
  await sessionStore.clear(s.id)
}

async function exportSession(s: { id: string; title?: string; firstMessage: string }) {
  closeMenus()
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
function forkSession(s: { id: string; title?: string; firstMessage: string }) {
  closeMenus()
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

/** P2（2026-09-11）：未送达徽标文案——无未读返回 null（不渲染），
 *  超过 9 显示 9+。集中一处，模板免 undefined 收窄问题。 */
function undeliveredBadge(s: { undelivered?: number }): string | null {
  const n = s.undelivered ?? 0
  if (n <= 0) return null
  return n > 9 ? '9+' : String(n)
}

// ---------------------------------------------------------------------------
// 会话信息（行菜单）：sessions.list 已回填的静态信息只读展示——归属/
// 时间/消息数/用量/模型/fork 血缘。零额外请求（消息数即 list 的
// messageCount；逐轮明细在 Dashboard 日志视图）。
// ---------------------------------------------------------------------------

const infoSession = ref<SessionEntry | null>(null)

function showInfo(s: SessionEntry) {
  closeMenus()
  infoSession.value = s
}

function projectOf(s: SessionEntry): string {
  if (!s.projectId) return '对话'
  return sessionStore.projects.find(p => p.id === s.projectId)?.name ?? '已移除'
}

function fmtTime(ts?: string): string {
  if (!ts) return '—'
  const d = new Date(ts)
  return isNaN(d.getTime()) ? ts : d.toLocaleString('zh-CN', { hour12: false })
}

function fmtTokens(n?: number): string {
  return n == null ? '—' : n.toLocaleString('zh-CN')
}

function fmtCost(c?: number): string {
  return c == null ? '—' : `$${c.toFixed(4)}`
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
    <!-- 顶部功能区：新建对话 + 定时 / Skills / 工作区 -->
    <div class="sidebar-functions">
      <button class="fn-new-chat" @click="newChat" title="新对话">＋ 新建对话</button>
      <div class="fn-row">
        <button class="fn-btn" @click="gotoPage('/tasks')" title="定时任务">⏱ 定时</button>
        <button class="fn-btn" @click="gotoPage('/skills')" title="Skills">⬡ Skills</button>
        <button
          class="fn-btn"
          :class="{ on: !fileTree.collapsed.value }"
          @click="fileTree.toggle()"
          title="工作区文件树"
        >📁 工作区</button>
      </div>
    </div>

    <div class="session-list" @click="closeMenus">
      <!-- 项目区标题条（循环外；零项目时入口仍可达，F1） -->
      <div class="section-bar proj-section-bar">
        <span class="section-label">项目</span>
        <span class="section-spacer" />
        <button class="project-create-btn" @click="showProjectModal = true" title="新建项目">＋</button>
      </div>
      <div v-if="sessionStore.projects.length === 0" class="section-empty">暂无项目——点「＋」注册一个目录</div>

      <template v-for="grp in displayGroups" :key="grp.key">
        <!-- 对话组前注入区块标题条（对话组无组头；⇅ 为排序循环按钮） -->
        <div v-if="grp.key === '__chat'" class="section-bar">
          <span class="section-label">对话</span>
          <span class="section-spacer" />
          <button
            class="sort-toggle"
            @click.stop="cycleSort"
            :title="`会话排序：${SORT_LABELS[sessionSort]}（点击切换 最近 → 创建 → 名称）`"
          >⇅ {{ SORT_LABELS[sessionSort] }}</button>
        </div>
        <!-- 组头：折叠 / 名称 / 计数 / 就地新建 / 溢出菜单 -->
        <div v-else-if="grp.header" class="group-header" :class="{ unavailable: !grp.header.available, orphan: grp.header.orphan }">
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
            <span v-if="isProjectPinned(grp.key)" class="pin-flag" title="已置顶">📌</span>
            <span class="group-name" :title="grp.header.name">{{ grp.header.name }}</span>
            <span
              v-if="!grp.header.available"
              class="group-warn"
              :title="grp.header.orphan ? '项目已移除（仅解除分组，文件未删）' : '目录不可用'"
            >⚠</span>
            <span class="group-count">({{ grp.items.length }})</span>
          </template>
          <span class="group-spacer" />
          <span v-if="!grp.header.orphan" class="group-actions" @click.stop>
            <button
              v-if="grp.header.available"
              class="add-in-group"
              @click="newChatIn(grp.key)"
              :title="`在「${grp.header.name}」新建会话`"
            >＋</button>
            <button class="group-more" @click="toggleGroupMenu(grp.key)" title="项目菜单">⋯</button>
          </span>
          <div v-if="menu?.kind === 'group' && menu.key === grp.key" class="group-menu" @click.stop>
            <button v-if="grp.header.available" class="menu-item" @click="newChatIn(grp.key)">＋ 新建会话</button>
            <button class="menu-item" @click="openDir(grp.key)">📂 打开目录</button>
            <button class="menu-item" @click="startRename(grp.key, grp.header.name)">✏ 重命名</button>
            <button class="menu-item" @click="toggleProjectPin(grp.key)">{{ isProjectPinned(grp.key) ? '取消置顶' : '置顶项目' }}</button>
            <button class="menu-item danger" @click="removeProject(grp.key, grp.header.name)">移除项目</button>
          </div>
        </div>

        <!-- 会话行（对话组 / 项目组 / 孤儿组单源复用；折叠时不渲染） -->
        <template v-if="!isCollapsed(grp.key)">
          <div
            v-for="s in visibleItems(grp)"
            :key="s.id"
            class="session-item"
            :class="{ active: s.id === currentId, orphaned: grp.header?.orphan }"
            @click="select(s.id)"
          >
            <div class="session-title">
              <!-- M6: pinned 会话标题前缀标记（置顶行的视觉识别） -->
              <span v-if="isPinned(s.id)" class="pin-flag" title="已置顶">📌</span>
              {{ title(s) }}
              <!-- P2（2026-09-11）: 未送达回复徽标（assistant 推送失败时后端
                   打点；点开拉取历史后清零）。 -->
              <span
                v-if="undeliveredBadge(s)"
                class="undelivered-badge"
                :title="`有未送达的回复（已落盘，打开即见）`"
              >{{ undeliveredBadge(s) }}</span>
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
              <button class="row-more" @click.stop="toggleRowMenu(s.id)" title="会话菜单">⋯</button>
            </div>
            <div v-if="menu?.kind === 'row' && menu.key === s.id" class="group-menu row-menu" @click.stop>
              <button class="menu-item" @click="showInfo(s)">ℹ 会话信息</button>
              <button class="menu-item" @click="togglePin(s.id)">{{ isPinned(s.id) ? '取消置顶' : '置顶' }}</button>
              <button class="menu-item" @click="renameSession(s)">✏ 重命名</button>
              <button class="menu-item" @click="forkSession(s)">⑂ 创建分支</button>
              <button class="menu-item" @click="clearSession(s)">🗑 清空消息</button>
              <button class="menu-item" @click="exportSession(s)">📥 导出</button>
              <button class="menu-item danger" @click="del(s.id)">✕ 删除会话</button>
            </div>
          </div>
          <!-- C：组内超出 SHOW_MORE_CAP 条时的展开/收起 footer（计数=全量-已见） -->
          <button
            v-if="grp.items.length > SHOW_MORE_CAP"
            class="show-more"
            @click.stop="toggleExpanded(grp.key)"
          >{{ isExpanded(grp.key) ? '收起' : `显示更多 (${grp.items.length - SHOW_MORE_CAP})` }}</button>
        </template>
        <!-- 对话区空态（区块内提示，替代整列表 .empty） -->
        <div
          v-if="grp.key === '__chat' && chatSessions.length === 0 && !sessionStore.listLoading"
          class="section-empty"
        >暂无会话，点上方「＋ 新建对话」开始</div>
      </template>
    </div>

    <!-- 会话信息只读弹窗（行菜单） -->
    <div v-if="infoSession" class="modal-backdrop" @click.self="infoSession = null">
      <div class="modal-box info-box">
        <h3>会话信息</h3>
        <div class="info-grid">
          <span class="info-k">标题</span><span class="info-v">{{ title(infoSession) }}</span>
          <span class="info-k">归属</span><span class="info-v">{{ projectOf(infoSession) }}</span>
          <span class="info-k">创建时间</span><span class="info-v">{{ fmtTime(infoSession.startTime) }}</span>
          <span class="info-k">最后活动</span><span class="info-v">{{ fmtTime(infoSession.lastTime) }}</span>
          <span class="info-k">消息数</span><span class="info-v">{{ infoSession.messageCount }}</span>
          <span class="info-k">Tokens</span><span class="info-v">{{ fmtTokens(infoSession.tokens) }}</span>
          <span class="info-k">费用</span><span class="info-v">{{ fmtCost(infoSession.cost) }}</span>
          <span class="info-k">模型</span><span class="info-v">{{ infoSession.model || '—' }}</span>
          <template v-if="infoSession.parent">
            <span class="info-k">父分支</span>
            <span class="info-v">{{ infoSession.parentTitle || parentSid(infoSession) }}<template v-if="infoSession.forkedAtTurn"> · 第 {{ infoSession.forkedAtTurn }} 轮</template></span>
          </template>
        </div>
        <div class="info-actions">
          <button class="info-close" @click="infoSession = null">关闭</button>
        </div>
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
/* 顶部功能区 */
.sidebar-functions {
  padding: 10px;
  border-bottom: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.fn-new-chat {
  width: 100%;
  padding: 8px 10px;
  font-size: 13px;
  font-weight: 600;
  border: 1px solid var(--accent);
  border-radius: 6px;
  background: transparent;
  color: var(--accent);
  cursor: pointer;
}
.fn-new-chat:hover {
  background: var(--accent-muted);
}
.fn-row {
  display: flex;
  gap: 6px;
}
.fn-btn {
  flex: 1;
  padding: 5px 4px;
  font-size: 12px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  white-space: nowrap;
}
.fn-btn:hover {
  color: var(--text);
  background: var(--bg-primary);
}
.fn-btn.on {
  color: var(--accent);
  border-color: var(--accent);
}
.session-list {
  flex: 1;
  overflow-y: auto;
  padding: 6px;
}
/* 区块标题条（项目 / 对话） */
.section-bar {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 10px 6px 4px;
  margin-top: 4px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-muted);
}
.proj-section-bar {
  border-top: none;
  margin-top: 0;
}
.section-spacer {
  flex: 1;
}
.project-create-btn {
  padding: 0 8px;
  font-size: 14px;
  line-height: 20px;
  border: none;
  background: transparent;
  color: var(--accent);
  cursor: pointer;
  border-radius: 4px;
}
.project-create-btn:hover {
  background: var(--accent-muted);
}
.section-empty {
  padding: 4px 8px 8px;
  color: var(--text-muted);
  font-size: 12px;
}
.session-item {
  padding: 10px;
  border-radius: 6px;
  cursor: pointer;
  margin-bottom: 4px;
  border-left: 3px solid transparent;
  position: relative;
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
  padding-right: 18px;
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
/* 会话行「⋯」：hover 显现（触屏/键盘可聚焦），active 行常显 */
.row-more {
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 0 6px;
  font-size: 14px;
  line-height: 1;
  border-radius: 4px;
  opacity: 0;
}
.session-item:hover .row-more,
.session-item.active .row-more,
.row-more:focus-visible {
  opacity: 1;
}
.row-more:hover {
  color: var(--text);
  background: var(--accent-muted);
}
/* M6: 会话 pin——置顶标记（入口在行菜单；B 轮项目组头复用同一标记） */
.pin-flag {
  font-size: 10px;
  margin-right: 2px;
}
/* P2（2026-09-11）: 未送达回复徽标——标题右侧小圆点计数 */
.undelivered-badge {
  margin-left: 6px;
  min-width: 14px;
  height: 14px;
  padding: 0 4px;
  border-radius: 7px;
  background: var(--accent, #e5484d);
  color: #fff;
  font-size: 10px;
  line-height: 14px;
  font-weight: 600;
  text-align: center;
  display: inline-block;
  vertical-align: 1px;
}
/* A：对话区排序循环按钮（⇅ + 当前档位，档位见 SORT_LABELS） */
.sort-toggle {
  background: none;
  border: none;
  color: var(--text-muted);
  font-size: 11px;
  cursor: pointer;
  padding: 1px 6px;
  border-radius: 4px;
  white-space: nowrap;
}
.sort-toggle:hover {
  color: var(--text);
  background: var(--bg-primary);
}
/* C：组内「显示更多 (N) / 收起」footer */
.show-more {
  display: block;
  width: 100%;
  margin: 2px 0 6px;
  padding: 4px 0;
  font-size: 11px;
  color: var(--text-muted);
  background: none;
  border: none;
  border-radius: 6px;
  cursor: pointer;
  text-align: center;
}
.show-more:hover {
  color: var(--text);
  background: var(--bg-primary);
}
.empty {
  padding: 20px 12px;
  color: var(--text-muted);
  font-size: 13px;
  text-align: center;
}
/* L6++：组头 */
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
.group-spacer {
  flex: 1;
}
.group-actions {
  display: flex;
  align-items: center;
  opacity: 0;
}
.group-header:hover .group-actions,
.group-actions:focus-within {
  opacity: 1;
}
.add-in-group {
  background: none;
  border: none;
  color: var(--accent);
  cursor: pointer;
  padding: 0 4px;
  font-size: 12px;
  line-height: 1;
}
.group-more {
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 0 4px;
  font-size: 14px;
  line-height: 1;
}
.group-more:hover,
.add-in-group:hover {
  color: var(--accent);
}
.group-menu {
  position: absolute;
  top: 100%;
  right: 0;
  z-index: 30;
  min-width: 120px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.25);
  overflow: hidden;
}
.group-menu .menu-item {
  display: block;
  width: 100%;
  padding: 6px 10px;
  border: none;
  background: none;
  text-align: left;
  font-size: 12px;
  color: inherit;
  cursor: pointer;
  white-space: nowrap;
}
.group-menu .menu-item:hover {
  background: var(--bg-primary);
}
.group-menu .menu-item.danger:hover {
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
/* 重命名进行中：spacer/动作钮让位，输入框占满组头（否则 flex:1 平分自由空间只剩 ~90px 宽） */
.rename-input ~ .group-spacer,
.rename-input ~ .group-actions {
  display: none;
}
/* 会话信息弹窗：遮罩用全局 .modal-backdrop（fixed 全视口+居中）。
   勿用 class="modal"——components.css 的全局 .modal 是弹窗盒子样式
   （width:90%/max-width:540px），会污染遮罩层把全视口遮罩缩成 540×720 小方块。 */
.modal-box {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 16px;
  min-width: 320px;
  max-width: 460px;
  max-height: 80vh;
  overflow-y: auto;
}
.modal-box h3 {
  margin: 0 0 12px;
  font-size: 14px;
}
.info-grid {
  display: grid;
  grid-template-columns: 72px 1fr;
  gap: 6px 10px;
  font-size: 12px;
}
.info-k {
  color: var(--text-muted);
}
.info-v {
  word-break: break-all;
}
.info-actions {
  margin-top: 14px;
  text-align: right;
}
.info-close {
  padding: 4px 14px;
  font-size: 12px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.info-close:hover {
  background: var(--bg-primary);
}
</style>
