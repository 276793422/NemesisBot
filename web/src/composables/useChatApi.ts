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
import { apiUrl } from '../lib/appBase'

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
  /** P2（2026-09-11）：未送达 assistant 回复数（推送失败时后端打点；
   *  拉取历史经 mark_delivered 清零）。0/缺省 = 无未读。 */
  undelivered?: number
  /** WS9/P17（2026-09-26）：谱系组合视图（sidecar meta 平面字段经
   *  logs.rs 后处理投影——祖先链沿 parent 上溯、帽 8、防环；无 parent
   *  的条目不带此字段）。祖先序 = 最近父 → 根。 */
  lineage?: {
    parent_session_id: string
    fork_point_seq?: number
    reason: string
    ancestors: Array<{ id: string; title: string }>
  }
  /** WS9/P17：分叉缘由（fork 端点 reason 入参落 meta 的 fork_reason；
   *  缺省不写）。 */
  forkReason?: string
  /** WS9/P17：最近一次回退记录（meta.last_rewind；no-op 回退不写）。
   *  后端 serde 无 rename——键保持 snake_case。 */
  lastRewind?: {
    at_index: number
    dropped_rows: number
    dropped_turns: number
    ts: string
  }
  /** WS9/P18：分支摘要预览（meta.branch_summary 前 160 字符截断——
   *  全文真相源在 agent 侧 meta，注入走 # Branch Context）。 */
  branchSummaryPreview?: string
  /** B11（2026-10-18 snooze）：会话暂停到点（RFC3339，meta sidecar
   *  回填）。缺省 = 未暂停；到期由后端 sweeper 摘标记并注入恢复行。 */
  snoozedUntil?: string
  /** B11：暂停备注（用户可选说明）。 */
  snoozeNote?: string
  /** 会话级全部放行（2026-10-24 长任务无人值守计划：sidecar `allow_all`
   *  投影）。缺省 = 关。侧栏 🔓 徽标的依据。 */
  allow_all?: boolean
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
  /** 完全独立形态（请求回显）：true = 新会话无血缘 meta。 */
  independent?: boolean
}

