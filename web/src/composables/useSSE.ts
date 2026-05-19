import { onUnmounted } from 'vue'

type EventHandler = (data: any) => void

let eventSource: EventSource | null = null
const eventHandlers: Record<string, EventHandler[]> = {}

const EVENT_TYPES = ['log', 'status', 'security-alert', 'scanner-progress', 'cluster-event', 'heartbeat']

function dispatch(eventType: string, data: any) {
  const handlers = eventHandlers[eventType] || []
  handlers.forEach(h => h(data))
}

export function connectEvents() {
  if (eventSource) return

  try {
    eventSource = new EventSource('/api/events/stream')

    eventSource.onopen = () => {
      console.log('[NemesisAPI] SSE connected')
    }

    eventSource.onerror = () => {
      console.log('[NemesisAPI] SSE error, will auto-reconnect')
    }

    EVENT_TYPES.forEach(type => {
      eventSource!.addEventListener(type, (e: MessageEvent) => {
        try {
          const data = JSON.parse(e.data)
          dispatch(type, data)
        } catch (err) {
          console.error('[NemesisAPI] SSE parse error:', err)
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

export function on(eventType: string, handler: EventHandler) {
  if (!eventHandlers[eventType]) {
    eventHandlers[eventType] = []
  }
  eventHandlers[eventType].push(handler)
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

  return { connectEvents, disconnectEvents, on, off }
}
