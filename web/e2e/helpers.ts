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
      localStorage.setItem(key, token as string)
    },
    [AUTH_KEY, E2E.token] as const,
  )
  const page = await context.newPage()
  await page.goto(BASE_URL)
  return page
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
 */
export function wsapi(page: Page, module: string, cmd: string, data: unknown = null): Promise<WsapiResult> {
  return page.evaluate(
    async ([m, c, d]) => {
      const reqId = `e2e-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
      const token = encodeURIComponent(localStorage.getItem('nemesisbot_auth_token') ?? '')
      const ws = new WebSocket(`ws://${location.host}/ws?token=${token}`)
      const result = await new Promise<WsapiResult>((resolve, reject) => {
        const timer = setTimeout(() => {
          try {
            ws.close()
          } catch {
            /* ignore */
          }
          reject(new Error(`wsapi ${m}.${c} 响应超时`))
        }, 20_000)
        ws.onopen = () => ws.send(JSON.stringify({ type: 'request', module: m, cmd: c, reqId, data: d }))
        ws.onmessage = (ev) => {
          const msg = JSON.parse(ev.data as string)
          if (msg.type === 'response' && msg.reqId === reqId) {
            clearTimeout(timer)
            resolve(msg.error ? { ok: false, data: null, error: msg.error } : { ok: true, data: msg.data ?? null })
          }
        }
        ws.onerror = () => {
          clearTimeout(timer)
          reject(new Error(`wsapi ${m}.${c} 连接失败`))
        }
      })
      try {
        ws.close()
      } catch {
        /* ignore */
      }
      return result
    },
    [module, cmd, data] as const,
  )
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