export function useChatApi() {
  const { request } = useWSAPI()
  const auth = useAuthStore()

  /** Authenticated JSON fetch against the HTTP API (same policy as SdkView:
   * X-Auth-Token header; throws with the server's error message on !ok). */
  async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
    const resp = await fetch(apiUrl(path), {
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
    list: async (): Promise<{ sessions: SessionEntry[]; bindings?: Record<string, string> }> =>
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

    /** B（2026-09-23 会话绑定注册表）：绑定式 get-or-create——binding_key
     *  已绑且会话存活 → 原样返回既有会话（reused=true，零新建）；未绑/
     *  陈旧 → 新建并落绑定。替代 localStorage 本地映射（每浏览器各自为政
     *  + 本地误判即重建 → 同目标空会话复制机）。 */
    createBound: async (
      binding_key: string,
      title?: string,
    ): Promise<{ session_id: string; title: string; reused: boolean }> =>
      await request('sessions', 'create', {
        binding_key,
        ...(title ? { title } : {}),
      }),

    /** 重绑：binding_key → 既有会话（draft_apply 后「__new__」目标落到正式
     *  工作流名时调用；旧会话自动让出键）。 */
    setBinding: async (binding_key: string, session_id: string): Promise<{ ok: boolean }> =>
      await request('sessions', 'set_binding', { binding_key, session_id }),

    /** 摘键：释放「__new__」引导键（其会话已重绑到正式工作流名）。 */
    removeBinding: async (binding_key: string): Promise<{ ok: boolean; removed: boolean }> =>
      await request('sessions', 'remove_binding', { binding_key }),

    rename: async (session_id: string, title: string): Promise<{ session_id: string; title: string }> =>
      await request('sessions', 'rename', { session_id, title }),

    /** `paused_cron_jobs` (2026-08-25): cron jobs that were bound to the
     * deleted session get disabled by the backend so they can't fire on —
     * and resurrect — a deleted conversation.
     * 回收站（2026-10-30）：默认移入 `<workspace>/logs/trash/sessions/`
     * （90 天 TTL，日志页「回收站」可恢复）；`hard:true` 是 API 层逃生口，
     * 前端 UI 不暴露。 */
    delete: async (
      session_id: string,
      hard?: boolean,
      source?: 'single' | 'batch',
    ): Promise<{
      deleted: string
      trashed?: boolean
      trash_entry?: string
      paused_cron_jobs?: { id: string; name: string }[]
    }> => {
      const data: Record<string, unknown> = { session_id }
      if (hard) data.hard = hard
      if (source) data.source = source
      return await request('sessions', 'delete', data)
    },

    // -----------------------------------------------------------------
    // 会话回收站（2026-10-30）：sessions.trash_* 四命令。
    // -----------------------------------------------------------------

    /** 条目列表（新→旧；孤儿条目 orphan:true、title 为诚实投影）。 */
    trashList: (): Promise<{
      entries: {
        entry: string
        original_session_key: string
        display_id: string
        title: string | null
        deleted_at_ms: number
        message_count: number
        bytes: number
        source: string
        orphan: boolean
      }[]
    }> => request('sessions', 'trash_list'),

    /** 恢复。同 id 冲突 → 结构化 `{conflict:true, suggested_sid}`（条目不
     *  消耗）；带 `new_sid` 改名恢复（拍板②）。 */
    trashRestore: (entry: string, new_sid?: string): Promise<Record<string, unknown>> =>
      request('sessions', 'trash_restore', new_sid ? { entry, new_sid } : { entry }),

    /** 彻底删除单条（不可逆）。 */
    trashPurge: (entry: string): Promise<{ purged: string }> =>
      request('sessions', 'trash_purge', { entry }),

    /** 清空回收站（不可逆）。 */
    trashPurgeAll: (): Promise<{ purged: number }> =>
      request('sessions', 'trash_purge_all'),

    clear: async (session_id: string): Promise<{ cleared: string }> =>
      await request('sessions', 'clear', { session_id }),

    /** P2（2026-09-11）：清零未送达标记（拉取历史后调用；幂等）。 */
    markDelivered: (session_id: string): Promise<{ cleared: boolean }> =>
      request('sessions', 'mark_delivered', { session_id }),

    /** B11（2026-10-18 snooze）：暂停会话到点。`until` = 相对秒数（如
     *  3600）或 RFC3339 时刻（必须在未来）；`note` 可选说明。到期由后端
     *  sweeper 自动摘标记 + 注入恢复行。重复调用 = 改期。 */
    snooze: (
      session_id: string,
      until: string,
      note?: string,
    ): Promise<{ session_id: string; snoozed_until: string }> =>
      request('sessions', 'snooze', {
        session_id,
        until,
        ...(note ? { note } : {}),
      }),

    /** B11：取消暂停（立即恢复）。幂等——未暂停时返回 unsnoozed=false。 */
    unsnooze: (session_id: string): Promise<{ session_id: string; unsnoozed: boolean }> =>
      request('sessions', 'unsnooze', { session_id }),

    /** detach（2026-10-20）：脱离父分支——清血缘 meta（parent/fork缘由/
     *  分支摘要），与会话的父会话平级独立；子分支跟随独立。不可逆；
     *  消息内容不受影响。无血缘时后端诚实报错。 */
    detach: (session_id: string): Promise<{ session_id: string; detached: boolean }> =>
      request('sessions', 'detach', { session_id }),

    export: async (session_id: string): Promise<{ session_id: string; messages: unknown[]; count: number }> =>
      await request('sessions', 'export', { session_id }),

    /** P3-1: turn boundary table for the fork dialog. */
    turns: (session_id: string): Promise<SessionTurns> =>
      apiFetch<SessionTurns>(`/api/chat/sessions/${encodeURIComponent(session_id)}/turns`),

    /** P3-1: fork at a turn boundary (omit at_turn = whole history).
     * Backend delegates to the Z1 fork_session — SessionStore + chat_log
     * copy + boundary events. `independent`（2026-10-20）：勾选后新会话
     * 不写血缘 meta（生来无父分支，无分叉缘由/分支摘要）。 */
    fork: (
      session_id: string,
      at_turn?: number,
      independent?: boolean,
    ): Promise<SessionForkResult> =>
      apiFetch<SessionForkResult>(`/api/chat/sessions/${encodeURIComponent(session_id)}/fork`, {
        method: 'POST',
        body: JSON.stringify({
          ...(at_turn != null ? { at_turn } : {}),
          ...(independent ? { independent: true } : {}),
        }),
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

    /** 在系统文件管理器中打开项目目录（后端只放行注册表已知路径）。 */
    openProjectDir: (project_id: string): Promise<{ opened: string }> =>
      request('projects', 'open_dir', { project_id }),
  }
}
