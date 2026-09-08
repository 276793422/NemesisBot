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
  /** M5（2026-09-05）：会话用量（`logs.session_list` 从 request_logs
   *  按 session_key 聚合回填；无用量记录时缺省）。 */
  tokens?: number
  cost?: number
  /** E4（2026-09-05）：fork 血缘（sidecar meta 回填；非 fork 会话缺省）。 */
  parent?: string
  parentTitle?: string
  forkedAtTurn?: number
  /** L6++（2026-09-08）：项目归属（sidecar meta 回填；无绑定缺省 =
   *  对话组）。显示名由前端以 projects.list 联结（后端不解析名字）。 */
  projectId?: string
  projectPath?: string
}

/** L6++（2026-09-08）：项目分组条目（镜像 handlers/projects.rs 的
 *  ProjectInfo 投影）。`running` = 项目 loop 存活（目录消失/inactive 时
 *  为 false —— 组头置灰、不可新建会话的依据）。 */
export interface ProjectInfo {
  id: string
  name: string
  path: string
  created_at: string
  running: boolean
}

/** P3-1 (2026-08-24 UI entry gap): fork-dialog turn row (GET /api/chat/sessions/:id/turns).
 * 2026-08-25 第三轮：计数全部按 chat_log jsonl 行（UI 渲染的真相源），不含
 * system/tool 行。 */
export interface SessionTurnRow {
  turn: number
  preview: string
  /** 该轮最后一条非空 user/assistant 消息首行 —— 分叉后新会话的末条。 */
  end_preview: string
  time: string
  /** chat_log rows inside this user→…→assistant exchange. */
  turn_messages: number
  /** Cumulative chat_log rows a fork cut at this turn retains. */
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

    create: async (
      title?: string,
      projectId?: string,
    ): Promise<{ session_id: string; title: string }> => {
      const data: Record<string, string> = {}
      if (title) data.title = title
      // L6++：项目会话创建（前端唯一表达归属的时刻；此后上行只带
      // session_id，归属由服务端 sid 索引裁决）。
      if (projectId) data.project_id = projectId
      return await request('sessions', 'create', Object.keys(data).length ? data : undefined)
    },

    rename: async (session_id: string, title: string): Promise<{ session_id: string; title: string }> =>
      await request('sessions', 'rename', { session_id, title }),

    /** `paused_cron_jobs` (2026-08-25): cron jobs that were bound to the
     * deleted session get disabled by the backend so they can't fire on —
     * and resurrect — a deleted conversation. */
    delete: async (session_id: string): Promise<{ deleted: string; paused_cron_jobs?: { id: string; name: string }[] }> =>
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

    // -----------------------------------------------------------------
    // L6++（2026-09-08）：项目分组（handlers/projects.rs 同名命令）。
    // -----------------------------------------------------------------
    listProjects: (): Promise<{ projects: ProjectInfo[]; count: number }> =>
      request('projects', 'list'),

    createProject: (name: string, path: string): Promise<{ project: ProjectInfo }> =>
      request('projects', 'create', { name, path }),

    /** 仅解除分组——不删除会话与项目目录内的任何文件（后端 note 原文）。 */
    removeProject: (project_id: string): Promise<{ removed: ProjectInfo; note: string }> =>
      request('projects', 'remove', { project_id }),

    renameProject: (project_id: string, name: string): Promise<{ project: ProjectInfo }> =>
      request('projects', 'rename', { project_id, name }),
  }
}
