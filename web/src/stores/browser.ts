/**
 * 浏览器接管 store（2026-11-15，计划 D10/D15）。
 *
 * 执行单元页【浏览器】TAB 与 ChatPanel 点名面板两表面共用的单一数据面：
 * - 会话列表 / tab 列表 / 操作日志，全部走 browser.* WSAPI（handlers/browser.rs）
 * - 设置读写走既有 config.get / config.set_field（config.json `browser` 段，
 *   gateway prefs_getter 每次 spawn 现读——改完即生效，无需额外通道）
 * - 编排操作实时事件 = SSE `browser_event`（agent_build.rs sink → EventHub），
 *   订阅方把帧并入 logs 头部（AI 动作路走 tool_event，本 store 不管）
 *
 * 轮询纪律：会话列表 3s 慢拍（运行实例卡）；tab 列表仅在面板展开时 2s
 * 跟随选中实例——两层各自负责自己的定时器，store 只提供数据与动作。
 */
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'

export interface BrowserSessionSummary {
  id: string
  seq: number
  kind: string
  kind_name: string
  phase2: boolean
  executable: string
  profile: string
  port: number
  started_at: string
  pid: number
  working_tab: string | null
  operating: boolean
  /** F27（2026-11-15 五审）：进程存活投影——手关窗口后后端 reaper 置位，前端凭此渲染「已退出」+ 跳过死实例轮询。 */
  alive: boolean
}

export interface BrowserTabRow {
  target_id: string
  title: string
  url: string
  attached: boolean
  working: boolean
  load_state: string
  operating: boolean
}

export interface BrowserLogRow {
  ts_ms: number
  session_id: string
  action: string
  target: string
  ok: boolean
  detail: string
  duration_ms: number
}

export interface DetectedBrowserRow {
  kind: string
  name: string
  path: string | null
  phase2: boolean
  source: string
}

export interface BrowserSettings {
  kind: string
  executable: string
}

export const useBrowserStore = defineStore('browser', () => {
  const { request } = useWSAPI()

  const sessions = ref<BrowserSessionSummary[]>([])
  const detected = ref<DetectedBrowserRow[] | null>(null)
  const settings = ref<BrowserSettings>({ kind: 'auto', executable: '' })
  /** tab 列表按 session_id 缓存（点名面板/列表子页签同源）。 */
  const tabsBySession = ref<Record<string, BrowserTabRow[]>>({})
  const logs = ref<BrowserLogRow[]>([])

  async function fetchSessions() {
    try {
      const r = await request('browser', 'sessions')
      sessions.value = (r?.sessions as BrowserSessionSummary[]) || []
    } catch { /* WS 未就绪——下轮再试 */ }
  }

  async function fetchTabs(sessionId?: string) {
    const r = await request('browser', 'tabs', sessionId ? { session_id: sessionId } : {})
    const sid = (r?.session_id as string) || ''
    if (sid) tabsBySession.value = { ...tabsBySession.value, [sid]: (r?.tabs as BrowserTabRow[]) || [] }
    return r
  }

  async function spawn(kind?: string, executable?: string) {
    const r = await request('browser', 'spawn', {
      ...(kind ? { kind } : {}),
      ...(executable ? { executable } : {}),
    })
    await fetchSessions()
    return r?.session as BrowserSessionSummary
  }

  async function attach(sessionId: string, targetId: string) {
    const r = await request('browser', 'attach', { session_id: sessionId, target_id: targetId })
    // 点名即时反馈：本地更新 working 指针（下轮 tab 快照校正）。
    const tabs = tabsBySession.value[sessionId]
    if (tabs) {
      tabsBySession.value = {
        ...tabsBySession.value,
        [sessionId]: tabs.map((t) => ({ ...t, working: t.target_id === targetId })),
      }
    }
    await fetchSessions()
    return r
  }

  async function release(sessionId: string) {
    const r = await request('browser', 'release', { session_id: sessionId })
    const tabs = tabsBySession.value[sessionId]
    if (tabs) {
      tabsBySession.value = {
        ...tabsBySession.value,
        [sessionId]: tabs.map((t) => ({ ...t, working: false })),
      }
    }
    await fetchSessions()
    return r
  }

  async function closeSession(sessionId: string) {
    const r = await request('browser', 'close', { session_id: sessionId })
    await fetchSessions()
    return r
  }

  async function fetchLogs(sessionId?: string, limit = 200) {
    try {
      const r = await request('browser', 'logs', {
        ...(sessionId ? { session_id: sessionId } : {}),
        limit,
      })
      logs.value = (r?.logs as BrowserLogRow[]) || []
    } catch { /* WS 未就绪 */ }
  }

  async function fetchDetected(force = false) {
    if (detected.value && !force) return detected.value
    try {
      const r = await request('browser', 'detect')
      detected.value = (r?.browsers as DetectedBrowserRow[]) || []
    } catch { /* WS 未就绪 */ }
    return detected.value
  }

  async function fetchSettings() {
    try {
      const r = await request('config', 'get')
      const seg = r?.config?.browser
      settings.value = {
        kind: typeof seg?.kind === 'string' && seg.kind ? seg.kind : 'auto',
        executable: typeof seg?.executable === 'string' ? seg.executable : '',
      }
    } catch { /* WS 未就绪 */ }
  }

  async function saveSettings(next: BrowserSettings) {
    // F47（2026-11-16 十审）：非 custom kind 落盘必须清空 executable——
    // 后端解析序「配置 executable > 配置 kind」（resolve_browser），视图隐藏
    // 输入框里的 stale 路径若照抄落盘会静默压过新选的 kind（UI 显示 edge、
    // 实际打开旧路径）。归一收在 store 单点，视图不重复判断；custom 形态
    // 路径原样透传。
    const executable = next.kind === 'custom' ? next.executable : ''
    await request('config', 'set_field', { path: 'browser.kind', value: next.kind })
    await request('config', 'set_field', { path: 'browser.executable', value: executable })
    settings.value = { kind: next.kind, executable }
  }

  /** SSE browser_event 帧并入 logs 头部（编排操作路实时投影）。 */
  function ingestEvent(ev: BrowserLogRow) {
    logs.value = [ev, ...logs.value].slice(0, 400)
  }

  return {
    sessions,
    detected,
    settings,
    tabsBySession,
    logs,
    fetchSessions,
    fetchTabs,
    spawn,
    attach,
    release,
    closeSession,
    fetchLogs,
    fetchDetected,
    fetchSettings,
    saveSettings,
    ingestEvent,
  }
})
