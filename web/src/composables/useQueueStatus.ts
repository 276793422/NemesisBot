import { ref, computed, onUnmounted } from 'vue'
import { useWSAPI } from './useWSAPI'

/**
 * P4（能力扩展 WS8）：`chat.queue_status` WSAPI 的前端消费面——steer（插队）
 * / followUp（排队）两条队列的独立计数快照，供发送区徽标显示。
 *
 * 与 `useInboxStatus`（U7，数据源 `agent.inbox_status`）刻意分源：inbox
 * 那条链驱动 mode/busy 语义与 busy 发送解锁（多份既有 spec mock 其命令
 * 名，不动）；本链只服务 P4 徽标的分队列计数（后端 `chat.queue_status`
 * 的 steer/followUp 契约字段）。两者同源同口径（后端同一个 `InboxStatus`
 * 快照），数字必然一致，只是字段命名按各链契约。
 *
 * 刷新节奏与 useInboxStatus 相同：按需一次（挂载/换会话/重连）+
 * streaming 中短轮询（队列深度随 agent 消费变化）。
 */

/** `chat.queue_status` WSAPI 响应形态。 */
export interface QueueStatusData {
  available: boolean
  session_key?: string
  /** 插队（steer / `!` 前缀）条数。 */
  steer: number
  /** 排队（followUp）条数。 */
  followUp: number
  capacity: number
  busy: boolean
  mode: string
}

/** Poll cadence while streaming — queue depth is not latency-critical. */
const POLL_MS = 4000

export function useQueueStatus() {
  const { request } = useWSAPI()

  const status = ref<QueueStatusData | null>(null)

  let pollTimer: ReturnType<typeof setInterval> | null = null
  let sessionId = ''

  /** Fetch a fresh snapshot. `sid` updates the session the queries target. */
  async function refresh(sid?: string) {
    if (sid !== undefined) {
      // 换目标先清旧快照（与 useInboxStatus 同纪律）：旧会话的计数不得
      // 短暂带到新会话。
      if (sid !== sessionId) status.value = null
      sessionId = sid
    }
    const target = sessionId
    try {
      const s = await request('chat', 'queue_status', { session_id: target })
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

  /** 插队（steer）条数；不可用/失败时 0（徽标不炸）。 */
  const steerCount = computed(() =>
    status.value?.available ? status.value.steer : 0,
  )
  /** 排队（followUp）条数；不可用/失败时 0。 */
  const followUpCount = computed(() =>
    status.value?.available ? status.value.followUp : 0,
  )
  /** 任一队列非空（徽标显隐）。 */
  const hasQueued = computed(() => steerCount.value > 0 || followUpCount.value > 0)

  onUnmounted(stopPolling)

  return {
    status,
    refresh,
    startPolling,
    stopPolling,
    steerCount,
    followUpCount,
    hasQueued,
  }
}
