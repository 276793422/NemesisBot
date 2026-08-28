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
    if (sid !== undefined) sessionId = sid
    try {
      status.value = await request('agent', 'inbox_status', { session_id: sessionId })
    } catch {
      // WS offline / agent absent — degrade to the conservative default.
      status.value = null
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
