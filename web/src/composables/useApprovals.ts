import { reactive, ref } from 'vue'
import { on, off } from './useSSE'
import { useWSAPI } from './useWSAPI'
import { useToast } from './useToast'

/**
 * M7（devtool-upgrade 阶段 5）：Dashboard 审批卡状态。
 *
 * 后端 WebApprovalManager 在 auditor "ask" 规则命中时：
 * - 广播 SSE `approval-requested`（payload = AgentEvent::ApprovalRequested
 *   的 data：request_id / operation / target / risk_level / reason /
 *   timeout_secs / chat_id / session_key / age_secs）；
 * - 阻塞等待 WSAPI `approval.respond`（超时自动拒绝）。
 *
 * 本组合式是全局单例（同 useToast）：initApprovals 订阅 SSE + 拉一次
 * `approval.pending` 补断线漏掉的事件；respond 后本地移除；倒计时到期仅
 * 本地移除卡片（服务端届时已自动拒绝，拒绝理由会出现在聊天流）。
 */

export interface ApprovalRequest {
  request_id: string
  operation: string
  target: string
  risk_level: string
  reason: string
  timeout_secs: number
  chat_id?: string
  session_key?: string
  /** F3:「总是允许」将记住的 pattern（B5 归约前缀，如 `cargo publish *`）。
   * 空/缺省 = 不适用（CRITICAL 非 exec、run_script 等无 target 场景）。 */
  pattern?: string
  /** 本地到期时刻（ms epoch）= 接收时刻 + (timeout_secs - age_secs) * 1000 */
  expiresAt: number
}

/** M5（2026-10-10 集群无人值守硬化）：远程审批卡（worker 节点的 ask 经
 * master 注册表代理）。裁决经 WSAPI `approval.remote_respond` RPC 下发
 * worker 本地 pending（always 恒不可用——后端钉死 false）。 */
export interface RemoteApprovalRequest {
  request_id: string
  /** 展示/回传短号（master 注册表键，8 位起冲突加长）。 */
  short_id: string
  worker: string
  operation: string
  target: string
  risk_level: string
  reason: string
  issue_number?: number
  timeout_secs: number
  expiresAt: number
}

const pendingApprovals = reactive<ApprovalRequest[]>([])
const pendingRemoteApprovals = reactive<RemoteApprovalRequest[]>([])
/** 每秒跳动的当前时刻，驱动卡片倒计时显示。 */
const now = ref(Date.now())
let initialized = false
let ticker: ReturnType<typeof setInterval> | null = null

function upsertFromPayload(data: any) {
  if (!data || typeof data.request_id !== 'string') return
  const existing = pendingApprovals.findIndex(a => a.request_id === data.request_id)
  const age = typeof data.age_secs === 'number' ? data.age_secs : 0
  const timeout = typeof data.timeout_secs === 'number' ? data.timeout_secs : 0
  const entry: ApprovalRequest = {
    request_id: data.request_id,
    operation: String(data.operation ?? ''),
    target: String(data.target ?? ''),
    risk_level: String(data.risk_level ?? 'MEDIUM'),
    reason: String(data.reason ?? ''),
    timeout_secs: timeout,
    chat_id: data.chat_id || undefined,
    session_key: data.session_key || undefined,
    pattern: data.pattern ? String(data.pattern) : undefined,
    expiresAt: Date.now() + Math.max(0, timeout - age) * 1000,
  }
  if (existing === -1) {
    pendingApprovals.push(entry)
  } else {
    // SSE 重复推送同一请求（重连重放等）— 保留原到期时刻，避免倒计时重置。
    entry.expiresAt = pendingApprovals[existing].expiresAt
    pendingApprovals.splice(existing, 1, entry)
  }
}

function removeLocal(requestId: string) {
  const idx = pendingApprovals.findIndex(a => a.request_id === requestId)
  if (idx !== -1) pendingApprovals.splice(idx, 1)
}

/** 本地倒计时到期：只摘卡片，服务端超时自动拒绝（理由走聊天流）。 */
function pruneExpired() {
  now.value = Date.now()
  for (let i = pendingApprovals.length - 1; i >= 0; i--) {
    if (pendingApprovals[i].expiresAt <= now.value) {
      pendingApprovals.splice(i, 1)
    }
  }
  for (let i = pendingRemoteApprovals.length - 1; i >= 0; i--) {
    if (pendingRemoteApprovals[i].expiresAt <= now.value) {
      pendingRemoteApprovals.splice(i, 1)
    }
  }
}

async function seedPending() {
  const { request } = useWSAPI()
  try {
    const res = await request('approval', 'pending', {}, 5000)
    const list: any[] = res?.pending ?? []
    list.forEach(upsertFromPayload)
  } catch {
    // 未装配 / WS 未就绪 — 静默（未装配实例永远不会有事件推送）。
  }
}

