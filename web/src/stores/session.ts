/**
 * Session store — Dashboard multi-session state.
 *
 * Single source of truth for the conversation list + the currently active
 * conversation id. ChatPanel watches `currentId` and does `chatStore.reset()`
 * + `loadHistory()` on change — this store only flips the id (keeps the data
 * flow unidirectional). Modeled on `stores/workflow.ts`.
 */

import { defineStore } from 'pinia'
import { ref } from 'vue'
import { useChatApi, type SessionEntry, type ProjectInfo } from '../composables/useChatApi'
import { useToast } from '../composables/useToast'
import { useChatStore } from './chat'

export const useSessionStore = defineStore('session', () => {
  const api = useChatApi()

  const sessions = ref<SessionEntry[]>([])
  const currentId = ref<string | null>(null)
  const listLoading = ref(false)
  const listError = ref<string | null>(null)
  const lastListFetch = ref(0)

  // L6++（2026-09-08）：项目分组单一数据源（侧栏组头 + ChatPanel chip 共用）。
  // 拉取失败静默——项目区退化为仅「＋新建项目」入口，操作错误由后端权威
  // 回显，不与 sessions 列表的错误横幅互相污染。
  const projects = ref<ProjectInfo[]>([])
  const lastProjectsFetch = ref(0)

  async function fetchProjects(force = false) {
    if (!force && Date.now() - lastProjectsFetch.value < 5000 && projects.value.length > 0) {
      return
    }
    try {
      const resp = await api.listProjects()
      projects.value = resp.projects ?? []
      lastProjectsFetch.value = Date.now()
    } catch {
      // 保持既有 projects（可能为空）；操作时后端错误原文会 toast。
    }
  }

  /** 新建项目。失败抛出（调用方 toast 错误原文，modal 不关）。 */
  async function createProject(name: string, path: string): Promise<ProjectInfo> {
    const resp = await api.createProject(name, path)
    projects.value = [...projects.value, resp.project]
    lastProjectsFetch.value = Date.now()
    return resp.project
  }

  /** 仅解除分组（后端不删任何文件）；其会话由侧栏派生为「已移除」灰组。 */
  async function removeProject(projectId: string): Promise<void> {
    await api.removeProject(projectId)
    projects.value = projects.value.filter(p => p.id !== projectId)
  }

  async function renameProject(projectId: string, name: string): Promise<void> {
    const resp = await api.renameProject(projectId, name)
    const p = projects.value.find(x => x.id === projectId)
    if (p) p.name = resp.project.name
  }

  /** projectId → 显示名（注册表已无此 pid = 已移除 → null，不误导）。 */
  function projectNameOf(projectId: string): string | null {
    return projects.value.find(p => p.id === projectId)?.name ?? null
  }

  async function fetchList(force = false) {
    if (listLoading.value) return
    // Cache for 5s unless forced — saves a round-trip on re-entry.
    if (!force && Date.now() - lastListFetch.value < 5000 && sessions.value.length > 0) {
      return
    }
    listLoading.value = true
    listError.value = null
    try {
      const resp = await api.list()
      sessions.value = resp.sessions ?? []
      lastListFetch.value = Date.now()
    } catch (e) {
      listError.value = typeof e === 'string' ? e : '加载会话列表失败'
    } finally {
      listLoading.value = false
    }
  }

  async function create(title?: string, projectId?: string): Promise<string | null> {
    try {
      const resp = await api.create(title, projectId)
      const sid = resp.session_id
      // Optimistically insert at top — the server-side file materializes on
      // the first message, so this row is editable immediately. L6++：带
      // projectId 的乐观行直接落对应项目组（等 list 刷新前即可见）。
      sessions.value.unshift({
        id: sid,
        channel: 'web',
        startTime: new Date().toISOString(),
        lastTime: new Date().toISOString(),
        messageCount: 0,
        firstMessage: resp.title || '新对话',
        model: '',
        ...(projectId ? { projectId } : {}),
      })
      switchTo(sid)
      return sid
    } catch {
      return null
    }
  }

  async function rename(session_id: string, title: string) {
    try {
      await api.rename(session_id, title)
      const s = sessions.value.find(x => x.id === session_id)
      if (s) s.title = title
    } catch {
      // caller may surface a toast
    }
  }

  async function clear(session_id: string) {
    try {
      await api.clear(session_id)
      // Clear the visible chat too if it's the active conversation.
      if (currentId.value === session_id) {
        const chatStore = useChatStore()
        chatStore.reset()
      }
    } catch {
      // caller may surface a toast
    }
  }

  async function exportSession(session_id: string) {
    return await api.export(session_id)
  }

  /** P2（2026-09-11）：本地清零未送达徽标 + 通知后端清 sidecar 计数。
   *  ChatPanel 拉取历史后调用（读到内容即视为已送达）。 */
  async function markDelivered(session_id: string) {
    const s = sessions.value.find(x => x.id === session_id)
    const had = (s?.undelivered ?? 0) > 0
    if (s) s.undelivered = 0
    if (had) {
      try {
        await api.markDelivered(session_id)
      } catch {
        // 后端清零失败只影响下次 list 的徽标精度，不阻塞阅读。
      }
    }
  }

  async function remove(session_id: string) {
    try {
      const res = await api.delete(session_id)
      // 2026-08-25: the backend disables cron jobs bound to the deleted
      // session (they would otherwise fire on — and resurrect — it).
      // Surface that visibly, or the user just wonders why the job stopped.
      const paused = res.paused_cron_jobs ?? []
      if (paused.length > 0) {
        const names = paused.map(j => j.name).join('、')
        useToast().warn(`已停用绑定该会话的定时任务：${names}（可在任务页重新启用）`, 8000)
      }
      sessions.value = sessions.value.filter(s => s.id !== session_id)
      // If we deleted the active one, fall back to the first remaining (or null).
      if (currentId.value === session_id) {
        currentId.value = sessions.value[0]?.id ?? null
      }
    } catch {
      // caller may surface a toast
    }
  }

  function switchTo(session_id: string) {
    if (currentId.value === session_id) return
    currentId.value = session_id
    // ChatPanel watches currentId → chatStore.reset() + loadHistory().
  }

  // Multi-session sidebar visibility — toggled from a button in ChatPanel's
  // input area (like the toolbar-toggle). Default hidden so the chat column
  // isn't permanently narrowed.
  const showSidebar = ref(false)
  function toggleSidebar() {
    showSidebar.value = !showSidebar.value
  }

  return { sessions, currentId, listLoading, listError, showSidebar, projects, fetchList, fetchProjects, create, createProject, removeProject, renameProject, projectNameOf, rename, clear, exportSession, markDelivered, remove, switchTo, toggleSidebar }
})
