/**
 * Chat API client — typed wrapper around the WSAPI `sessions.*` commands
 * (Dashboard multi-session management).
 *
 * Mirrors `crates/nemesis-web/src/handlers/sessions.rs`. The `id` returned
 * by `list` is the bare session id (sid) — the same value the client sends
 * back as `moduleData.session_id` on every chat.send / history_request.
 */

import { useWSAPI } from './useWSAPI'

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

export function useChatApi() {
  const { request } = useWSAPI()

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
  }
}