function upsertRemoteFromPayload(data: any) {
  if (!data || typeof data.request_id !== 'string' || typeof data.short_id !== 'string') return
  const existing = pendingRemoteApprovals.findIndex(a => a.request_id === data.request_id)
  const age = typeof data.age_secs === 'number' ? data.age_secs : 0
  const timeout = typeof data.timeout_secs === 'number' ? data.timeout_secs : 300
  const entry: RemoteApprovalRequest = {
    request_id: data.request_id,
    short_id: data.short_id,
    worker: String(data.worker ?? ''),
    operation: String(data.operation ?? ''),
    target: String(data.target ?? ''),
    risk_level: String(data.risk_level ?? 'MEDIUM'),
    reason: String(data.reason ?? ''),
    issue_number: typeof data.issue_number === 'number' ? data.issue_number : undefined,
    timeout_secs: timeout,
    expiresAt: Date.now() + Math.max(0, timeout - age) * 1000,
  }
  if (existing === -1) {
    pendingRemoteApprovals.push(entry)
  } else {
    // SSE 重复推送同一请求（重连重放等）— 保留原到期时刻，避免倒计时重置。
    entry.expiresAt = pendingRemoteApprovals[existing].expiresAt
    pendingRemoteApprovals.splice(existing, 1, entry)
  }
}

function removeRemoteLocal(requestId: string) {
  const idx = pendingRemoteApprovals.findIndex(a => a.request_id === requestId)
  if (idx !== -1) pendingRemoteApprovals.splice(idx, 1)
}

/** 远程审批兜底补拉（断线期间错过 ask_register 事件）。 */
async function seedRemotePending() {
  const { request } = useWSAPI()
  try {
    const res = await request('approval', 'remote_list', {}, 5000)
    const list: any[] = res?.pending ?? []
    list.forEach(upsertRemoteFromPayload)
  } catch {
    // 无集群 / WS 未就绪 — 静默。
  }
}

export function useApprovals() {
  const toast = useToast()

  /** 幂等初始化：AppLayout 挂载时调用一次。 */
  function initApprovals() {
    if (initialized) return
    initialized = true

    on('approval-requested', (data: any) => {
      upsertFromPayload(data)
    })
    // F6：裁决广播（任一窗口 respond 成功 / 服务端超时）——所有窗口静默
    // 摘除本地卡：竞速败方不再挂到倒计时结束，也不弹错误 toast。
    on('approval-resolved', (data: any) => {
      if (data && typeof data.request_id === 'string') {
        removeLocal(data.request_id)
      }
    })
    // M5：远程审批（worker ask 经 master 注册表代理）——SSE 事件 + 断线
    // 补拉。resolved 载荷带 request_id，摘卡语义与本地一致。
    on('approval-remote-requested', (data: any) => {
      upsertRemoteFromPayload(data)
    })
    on('approval-remote-resolved', (data: any) => {
      if (data && typeof data.request_id === 'string') {
        removeRemoteLocal(data.request_id)
      }
    })
    // 断线期间错过的事件：连上后拉一次 pending 补齐。
    seedPending()
    seedRemotePending()
    ticker = setInterval(pruneExpired, 1000)
  }

  /** F3：`always=true` = 批准并记住（后端按层级安全门决定是否落规则：
   * CRITICAL 非 exec 忽略 always，只执行本次批准）。F6：`note` = 拒绝备注
   * （仅 approved=false 时随裁决送达，auditor 拼进拒绝消息回灌给模型）。 */
  async function respondTo(requestId: string, approved: boolean, always = false, note?: string) {
    const { request } = useWSAPI()
    try {
      await request('approval', 'respond', {
        request_id: requestId,
        approved,
        always,
        ...(note ? { note } : {}),
      })
      removeLocal(requestId)
      if (approved && always) {
        toast.success('已批准并记住该规则')
      } else {
        toast.success(approved ? '已批准' : '已拒绝')
      }
    } catch (err: any) {
      const msg = String(err ?? '')
      if (msg.includes('unknown') || msg.includes('not wired')) {
        // 已被超时/另一端裁决，或实例未装配 — 卡片同步摘除。
        removeLocal(requestId)
      }
      toast.error(`审批提交失败: ${msg}`)
    }
  }

  /** M5：远程批复（WSAPI `approval.remote_respond` → RPC 下发 worker）。
   * 无「总是允许」（后端 always 钉死 false）；`note` = 拒绝备注（随裁决
   * 送达 worker 回灌模型，与本地卡 F6 同语义）。裁决送达失败 = 卡保留可
   * 重发（unknown = 已超时/已裁决，摘卡同步）。 */
  async function respondRemoteTo(requestId: string, approved: boolean, note?: string) {
    const { request } = useWSAPI()
    try {
      await request('approval', 'remote_respond', {
        request_id: requestId,
        approved,
        ...(note ? { note } : {}),
      })
      removeRemoteLocal(requestId)
      toast.success(approved ? '已转发批准至节点' : '已转发拒绝至节点')
    } catch (err: any) {
      const msg = String(err ?? '')
      if (msg.includes('unknown') || msg.includes('not registered')) {
        // worker 已超时/已裁决，或注册表条目已过期 — 卡片同步摘除。
        removeRemoteLocal(requestId)
      }
      toast.error(`远程批复失败: ${msg}`)
    }
  }

  return { pendingApprovals, pendingRemoteApprovals, now, initApprovals, respondTo, respondRemoteTo }
}

/** 测试辅助：重置模块级单例状态（生产代码勿用）。 */
export function _resetApprovalsForTest() {
  pendingApprovals.splice(0, pendingApprovals.length)
  pendingRemoteApprovals.splice(0, pendingRemoteApprovals.length)
  if (ticker !== null) {
    clearInterval(ticker)
    ticker = null
  }
  initialized = false
}
