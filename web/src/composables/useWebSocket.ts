import { ref, onUnmounted } from 'vue'
import { handleWSResponse } from './wsResponseHandler'
import { initWSAPI } from './useWSAPI'
import { wsUrl as buildBaseWsUrl, apiUrl } from '../lib/appBase'

export type WSStatus = 'connecting' | 'connected' | 'disconnected'

export const wsStatus = ref<WSStatus>('disconnected')

let ws: WebSocket | null = null
let token: string | null = null
let reconnectDelay = 1000
const maxReconnectDelay = 30000
const messageQueue: string[] = []
let manualClose = false
let heartbeatInterval: ReturnType<typeof setInterval> | null = null
// W2（2026-09-23）：重连定时器句柄——disconnect() 必须能取消已排程的
// 重连，否则显式断开后定时器仍触发 connect()，连接“死而复生”。
let reconnectTimer: ReturnType<typeof setTimeout> | null = null

// BUG-20261121-005 半开连接检测：heartbeat ping 有配套 pong（后端
// handle_system_module 对 heartbeat.ping 回 {system,heartbeat,pong} 帧），
// 此前端从不消费 pong——休眠唤醒/网络切换后的半开 TCP 下 send() 静默进
// 内核缓冲，status 恒 connected，chat.send 永久无响应（带 reqId 的 WSAPI
// 30s 超时报错，chat.send 这类 message 帧只能干等）。ping 后一个完整
// 心跳周期（30s）未见 pong → 判半开，主动断开走重连循环。
let pendingPing = false

// Extra query params appended to the WS URL on connect (e.g.
// `workflow_chat=<index>&pwd=<password>` for the standalone
// workflow-chat page). Persists across reconnects.
let extraQueryParams: Record<string, string> = {}

// Multi-handler support (replaces single onMessageCallback)
type MessageHandler = (data: any) => void
const messageHandlers: MessageHandler[] = []

function buildWSUrl(): string {
  if (window.__DASHBOARD_BACKEND__) {
    // desktop（wry）形态：注入的绝对 backend 地址，不经桥子路径。
    return 'ws://' + window.__DASHBOARD_BACKEND__ + '/ws'
  }
  // 经桥远程访问时加 `/d/<node_id>` 前缀（appBase 读 <base> 标签；直连为空）。
  return buildBaseWsUrl('/ws')
}

function applyQueryParams(wsUrl: string): string {
  const params = new URLSearchParams()
  if (token) params.set('token', token)
  for (const [k, v] of Object.entries(extraQueryParams)) {
    params.set(k, v)
  }
  const qs = params.toString()
  if (!qs) return wsUrl
  const sep = wsUrl.includes('?') ? '&' : '?'
  return wsUrl + sep + qs
}

function flushQueue() {
  while (messageQueue.length > 0) {
    const json = messageQueue.shift()!
    // Send raw queued JSON as-is (preserves original type/module/cmd)
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(json)
    }
  }
}

