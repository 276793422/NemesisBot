import { onUnmounted } from 'vue'
import { apiUrl } from '../lib/appBase'
import { currentAuthToken } from './useWebSocket'

type EventHandler = (data: any) => void

let eventSource: EventSource | null = null;
const eventHandlers: Record<string, EventHandler[]> = {}

// L2（devtool-upgrade 阶段 6）：'resync' = 服务端提示缺口已滑出重放窗口
// （或网关重启 seq 重置），订阅方应全量刷新兜底。
// SB（2026-09-17）：'session.created' = 会话 jsonl 首行落盘（隐式物化），
// 侧栏订阅后 force 刷新列表（修「首条消息隐式创建的会话不进侧栏」）。
// P8（2026-09-21）：'chat.activity' = 某会话 chat_event_log 落了新帧
//（chat 行或工具事件），全局广播 {session_id, seq}——其他标签/端据此
// 感知落后并全量刷新（本端 WS 实时帧先行，seq 游标短路零开销）。
// P30（WS14）：'canvas.open' = agent 终答检出合法 ```canvas 块（Canvas
// 面板渲染，iframe sandbox + 严格 CSP，v1 无网络）。
// L10（2026-10-13）：'provider_stream_event' = provider 流式输出增量帧
//（ephemeral：无 id 不参与补拉——断线窗口内的流不可回放，Logs Dashboard
// 实时页 ProviderStreamPanel 消费）。
const EVENT_TYPES = ['log', 'status', 'security-alert', 'scanner-progress', 'cluster-event', 'heartbeat', 'memory-setup', 'board-changed', 'usage-changed', 'approval-requested', 'approval-resolved', 'approval-remote-requested', 'approval-remote-resolved', 'question-asked', 'question-resolved', 'board.plan_started', 'board.plan_ready', 'board.plan_failed', 'resync', 'session.created', 'editor-mode', 'chat.activity', 'canvas.open', 'provider_stream_event']

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
    // F1 跟进修复（2026-10-18）：autoLogin 认证链（插件窗口 __DASHBOARD_TOKEN__
    // 注入 / URL hash token / 桌面 runtime 事件）从不写 localStorage，而下方
    // URL 之前只读 localStorage——注入会话生成不带 ?token= 的请求，REST 统一
    // 鉴权（2026-09-22）下 SSE 恒 401（EventSource 对非 200 fail-fast 关闭且
    // 不自动重试），Logs 页「正在连接 SSE…」永挂；同刻 WS 携内存 token 正常，
    // 恰成「WS 连上、SSE 401」的现场指纹。改与 httpGet 同源（useWebSocket
    // 模块级 token，connect() 注入），localStorage 兜底浏览器手动登录路径。
    const stored = currentAuthToken() || localStorage.getItem('nemesisbot_auth_token')
    const sseUrl = stored ? `${apiUrl('/api/events/stream')}?token=${encodeURIComponent(stored)}` : apiUrl('/api/events/stream')
    eventSource = new EventSource(sseUrl)

    eventSource.onopen = () => {
      console.log('[NemesisAPI] SSE connected')
      // board-changed 是 fire-and-forget 通知（无 id/seq，不参与服务端重放）：
      // 断线窗口内的事件永久丢失，board 面板会停在旧数据直到下一次无关
      // 写入（实证 2026-10-15：gateway 重启后派发链全程列表冻结）。连接
      // 成功（含浏览器自动重连）即补发一次本地合成事件，各面板静默换新
      // 兜底——与 L2 resync 同思路，只是该事件本身无游标可补拉。
      dispatch('board-changed', { ts: new Date().toISOString(), reconnect: true })
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
