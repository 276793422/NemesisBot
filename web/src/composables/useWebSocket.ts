import { ref, onUnmounted } from 'vue'

export type WSStatus = 'connecting' | 'connected' | 'disconnected'

export const wsStatus = ref<WSStatus>('disconnected')

let ws: WebSocket | null = null
let token: string | null = null
let reconnectDelay = 1000
const maxReconnectDelay = 30000
const messageQueue: string[] = []
let manualClose = false
let heartbeatInterval: ReturnType<typeof setInterval> | null = null

let onMessageCallback: ((data: any) => void) | null = null

function buildWSUrl(): string {
  if (window.__DASHBOARD_BACKEND__) {
    return 'ws://' + window.__DASHBOARD_BACKEND__ + '/ws'
  }
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return protocol + '//' + window.location.host + '/ws'
}

function flushQueue() {
  while (messageQueue.length > 0) {
    const msg = messageQueue.shift()!
    send(msg)
  }
}

function startHeartbeat() {
  stopHeartbeat()
  heartbeatInterval = setInterval(() => {
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({
        type: 'system',
        module: 'heartbeat',
        cmd: 'ping',
        data: {},
        timestamp: new Date().toISOString(),
      }))
    }
  }, 30000)
}

function stopHeartbeat() {
  if (heartbeatInterval) {
    clearInterval(heartbeatInterval)
    heartbeatInterval = null
  }
}

function notifyStatus(s: WSStatus) {
  wsStatus.value = s
}

function reconnect() {
  if (manualClose) return
  console.log(`[NemesisAPI] Reconnecting in ${reconnectDelay}ms...`)
  setTimeout(() => {
    reconnectDelay = Math.min(reconnectDelay * 2, maxReconnectDelay)
    connect(null, token)
  }, reconnectDelay)
}

export function connect(host?: string | null, authToken?: string | null) {
  if (ws && ws.readyState === WebSocket.OPEN) return

  if (authToken) token = authToken
  manualClose = false
  notifyStatus('connecting')

  let wsUrl = host || buildWSUrl()
  if (token) {
    const sep = wsUrl.includes('?') ? '&' : '?'
    wsUrl = wsUrl + sep + 'token=' + encodeURIComponent(token)
  }

  try {
    ws = new WebSocket(wsUrl)

    ws.onopen = () => {
      console.log('[NemesisAPI] WebSocket connected')
      reconnectDelay = 1000
      notifyStatus('connected')
      flushQueue()
      startHeartbeat()
    }

    ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data)
        if (onMessageCallback) onMessageCallback(data)
      } catch (e) {
        console.error('[NemesisAPI] Parse error:', e)
      }
    }

    ws.onclose = (event) => {
      console.log('[NemesisAPI] WebSocket closed:', event.code)
      ws = null
      stopHeartbeat()

      if (!manualClose) {
        notifyStatus('disconnected')
        if (event.code === 1008 || event.code === 4001) {
          notifyStatus('disconnected')
        } else {
          reconnect()
        }
      }
    }

    ws.onerror = () => {
      notifyStatus('disconnected')
    }
  } catch (e) {
    console.error('[NemesisAPI] Connect error:', e)
    notifyStatus('disconnected')
    reconnect()
  }
}

export function send(content: string) {
  const message = {
    type: 'message',
    module: 'chat',
    cmd: 'send',
    data: { content },
    timestamp: new Date().toISOString(),
  }

  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(message))
  } else {
    messageQueue.push(content)
    connect()
  }
}

export function sendHistoryRequest(requestId: string, limit: number, beforeIndex?: number | null) {
  const data: any = { request_id: requestId, limit }
  if (beforeIndex != null) data.before_index = beforeIndex

  const message = {
    type: 'message',
    module: 'chat',
    cmd: 'history_request',
    data,
    timestamp: new Date().toISOString(),
  }

  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(message))
  }
}

export function disconnect() {
  manualClose = true
  stopHeartbeat()
  if (ws) {
    ws.close()
    ws = null
  }
  notifyStatus('disconnected')
}

export function testConnection(testToken: string): Promise<boolean> {
  return new Promise((resolve) => {
    let wsUrl = buildWSUrl()
    const sep = wsUrl.includes('?') ? '&' : '?'
    wsUrl = wsUrl + sep + 'token=' + encodeURIComponent(testToken)

    const testWs = new WebSocket(wsUrl)
    let done = false

    testWs.onopen = () => {
      if (!done) { done = true; testWs.close(); resolve(true) }
    }
    testWs.onerror = () => {
      if (!done) { done = true; resolve(false) }
    }
    testWs.onclose = (event) => {
      if (!done) {
        done = true
        resolve(!(event.code === 1008 || event.code === 4001))
      }
    }
    setTimeout(() => {
      if (!done) { done = true; testWs.close(); resolve(false) }
    }, 5000)
  })
}

export function httpGet<T = any>(path: string): Promise<T> {
  return fetch(path).then(res => {
    if (!res.ok) throw new Error('HTTP ' + res.status)
    return res.json()
  })
}

export function onMessage(cb: (data: any) => void) {
  onMessageCallback = cb
}

export function useWebSocket() {
  onUnmounted(() => {
    // Don't disconnect on unmount - connection is shared
  })

  return {
    status: wsStatus,
    connect,
    send,
    sendHistoryRequest,
    disconnect,
    testConnection,
    httpGet,
    onMessage,
  }
}
