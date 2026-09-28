import { reactive } from 'vue'
import { on } from './useSSE'
import { useWSAPI } from './useWSAPI'

/**
 * P30（WS14）：Dashboard Canvas 面板状态。
 *
 * 后端 agent 终答里检出**全部合法**的 ```canvas 块后逐块发布
 * `AgentEvent::CanvasOpen`；web pump 转 SSE `canvas.open`（data 展平 +
 * session_id 注入）：{session_id, html, index, chat_id, session_key}。
 *
 * 本组合式是全局单例（同 useToast/useQuestions）：initCanvas 订阅 SSE，
 * 按 (session_id, index) 幂等 upsert；面板以 iframe
 * `sandbox="allow-scripts"`（刻意不带 allow-same-origin）+ 注入严格
 * CSP meta 的 srcdoc 渲染。**v1 完全无网络**：default-src 'none'，
 * 图片/外链一律拦截，数据必须内联（`<script type="application/json">`
 * 数据岛随 srcdoc 原样保留）。
 */

export interface CanvasEntry {
  /** 同一回复内的块序号（0 起）——幂等 upsert 键之一。 */
  index: number
  /** canvas 块内容原文（HTML 文档或片段，未注入 CSP）。 */
  html: string
}

/** canvas 内容的严格 CSP（v1 无网络：仅放行内联脚本与内联样式）。 */
export const CANVAS_CSP = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'"

const CSP_META = `<meta http-equiv="Content-Security-Policy" content="${CANVAS_CSP}">`

/**
 * 把 CSP meta 注入 canvas HTML。
 *
 * 主路径走 DOMParser 级锚定注入（2026-09-26 复检挂账高优 Canvas-#1 小加固）：
 * 此前是正则级（全文首个 `<head>` 形态匹配）——模型 HTML 里若在真 head 之前
 * 出现形如 `<head>` 的字符串（注释/属性/伪元素内），meta 会插错位，CSP 对
 * 整篇失效。DOMParser 按 HTML 语义定位 head，我们的 meta 恒为 head 首子节点
 * （先于任何模型脚本/meta 解析，保证 CSP 全程生效）。内容自身若已带 CSP
 * meta，两条按浏览器语义取交集（更严者胜），不冲突。
 *
 * DOMParser 不可用时（防御分支）退回正则级三形态注入（弱保证，语义同旧版）。
 */
export function injectCanvasCsp(html: string): string {
  try {
    const doc = new DOMParser().parseFromString(html, 'text/html')
    const head = doc.head ?? doc.createElement('head')
    if (!head.parentNode && doc.documentElement) {
      doc.documentElement.insertBefore(head, doc.documentElement.firstChild)
    }
    const meta = doc.createElement('meta')
    meta.setAttribute('http-equiv', 'Content-Security-Policy')
    meta.setAttribute('content', CANVAS_CSP)
    head.insertBefore(meta, head.firstChild)
    return `<!DOCTYPE html>\n${doc.documentElement.outerHTML}`
  } catch {
    // 退化路径：正则级三形态（完整文档 / 有 html 无 head / 片段）。
    if (/<head[^>]*>/i.test(html)) {
      return html.replace(/<head[^>]*>/i, (m) => `${m}${CSP_META}`)
    }
    if (/<html[^>]*>/i.test(html)) {
      return html.replace(/<html[^>]*>/i, (m) => `${m}<head>${CSP_META}</head>`)
    }
    return `<!DOCTYPE html><html><head>${CSP_META}</head><body>${html}</body></html>`
  }
}

/** 每会话保留的画布上限（多块回复的极端场景护栏，超出丢最旧）。 */
export const MAX_CANVASES_PER_SESSION = 10

/** session_id → 该会话的画布列表（按 index 升序）。 */
const canvasesBySession = reactive<Record<string, CanvasEntry[]>>({})

let initialized = false

function upsertFromPayload(data: any) {
  if (!data || typeof data.session_id !== 'string') return
  if (typeof data.html !== 'string') return
  const sid = data.session_id
  const index = typeof data.index === 'number' ? data.index : 0
  const list = canvasesBySession[sid] ?? (canvasesBySession[sid] = [])
  const entry: CanvasEntry = { index, html: data.html }
  const existing = list.findIndex(c => c.index === index)
  if (existing === -1) {
    list.push(entry)
    if (list.length > MAX_CANVASES_PER_SESSION) list.shift()
  } else {
    // SSE 重连重放等重复帧 — 幂等覆盖（同 (session, index) 新值胜）。
    list.splice(existing, 1, entry)
  }
  list.sort((a, b) => a.index - b.index)
}

/**
 * 关闭一块画布：本地移除 + `canvas.close` 回执（fire-and-forget）。
 * v1 的 canvas.close 只作后端审计记录，无服务端面板状态可清理。
 */
function closeCanvas(sessionId: string, index: number) {
  const list = canvasesBySession[sessionId]
  if (!list) return
  const existing = list.findIndex(c => c.index === index)
  if (existing !== -1) list.splice(existing, 1)
  const { request } = useWSAPI()
  request('canvas', 'close', { session_id: sessionId }).catch(() => {
    // 回执失败不回滚本地关闭（面板状态以本地为准）。
  })
}

export function useCanvas() {
  /** 幂等初始化：AppLayout 挂载时调用一次。 */
  function initCanvas() {
    if (initialized) return
    initialized = true
    on('canvas.open', (data: any) => {
      upsertFromPayload(data)
    })
  }

  return { canvasesBySession, initCanvas, closeCanvas }
}

/** 测试辅助：重置模块级单例状态（生产代码勿用）。 */
export function _resetCanvasForTest() {
  for (const k of Object.keys(canvasesBySession)) {
    delete canvasesBySession[k]
  }
  initialized = false
}
