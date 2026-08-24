/**
 * Chat API client — typed wrapper around the WSAPI `sessions.*` commands
 * (Dashboard multi-session management).
 *
 * Mirrors `crates/nemesis-web/src/handlers/sessions.rs`. The `id` returned
 * by `list` is the bare session id (sid) — the same value the client sends
 * back as `moduleData.session_id` on every chat.send / history_request.
 */

import { useWSAPI } from './useWSAPI'
import { useAuthStore } from '../stores/auth'

export interface SessionEntry {
  id: string
  channel: string
  startTime: string
  lastTime: string
  messageCount: number
  firstMessage: string
  model: string
  title?: string
}

/** P3-1 (2026-08-24 UI entry gap): fork-dialog turn row (GET /api/chat/sessions/:id/turns). */
export interface SessionTurnRow {
  turn: number
  preview: string
  time: string
  /** Messages inside this user→…→assistant exchange. */
  turn_messages: number
  /** Cumulative history size a fork cut at this turn retains (含 system). */
  kept_messages: number
}

export interface SessionTurns {
  session_id: string
  session_key: string
  total_turns: number
  total_messages: number
  turns: SessionTurnRow[]
}

export interface SessionForkResult {
  forked: boolean
  session_id: string
  source_session_id: string
  new_key: string
  at_turn: number
  kept_messages: number
  dropped_messages: number
  summary_kept: boolean
  chat_log_lines: number
}

export function useChatApi() {
  const { request } = useWSAPI()
  const auth = useAuthStore()

  /** Authenticated JSON fetch against the HTTP API (same policy as SdkView:
   * X-Auth-Token header; throws with the server's error message on !ok). */
  async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
    const resp = await fetch(path, {
      ...init,
      headers: {
        'Content-Type': 'application/json',
        ...(auth.token ? { 'X-Auth-Token': auth.token } : {}),
        ...(init?.headers || {}),
      },
    })
    const body = await resp.json().catch(() => ({}))
    if (!resp.ok) {
      throw new Error(body?.error || `HTTP ${resp.status}`)
    }
    return body as T
  }

  return {
    list: async (): Promise<{ sessions: SessionEntry[] }> =>
      await request('sessions', 'list'),

    create: async (title?: string): Promise<{ session_id: string; title: string }> =>
      await request('sessions', 'create', title ? { title } : undefined),

    rename: async (session_id: string, title: string): Promise<{ session_id: string; title: string }> =>
      await request('sessions', 'rename', { session_id, title }),

    delete: async (session_id: string): Promise<{ deleted: string }> =>
      await request('sessions', 'delete', { session_id }),

    clear: async (session_id: string): Promise<{ cleared: string }> =>
      await request('sessions', 'clear', { session_id }),

    export: async (session_id: string): Promise<{ session_id: string; messages: unknown[]; count: number }> =>
      await request('sessions', 'export', { session_id }),

    /** P3-1: turn boundary table for the fork dialog. */
    turns: (session_id: string): Promise<SessionTurns> =>
      apiFetch<SessionTurns>(`/api/chat/sessions/${encodeURIComponent(session_id)}/turns`),

    /** P3-1: fork at a turn boundary (omit at_turn = whole history).
     * Backend delegates to the Z1 fork_session — SessionStore + chat_log
     * copy + boundary events. */
    fork: (session_id: string, at_turn?: number): Promise<SessionForkResult> =>
      apiFetch<SessionForkResult>(`/api/chat/sessions/${encodeURIComponent(session_id)}/fork`, {
        method: 'POST',
        body: JSON.stringify(at_turn != null ? { at_turn } : {}),
      }),
  }
}