function startHeartbeat() {
  stopHeartbeat()
  heartbeatInterval = setInterval(() => {
    if (ws && ws.readyState === WebSocket.OPEN) {
      if (pendingPing) {
        // BUG-20261121-005 半开臂：上个周期的 ping 没等到 pong → 判半开。
        // W7 纪律：先摘模块状态再 close——FakeWS/真浏览器 close() 触发的
        // 迟到 onclose 被 ws !== sock 守卫拦住，不会重复走断线路径。
        const dead = ws
        ws = null
        stopHeartbeat()
        notifyStatus('disconnected')
        try { dead.close() } catch {}
        reconnect()
        return
      }
      pendingPing = true
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
  // 重排程前先清掉旧定时器（防 onclose/connect-error 双路径叠定时器）；
  // 回调内复查 manualClose——排程与触发之间可能发生显式 disconnect()。
  if (reconnectTimer !== null) clearTimeout(reconnectTimer)
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null
    if (manualClose) return
    reconnectDelay = Math.min(reconnectDelay * 2, maxReconnectDelay)
    connect(null, token)
  }, reconnectDelay)
}

export function connect(
  host?: string | null,
  authToken?: string | null,
  extraParams?: Record<string, string> | null,
) {
  // Skip if already open or connecting (prevents orphaned WebSocket connections)
  if (ws && ws.readyState < WebSocket.CLOSING) {
    // 不重复开连接，但要记下最新 token——否则登录 connect 撞上在途重连
    // 时 token 赋值被跳过，重连循环永远不带 token（401 死循环）。
    if (authToken) token = authToken
    return
  }

  if (authToken) token = authToken
  if (extraParams) extraQueryParams = { ...extraParams }
  manualClose = false
  notifyStatus('connecting')

  const wsUrl = applyQueryParams(host || buildWSUrl())

  try {
    ws = new WebSocket(wsUrl)

    // W7（2026-10-14）：捕获本连接引用——守卫放行 CLOSING 态旧 socket 时
    // connect 可重入，新 socket 已赋模块变量；旧 socket 迟到的 onclose
    // 若直接清 `ws`/停心跳/报断线，会误杀新连接（sendRaw 永久入队、下次
    // connect 双活 socket 帧双发、状态假断线）。各回调只在自己仍是当前
    // 连接时才动模块状态。
    const sock = ws

    sock.onopen = () => {
      console.log('[NemesisAPI] WebSocket connected')
      reconnectDelay = 1000
      pendingPing = false
      notifyStatus('connected')
      flushQueue()
      startHeartbeat()
    }

    sock.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data)

        // BUG-20261121-005：服务端对 heartbeat.ping 回 pong 帧（无 reqId，
        // handleWSResponse 不消费）——在此解除半开嫌疑，不进 handlers 分发。
        if (data.type === 'system' && data.module === 'heartbeat' && data.cmd === 'pong') {
          pendingPing = false
          return
        }

        // 1. Handle response-type messages (useWSAPI Promise routing)
        if (handleWSResponse(data)) return

        // 2. Dispatch to all registered handlers (chat, logs, etc.)
        for (const handler of messageHandlers) {
          try {
            handler(data)
          } catch (e) {
            console.error('[NemesisAPI] Handler error:', e)
          }
        }
      } catch (e) {
        console.error('[NemesisAPI] Parse error:', e)
      }
    }

    sock.onclose = (event) => {
      console.log('[NemesisAPI] WebSocket closed:', event.code)
      // W7：被更新的连接取代后，本 socket 的关闭不再拥有模块状态。
      if (ws !== sock) return
      ws = null
      stopHeartbeat()
      pendingPing = false

      if (!manualClose) {
        notifyStatus('disconnected')
        if (event.code !== 1008 && event.code !== 4001) {
          reconnect()
        }
      }
    }

    sock.onerror = () => {
      // W7：同 onclose——旧 socket 的错误不得伪造新连接的断线状态。
      if (ws !== sock) return
      notifyStatus('disconnected')
    }
  } catch (e) {
    console.error('[NemesisAPI] Connect error:', e)
    notifyStatus('disconnected')
    reconnect()
  }
}

/**
 * Send a raw WS message object. Handles queuing when disconnected.
 */
export function sendRaw(msg: object) {
  const payload = { ...msg, timestamp: new Date().toISOString() }
  const json = JSON.stringify(payload)
  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(json)
  } else {
    messageQueue.push(json)
    connect()
  }
}

// Initialize useWSAPI with sendRaw (breaks circular dependency)
initWSAPI(sendRaw)

/** T8 多模态：chat.send media 项——`{id}`（上传端点返回的 uploads 裸文件名）
 *  或 `{path}`（用户点名本地路径）。后端 resolve_media_ref 统一解析。 */
export interface MediaRef {
  id?: string
  path?: string
}

