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

const pendingApprovals = reactive<ApprovalRequest[]>([])
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
    // 断线期间错过的事件：连上后拉一次 pending 补齐。
    seedPending()
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

  return { pendingApprovals, now, initApprovals, respondTo }
}

/** 测试辅助：重置模块级单例状态（生产代码勿用）。 */
export function _resetApprovalsForTest() {
  pendingApprovals.splice(0, pendingApprovals.length)
  if (ticker !== null) {
    clearInterval(ticker)
    ticker = null
  }
  initialized = false
}
