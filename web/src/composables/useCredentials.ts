import { reactive, ref } from 'vue'
import { on, off } from './useSSE'
import { useWSAPI } from './useWSAPI'
import { useToast } from './useToast'

/**
 * B4（2026-10-18 凭据 vault §7）：Dashboard 凭据索要卡状态。
 *
 * 后端 WebCredentialBroker 在模型 API key 401/403（Auth 卡）等触发点发卡
 * 阻塞等裁决：
 * - 广播 SSE `credential-requested`（payload = AgentEvent::CredentialRequested
 *   的 data：request_id / service / key_name / reason / timeout_secs；
 *   `credential.pending` 列表另带 age_secs）；
 * - 阻塞等待 WSAPI `credential.respond`（值**不 transit 通道**：broker 先落
 *   vault 再放行等待侧——本组合式只提交明文一次，成功即弃）。
 *
 * 本组合式是全局单例（同 useQuestions/useApprovals）：initCredentials 订阅
 * SSE + 拉一次 `credential.pending` 补断线漏掉的事件；respond 成功（服务端
 * 已摘卡）或 `credential-resolved` 广播到达（任一窗口提交/拒绝/服务端超时）
 * 后本地摘卡；拒绝走 respond 的 denied 语义（服务端出册 + 广播 denied）。
 */

export interface PendingCredential {
  request_id: string
  service: string
  key_name: string
  reason?: string
  timeout_secs: number
  /** 本地到期时刻（ms epoch）= 接收时刻 + (timeout_secs - age_secs) * 1000 */
  expiresAt: number
}

const pendingCredentials = reactive<PendingCredential[]>([])
/** 每秒跳动的当前时刻，驱动卡片倒计时显示。 */
const now = ref(Date.now())
let initialized = false
let ticker: ReturnType<typeof setInterval> | null = null

function upsertFromPayload(data: any) {
  if (!data || typeof data.request_id !== 'string') return
  if (typeof data.key_name !== 'string') return
  const age = typeof data.age_secs === 'number' ? data.age_secs : 0
  const timeout = typeof data.timeout_secs === 'number' ? data.timeout_secs : 300
  const entry: PendingCredential = {
    request_id: data.request_id,
    service: String(data.service ?? ''),
    key_name: data.key_name,
    reason: typeof data.reason === 'string' ? data.reason : undefined,
    timeout_secs: timeout,
    expiresAt: Date.now() + Math.max(0, timeout - age) * 1000,
  }
  const existing = pendingCredentials.findIndex(c => c.request_id === data.request_id)
  if (existing === -1) {
    pendingCredentials.push(entry)
  } else {
    // SSE 重复推送同一请求（重连重放等）— 保留原到期时刻，避免倒计时重置。
    entry.expiresAt = pendingCredentials[existing].expiresAt
    pendingCredentials.splice(existing, 1, entry)
  }
}

function removeLocal(requestId: string) {
  const idx = pendingCredentials.findIndex(c => c.request_id === requestId)
  if (idx !== -1) pendingCredentials.splice(idx, 1)
}

/** 本地倒计时到期：只摘卡片，服务端届时已超时放行（调用方走原错误路径）。 */
function pruneExpired() {
  now.value = Date.now()
  for (let i = pendingCredentials.length - 1; i >= 0; i--) {
    if (pendingCredentials[i].expiresAt <= now.value) {
      pendingCredentials.splice(i, 1)
    }
  }
}

async function seedPending() {
  const { request } = useWSAPI()
  try {
    const res = await request('credential', 'pending', {}, 5000)
    const list: any[] = res?.pending ?? []
    list.forEach(upsertFromPayload)
  } catch {
    // 未装配 / WS 未就绪 — 静默（未装配实例永远不会有事件推送）。
  }
}

export function useCredentials() {
  const toast = useToast()

  /** 幂等初始化：AppLayout 挂载时调用一次。 */
  function initCredentials() {
    if (initialized) return
    initialized = true

    on('credential-requested', (data: any) => {
      upsertFromPayload(data)
    })
    // 任一窗口提交成功 / 拒绝 / 服务端超时 → 所有窗口静默摘卡（竞速败方
    // 无感关闭；stored 与否由提交方 toast 呈现，这里不弹）。
    on('credential-resolved', (data: any) => {
      if (data && typeof data.request_id === 'string') {
        removeLocal(data.request_id)
      }
    })
    // 断线期间错过的事件：连上后拉一次 pending 补齐。
    seedPending()
    ticker = setInterval(pruneExpired, 1000)
  }

  /** 提交凭据值（明文只此一跳：WSAPI → broker 先落 vault 再放行等待侧）。
   * 后端落库失败（vault 锁定等）请求保留可重试——本地卡片保留；unknown/
   * not wired（已超时/另一窗口已提交）→ 同步摘卡。 */
  async function submitCredential(requestId: string, value: string) {
    const { request } = useWSAPI()
    try {
      await request('credential', 'respond', { request_id: requestId, value })
      removeLocal(requestId)
      toast.success('凭据已保存到 vault')
    } catch (err: any) {
      const msg = String(err ?? '')
      if (msg.includes('unknown') || msg.includes('not wired') || msg.includes('不存在')) {
        removeLocal(requestId)
      }
      toast.error(`凭据提交失败: ${msg}`)
    }
  }

  /** 拒绝：服务端无专用 denied 入口——respond 一个哨兵值会被空值校验拒绝，
   * 这里改为直接摘本地卡（服务端等到超时自然放行，调用方走原错误路径）。 */
  function dismissCredential(requestId: string) {
    removeLocal(requestId)
    toast.info('已忽略本次凭据请求')
  }

  return { pendingCredentials, now, initCredentials, submitCredential, dismissCredential }
}

/** 测试辅助：重置模块级单例状态（生产代码勿用）。 */
export function _resetCredentialsForTest() {
  pendingCredentials.splice(0, pendingCredentials.length)
  if (ticker !== null) {
    clearInterval(ticker)
    ticker = null
  }
  initialized = false
}