export function send(
  content: string,
  voicePlayback?: boolean,
  extra?: { module?: string; moduleData?: Record<string, unknown>; media?: MediaRef[] },
) {
  const data: any = { content }
  if (voicePlayback) {
    data.voice_playback = true
  }
  if (extra?.media && extra.media.length > 0) {
    data.media = extra.media
  }
  if (extra?.moduleData) {
    Object.assign(data, extra.moduleData)
  }
  sendRaw({
    type: 'message',
    module: extra?.module ?? 'chat',
    cmd: 'send',
    data,
  })
}

export function sendHistoryRequest(
  requestId: string,
  limit: number,
  beforeIndex?: number | null,
  extra?: { module?: string; moduleData?: Record<string, unknown> },
) {
  const data: any = { request_id: requestId, limit }
  if (beforeIndex != null) data.before_index = beforeIndex
  if (extra?.moduleData) {
    Object.assign(data, extra.moduleData)
  }

  sendRaw({
    type: 'message',
    module: extra?.module ?? 'chat',
    cmd: 'history_request',
    data,
  })
}

export function disconnect() {
  manualClose = true
  stopHeartbeat()
  pendingPing = false
  // W2：取消已排程的重连——manualClose 只拦回调内的老检查点，
  // 不取消的话定时器到点仍会 connect()（显式断开后连接复活）。
  if (reconnectTimer !== null) {
    clearTimeout(reconnectTimer)
    reconnectTimer = null
  }
  // BUG-20261121-006：手动 disconnect = 终结/切换语义（logout 换认证会话、
  // standalone connectChat 换 workflow 会话）。断线期间 sendRaw 积压的旧
  // 消息不得经下次 connect 的 onopen flushQueue 复活——否则 logout 前的
  // chat.send 以新认证身份突然发出（auth.ts W3「旧会话轮次状态不带入下个
  // 登录周期」的漏网面）；standalone 切索引时旧积压错位进新会话。自动重连
  // 路径不经 disconnect()，断线重连的 flush 语义不变。
  messageQueue.length = 0
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

/// 会话 token 只读视图。与 httpGet 的 X-Auth-Token 同源（connect() 注入的
/// 模块级变量，单一真相）——useSSE 等无法携带自定义头的消费者复用。
export function currentAuthToken(): string | null {
  return token
}

export function httpGet<T = any>(path: string): Promise<T> {
  // X-Auth-Token 统一鉴权（lib/authFetch.ts）：本模块因循环依赖
  // （auth store → 本模块）不能引 store，但 connect() 已把同一 token
  // 存进模块级变量——单一来源，直接复用。
  return fetch(apiUrl(path), {
    headers: token ? { 'X-Auth-Token': token } : {},
  }).then(res => {
    if (!res.ok) throw new Error('HTTP ' + res.status)
    return res.json()
  })
}

/**
 * Register a message handler. Backward compatible — internally adds to
 * the multi-handler list. Multiple calls add multiple handlers.
 */
export function onMessage(cb: (data: any) => void) {
  addMessageHandler(cb)
}

/**
 * Add a message handler to the dispatch list.
 * Prevents duplicate registration of the same function reference.
 */
export function addMessageHandler(handler: MessageHandler) {
  if (!messageHandlers.includes(handler)) {
    messageHandlers.push(handler)
  }
}

/**
 * Remove a previously registered message handler.
 */
export function removeMessageHandler(handler: MessageHandler) {
  const idx = messageHandlers.indexOf(handler)
  if (idx >= 0) messageHandlers.splice(idx, 1)
}

export function useWebSocket() {
  onUnmounted(() => {
    // Don't disconnect on unmount - connection is shared
  })

  return {
    status: wsStatus,
    connect,
    send,
    sendRaw,
    sendHistoryRequest,
    disconnect,
    testConnection,
    httpGet,
    onMessage,
    addMessageHandler,
    removeMessageHandler,
  }
}
