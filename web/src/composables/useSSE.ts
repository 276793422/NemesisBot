import { onUnmounted } from 'vue'
import { apiUrl } from '../lib/appBase'

type EventHandler = (data: any) => void

let eventSource: EventSource | null = null
const eventHandlers: Record<string, EventHandler[]> = {}

// L2（devtool-upgrade 阶段 6）：'resync' = 服务端提示缺口已滑出重放窗口
// （或网关重启 seq 重置），订阅方应全量刷新兜底。
// SB（2026-09-17）：'session.created' = 会话 jsonl 首行落盘（隐式物化），
// 侧栏订阅后 force 刷新列表（修「首条消息隐式创建的会话不进侧栏」）。
// P8（2026-09-21）：'chat.activity' = 某会话 chat_event_log 落了新帧
//（chat 行或工具事件），全局广播 {session_id, seq}——其他标签/端据此
// 感知落后并全量刷新（本端 WS 实时帧先行，seq 游标短路零开销）。
const EVENT_TYPES = ['log', 'status', 'security-alert', 'scanner-progress', 'cluster-event', 'heartbeat', 'memory-setup', 'board-changed', 'usage-changed', 'approval-requested', 'approval-resolved', 'question-asked', 'question-resolved', 'board.plan_ready', 'board.plan_failed', 'resync', 'session.created', 'editor-mode', 'chat.activity']

function dispatch(eventType: string, data: any) {
  const handlers = eventHandlers[eventType] || []
  handlers.forEach(h => h(data))
}

export function connectEvents() {
  if (eventSource) {
    // 已有连接 — 但如果之前出错被浏览器关闭过，需要重置
    if (eventSource.readyState === EventSource.CLOSED) {
      try { eventSource.close() } catch {}
      eventSource = null
    } else {
      return
    }
  }

  try {
    // F1（2026-09-22 审计修复）：服务端 REST 面挂了统一鉴权中间件，SSE 端点
    // 同在闸内。EventSource 无法携带自定义头（X-Auth-Token 不可用），按
    // 服务端 token 载体优先级补 `?token=` 查询参数；未配置鉴权（空 token）
    // 时参数缺省，行为与此前一致。
    const stored = localStorage.getItem('nemesisbot_auth_token')
    const sseUrl = stored ? `${apiUrl('/api/events/stream')}?token=${encodeURIComponent(stored)}` : apiUrl('/api/events/stream')
    eventSource = new EventSource(sseUrl)

    eventSource.onopen = () => {
      console.log('[NemesisAPI] SSE connected')
    }

    eventSource.onerror = (e) => {
      console.warn('[NemesisAPI] SSE error (readyState=' +
        (eventSource ? eventSource.readyState : 'null') + '), browser will auto-reconnect', e)
    }

    EVENT_TYPES.forEach(type => {
      eventSource!.addEventListener(type, (e: MessageEvent) => {
        try {
          const data = JSON.parse(e.data)
          dispatch(type, data)
        } catch (err) {
          console.error('[NemesisAPI] SSE parse error:', err, 'raw:', e.data)
        }
      })
    })
  } catch (e) {
    console.error('[NemesisAPI] SSE connect error:', e)
  }
}

export function disconnectEvents() {
  if (eventSource) {
    eventSource.close()
    eventSource = null
  }
}

/// 返回当前 SSE 连接状态：
/// - `null` — 从未调用 connectEvents
/// - 0 (CONNECTING)、1 (OPEN)、2 (CLOSED) — EventSource.readyState
export function sseReadyState(): number | null {
  return eventSource ? eventSource.readyState : null
}

export function on(eventType: string, handler: EventHandler) {
  if (!eventHandlers[eventType]) {
    eventHandlers[eventType] = []
  }
  if (!eventHandlers[eventType].includes(handler)) {
    eventHandlers[eventType].push(handler)
  }
}

export function off(eventType: string, handler?: EventHandler) {
  if (!eventHandlers[eventType]) return
  if (!handler) {
    delete eventHandlers[eventType]
    return
  }
  eventHandlers[eventType] = eventHandlers[eventType].filter(h => h !== handler)
}

export function useSSE() {
  onUnmounted(() => {
    // Don't disconnect SSE on component unmount - it's shared
  })

  return { connectEvents, disconnectEvents, on, off, sseReadyState }
}
