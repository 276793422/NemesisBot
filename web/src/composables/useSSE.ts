import { onUnmounted } from 'vue'

type EventHandler = (data: any) => void

let eventSource: EventSource | null = null
const eventHandlers: Record<string, EventHandler[]> = {}

const EVENT_TYPES = ['log', 'status', 'security-alert', 'scanner-progress', 'cluster-event', 'heartbeat', 'memory-setup']

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
    eventSource = new EventSource('/api/events/stream')

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
