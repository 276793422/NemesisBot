import { ref, computed, onUnmounted } from 'vue'
import { useWSAPI } from './useWSAPI'

/**
 * U7 inbox visibility (G1): expose the agent's per-session queue/steer state
 * to the chat UI.
 *
 * Backend truth source is the `agent.inbox_status` WSAPI command
 * (crates/nemesis-web/src/handlers/agent.rs) which snapshots the session's
 * dual FIFO (next_turn / next_step), shared capacity, busy flag and the
 * configured concurrent mode. The composable keeps that snapshot fresh:
 * one fetch on demand (mount / session switch / reconnect) plus a short
 * poll interval while the chat is streaming (queue depth changes as the
 * agent drains messages).
 *
 * Conservative by design: until a successful response says otherwise the
 * mode is treated as `reject`, i.e. NO extra send capability is unlocked.
 */

/** Shape of the `agent.inbox_status` WSAPI response. */
export interface InboxStatusData {
  available: boolean
  session_key?: string
  next_turn: number
  next_step: number
  capacity: number
  busy: boolean
  mode: string
}

/** Poll cadence while streaming — queue depth is not latency-critical. */
const POLL_MS = 4000

export function useInboxStatus() {
  const { request } = useWSAPI()

  const status = ref<InboxStatusData | null>(null)

  let pollTimer: ReturnType<typeof setInterval> | null = null
  let sessionId = ''

  /** Fetch a fresh snapshot. `sid` updates the session the queries target. */
  async function refresh(sid?: string) {
    if (sid !== undefined) {
      // 换目标先清旧快照（2026-09-24，先判断再展示）：旧会话的排队数/
      // queue 放行态不得短暂带到新会话（mode 兜底 'reject' 保守态）。
      if (sid !== sessionId) status.value = null
      sessionId = sid
    }
    const target = sessionId
    try {
      const s = await request('agent', 'inbox_status', { session_id: target })
      // 丢弃陈旧会话的乱序响应（会话切换 + 轮询并发时旧快照后到）。
      if (target === sessionId) status.value = s
    } catch {
      if (target === sessionId) status.value = null
    }
  }

  /** Poll every POLL_MS until stopPolling(). No-op if already polling. */
  function startPolling(sid?: string) {
    if (sid !== undefined) sessionId = sid
    if (pollTimer) return
    void refresh()
    pollTimer = setInterval(() => { void refresh() }, POLL_MS)
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer)
      pollTimer = null
    }
  }

  /** Backend routing mode; 'reject' until proven otherwise. */
  const mode = computed(() => (status.value?.available ? status.value.mode : 'reject'))
  /** Steer mode: `!`-prefixed input is delivered into the running turn. */
  const steerEnabled = computed(() => mode.value === 'steer')
  /** busy 时发送是否仍有效（后端排队/插队而不是弹回 BUSY_MESSAGE）。 */
  const queueEnabled = computed(() => mode.value === 'queue' || mode.value === 'steer')

  /** Total messages waiting across both queues. */
  const queuedTotal = computed(() =>
    status.value?.available ? status.value.next_turn + status.value.next_step : 0,
  )
  const queueFull = computed(() =>
    status.value?.available ? queuedTotal.value >= status.value.capacity : false,
  )

  onUnmounted(stopPolling)

  return {
    status,
    refresh,
    startPolling,
    stopPolling,
    mode,
    steerEnabled,
    queueEnabled,
    queuedTotal,
    queueFull,
  }
}
