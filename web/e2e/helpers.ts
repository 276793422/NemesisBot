/**
 * e2e/helpers.ts — spec 共享工具。
 *
 * - newAuthedPage：带 token 的已认证页面（localStorage 直注，跳过登录表单）
 * - wsapi：页面内开原生 WebSocket 发 WSAPI request（reqId 关联），拿 response
 * - sendChat / waitAssistantReply：经 UI 发消息并等 mock 回复
 * - evidenceLines / readEvidenceSince：mock 证据文件读数（注入断言用增量读取）
 */
import fs from 'node:fs'
import { expect, type Browser, type Page } from '@playwright/test'
import { BASE_URL, E2E, EVIDENCE_LOG, SUPERVISOR_BASE } from './env'

export const AUTH_KEY = 'nemesisbot_auth_token'

/** 已认证页面：localStorage 预置 token → App.vue autoLogin 直进主界面。 */
export async function newAuthedPage(browser: Browser): Promise<Page> {
  const context = await browser.newContext()
  await context.addInitScript(
    ([key, token]) => {
      // initScript 在本 context 每个文档上执行，包括 gotoRoute 的 about:blank
      // 中转——opaque origin 上 localStorage 访问抛 SecurityError，成为
      // pageerror 污染「console 零报错」断言（2026-11-07 回归实录 executors-
      // redirect ×3 / processes-edge）。页面本身完全正常，token 在真实页面
      // （http origin）上重跑本脚本时正常预置，此处静默跳过即可。
      try {
        localStorage.setItem(key, token as string)
      } catch {
        /* opaque origin：跳过预置 */
      }
    },
    [AUTH_KEY, E2E.token] as const,
  )
  const page = await context.newPage()
  await page.goto(BASE_URL)
  return page
}

/**
 * 路由级导航（2026-11-07 webkit 稳定化）：同文档 hash 导航（已在应用页时
 * goto 到 `/#/route`）在 webkit 间歇不触发路由切换（实录 board-ui/proxy-keys
 * 失败快照主区停聊天视图；solo 空载复现不出）。先 about:blank 再全量加载
 * 目标 URL——应用直接以目标 hash 启动，路由不依赖 hashchange 事件。
 */
export async function gotoRoute(page: Page, pathWithHash: string): Promise<void> {
  await page.goto('about:blank')
  await page.goto(`${BASE_URL}${pathWithHash}`)
}

/** 未认证页面（auth spec 用）。 */
export async function newCleanPage(browser: Browser): Promise<Page> {
  const context = await browser.newContext()
  const page = await context.newPage()
  await page.goto(BASE_URL)
  return page
}

export interface WsapiResult {
  ok: boolean
  data: unknown
  error?: string
}

/**
 * 页面内直发 WSAPI（type=request / reqId 关联 / type=response 收口）。
 * 复用页面 origin 的端口与 localStorage token——url 伪装与跨源都不涉及。
 *
 * 2026-11-07 WS-56 根修：此前每次调用开一条临时 WS、用完即 close——单个
 * spec 造几十条毫秒级短命连接，撞上网关 tool_event 推送风暴时，服务端
 * 对正在收尾的 socket 写失败即弃连（TCP RST，websocket_handler.rs 的
 * run_send_writer 写失败路径），WebKit 把该网络层错误打进 console →
 * 「console 零报错」严格断言被测试自伤污染（实录 processes-edge:99
 * solo×2；应用常驻 WS 全程无恙——console 无 Reconnecting 日志佐证）。
 * 改为页面级常驻连接：window 挂单例 + reqId 多路复用；页面导航（新
 * 文档）或网关重启致断连后，下次调用自动重建。push/receive 等非
 * response 帧与本通道无关，静默忽略。
 */
