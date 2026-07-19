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
import { useChatApi, type SessionEntry } from '../composables/useChatApi'
import { useChatStore } from './chat'

export const useSessionStore = defineStore('session', () => {
  const api = useChatApi()

  const sessions = ref<SessionEntry[]>([])
  const currentId = ref<string | null>(null)
  const listLoading = ref(false)
  const listError = ref<string | null>(null)
  const lastListFetch = ref(0)

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

  async function create(): Promise<string | null> {
    try {
      const resp = await api.create()
      const sid = resp.session_id
      // Optimistically insert at top — the server-side file materializes on
      // the first message, so this row is editable immediately.
      sessions.value.unshift({
        id: sid,
        channel: 'web',
        startTime: new Date().toISOString(),
        lastTime: new Date().toISOString(),
        messageCount: 0,
        firstMessage: resp.title || '新对话',
        model: '',
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

  async function remove(session_id: string) {
    try {
      await api.delete(session_id)
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

  // Multi-session sidebar visibility — toggled from ChatPanel.
  // Default visible on desktop (≥769px) so conversations are discoverable;
  // mobile starts collapsed to preserve chat width.
  function defaultShowSidebar(): boolean {
    if (typeof window === 'undefined') return true
    try {
      return window.matchMedia('(min-width: 769px)').matches
    } catch {
      return true
    }
  }
  const showSidebar = ref(defaultShowSidebar())
  function toggleSidebar() {
    showSidebar.value = !showSidebar.value
  }

  return { sessions, currentId, listLoading, listError, showSidebar, fetchList, create, rename, clear, exportSession, remove, switchTo, toggleSidebar }
})