export function wsapi(page: Page, module: string, cmd: string, data: unknown = null): Promise<WsapiResult> {
  return page.evaluate(
    async ([m, c, d]) => {
      interface Pending {
        resolve: (r: WsapiResult) => void
        reject: (e: Error) => void
      }
      interface Conn {
        ws: WebSocket
        pending: Map<string, Pending>
      }
      const w = window as unknown as {
        __e2eWs?: Conn
        __e2eWsConnecting?: Promise<Conn>
      }
      const token = encodeURIComponent(localStorage.getItem('nemesisbot_auth_token') ?? '')

      function rejectAll(conn: Conn, message: string): void {
        for (const [, p] of conn.pending) p.reject(new Error(message))
        conn.pending.clear()
      }

      async function getConnection(): Promise<Conn> {
        for (;;) {
          // 在途建连 → 排队共用（并发首调不重复开连接）；失败则下轮自建
          if (w.__e2eWsConnecting) {
            try {
              return await w.__e2eWsConnecting
            } catch {
              continue
            }
          }
          const existing = w.__e2eWs
          if (existing && existing.ws.readyState === WebSocket.OPEN) return existing
          // 死连接（网关重启/socket 异常）→ 在途请求诚实失败，清位重建
          if (existing) {
            rejectAll(existing, 'wsapi 连接已断')
            w.__e2eWs = undefined
          }
          const connecting = (async () => {
            const ws = new WebSocket(`ws://${location.host}/ws?token=${token}`)
            const conn: Conn = { ws, pending: new Map() }
            await new Promise<void>((resolve, reject) => {
              const timer = setTimeout(() => reject(new Error('wsapi 连接超时')), 20_000)
              ws.onopen = () => {
                clearTimeout(timer)
                w.__e2eWs = conn
                resolve()
              }
              ws.onerror = () => {
                clearTimeout(timer)
                reject(new Error('wsapi 连接失败'))
              }
            })
            return conn
          })()
          w.__e2eWsConnecting = connecting
          void connecting
            .catch(() => {})
            .then(() => {
              if (w.__e2eWsConnecting === connecting) w.__e2eWsConnecting = undefined
            })
          const conn = await connecting
          conn.ws.onmessage = (ev) => {
            const msg = JSON.parse(ev.data as string)
            if (msg.type === 'response' && typeof msg.reqId === 'string') {
              const p = conn.pending.get(msg.reqId)
              if (p) {
                conn.pending.delete(msg.reqId)
                p.resolve(
                  msg.error ? { ok: false, data: null, error: msg.error } : { ok: true, data: msg.data ?? null },
                )
              }
            }
          }
          conn.ws.onclose = () => {
            // 断连：在途请求诚实失败；单例清空，下次调用重建
            if (w.__e2eWs === conn) {
              rejectAll(conn, 'wsapi 连接已断')
              w.__e2eWs = undefined
            }
          }
          return conn
        }
      }

      const reqId = `e2e-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
      const conn = await getConnection()
      return await new Promise<WsapiResult>((resolve, reject) => {
        conn.pending.set(reqId, { resolve, reject })
        const timer = setTimeout(() => {
          conn.pending.delete(reqId)
          reject(new Error(`wsapi ${m}.${c} 响应超时`))
        }, 20_000)
        try {
          conn.ws.send(JSON.stringify({ type: 'request', module: m, cmd: c, reqId, data: d }))
        } catch {
          clearTimeout(timer)
          conn.pending.delete(reqId)
          reject(new Error(`wsapi ${m}.${c} 发送失败（连接刚断）`))
        }
      })
    },
    [module, cmd, data] as const,
  )
}

/** 展开会话侧栏并等「＋ 新建对话」可见（2026-11-07 充分测试战役 webkit
 *  全量实录根修）：旧的「瞬时 isVisible() → 可见才点」落在 ChatPanel 挂载
 *  窗口内 = false → 跳过点击 → 侧栏永不开，后续 .fn-new-chat 死等 10s
 *  超时（慢浏览器全量下窗口放大；单跑恒过）。改为 title 精确选择器 +
 *  click auto-wait（等出现可点），已开（fn-new-chat 已可见）幂等直返，
 *  返回即「侧栏已开」确定性态。executors-redirect / disconnect-recovery /
 *  project-history / resync / subagent-capsule-visual 七处调用点同款替换。 */
export async function openSessionSidebar(page: Page): Promise<void> {
  const newChat = page.locator('.fn-new-chat')
  if (await newChat.isVisible().catch(() => false)) return
  await page.locator('.toolbar-toggle[title="显示会话列表"]').first().click()
  await newChat.waitFor({ state: 'visible', timeout: 15_000 })
}

/** 经 UI 发聊天消息（真实用户路径：输入框 + 发送按钮）。 */
export async function sendChat(page: Page, text: string): Promise<void> {
  const input = page.locator('.chat-input-area textarea')
  await input.fill(text)
  await page.locator('.chat-input-area button.btn-primary').click()
}

/** 等 assistant 气泡出现含指定文本的最新一条。 */
export async function waitAssistantReply(page: Page, contains: string, timeout = 30_000): Promise<void> {
  await expect(page.locator('.message.assistant .message-bubble').last()).toContainText(contains, { timeout })
}

/** mock 证据文件当前全部行。 */
export function evidenceLines(): string[] {
  try {
    return fs.readFileSync(EVIDENCE_LOG, 'utf-8').split(/\r?\n/).filter((l) => l.length > 0)
  } catch {
    return []
  }
}

/** 增量读取：返回 lineCountAfter - lineCountBefore 区间内的新行（注入断言用）。 */
export function evidenceSince(beforeCount: number): string[] {
  return evidenceLines().slice(beforeCount)
}

/** 当前 mock 证据行数（发送前打点用）。 */
export function evidenceCount(): number {
  return evidenceLines().length
}

/**
 * 重启 gateway（project-history「场景 A」复现用：目录缺失须在 gateway 启动
 * 前成立，项目 loop 才会不 spawn → 侧栏组头置灰。运行中删目录是 lenient
 * 设计——loop 继续跑、running 保持 true，复现不了置灰态）。
 *
 * 经 gateway supervisor HTTP 执行（gateway spawn 权归 supervisor——Playwright
 * worker 带 job object 清理语义，spec 内 spawn 的 gateway 会在 worker 回收
 * （文件边界）时被连带杀掉，实录见 gateway-supervisor.mjs 头注释）。
 * supervisor 在新 gateway health OK 后才应答；本函数再补一发 web 端口
 * 探活（health 覆盖 gateway.port，web server 是另一个 bind）。
 *
 * 磁盘态（注册表/会话 sidecar/chat_log/session_bindings）重启后全部存活。
 */
export async function restartGateway(): Promise<void> {
  const res = await fetch(`${SUPERVISOR_BASE}/restart`, {
    method: 'POST',
    signal: AbortSignal.timeout(150_000),
  })
  const body = (await res.json().catch(() => null)) as { ok?: boolean; error?: string } | null
  if (!res.ok || !body?.ok) {
    throw new Error(`gateway supervisor /restart 失败（HTTP ${res.status}）：${body?.error ?? '<无响应体>'}`)
  }
  const webDeadline = Date.now() + 30_000
  for (;;) {
    try {
      const r = await fetch(BASE_URL)
      if (r.ok) return
    } catch {
      /* 未就绪 */
    }
    if (Date.now() > webDeadline) throw new Error('gateway 重启后 web 端口 30s 未就绪')
    await new Promise((r) => setTimeout(r, 500))
  }
}

/**
 * WS message 帧直发 chat.send（绕过 UI，种子数据用——聊天 UI 路径由 chat
 * spec 单独覆盖）。等 receive 帧 role=assistant 即收口，返回回复文本。
 */
export function wsChatSend(page: Page, sessionId: string, content: string): Promise<string> {
  return page.evaluate(
    async ([sid, text]) => {
      const reqId = `e2e-chat-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
      const token = encodeURIComponent(localStorage.getItem('nemesisbot_auth_token') ?? '')
      const ws = new WebSocket(`ws://${location.host}/ws?token=${token}`)
      return await new Promise<string>((resolve, reject) => {
        const timer = setTimeout(() => {
          try {
            ws.close()
          } catch {
            /* ignore */
          }
          reject(new Error(`chat 种子消息回复超时（session=${sid}）`))
        }, 60_000)
        ws.onopen = () =>
          ws.send(
            JSON.stringify({ type: 'message', module: 'chat', cmd: 'send', reqId, data: { content: text, session_id: sid } }),
          )
        ws.onmessage = (ev) => {
          const msg = JSON.parse(ev.data as string)
          if (
            msg.type === 'message' &&
            msg.module === 'chat' &&
            msg.cmd === 'receive' &&
            msg.data?.role === 'assistant' &&
            msg.data?.session_id === sid
          ) {
            clearTimeout(timer)
            resolve(String(msg.data?.content ?? ''))
          }
        }
        ws.onerror = () => {
          clearTimeout(timer)
          reject(new Error('chat 种子消息连接失败'))
        }
      })
    },
    [sessionId, content] as const,
  )
}
