/**
 * srp-del-behavior.mjs — 等待队列逐条删除 UI 行为验证（2026-11-27）。
 * 对 demo gateway（37355，target/demo-home），真模型链：
 *
 * T1 核心行为证明：长轮 busy 期间播种 4 条（插队甲/乙 + 排队甲/乙）→
 *    UI 点 ✕ 删「插队乙」和「排队甲」→ 让轮自然跑完 → 落盘 jsonl 断言：
 *    - 被删两条无 user 行（永不执行）
 *    - 剩余两条有 user 行且按序（插队甲在前、排队乙在后）
 *    - 模型响应内容对应（甲/乙收到，被删者无响应）
 *
 * T2 失败路径：面板行在场时后台 WSAPI 偷删同一条 → UI 点该行 ✕ →
 *    错误 toast（诚实文案）+ 面板自动刷新行消失。
 *
 * 用法：node web/e2e/srp-del-behavior.mjs
 */
import path from 'node:path'
import fs from 'node:fs'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..')
const require = createRequire(path.join(repoRoot, 'web', 'package.json'))
const { chromium } = require('playwright')

const BASE_URL = process.env.LIVE_BASE_URL ?? 'http://127.0.0.1:37355'
const TOKEN = process.env.LIVE_TOKEN ?? 'demo-local-token'
const SHOT_DIR = path.join(repoRoot, 'target', 'srp-del-behavior-shots')
const SESSION_LOGS = path.join(
  repoRoot, 'target', 'demo-home', '.nemesisbot', 'workspace', 'logs', 'session_logs',
)
fs.mkdirSync(SHOT_DIR, { recursive: true })

const consoleErrors = []
const browser = await chromium.launch()
const context = await browser.newContext({ viewport: { width: 1600, height: 950 } })
await context.addInitScript(([k, token]) => {
  try { localStorage.setItem(k, token) } catch {}
}, ['nemesisbot_auth_token', TOKEN])
const page = await context.newPage()
page.on('console', (m) => { if (m.type() === 'error') consoleErrors.push(m.text()) })

// 抓应用自己 WS 连接上 session.resources 的响应帧（waiting_queue 形态取证）。
const resFrames = []
page.on('websocket', (ws) => {
  ws.on('framereceived', (f) => {
    const s = String(f.payload ?? '')
    if (s.includes('waiting_queue') || s.includes('"resources"')) {
      try {
        const m = JSON.parse(s)
        const wq = m.data?.categories?.waiting_queue
        resFrames.push({
          t: Date.now(),
          reqId: String(m.reqId ?? '?').slice(0, 24),
          wqStatus: wq?.status ?? '(absent)',
          steer: wq?.summary?.steer?.length ?? -1,
          follow: wq?.summary?.follow_up?.length ?? -1,
          openLen: s.length,
        })
      } catch { resFrames.push({ t: Date.now(), raw: s.slice(0, 120) }) }
    }
  })
})

// 页面内常驻 WS 单例发 WSAPI（srp 系同款收口）。
async function wsapi(moduleName, cmd, data = null) {
  return page.evaluate(async ([m, c, d]) => {
    const w = window
    const token = encodeURIComponent(localStorage.getItem('nemesisbot_auth_token') ?? '')
    function conn() {
      if (w.__shotWs?.readyState === WebSocket.OPEN) return Promise.resolve(w.__shotWs)
      if (!w.__shotConnecting) {
        w.__shotConnecting = new Promise((resolve, reject) => {
          const ws = new WebSocket(`ws://${location.host}/ws?token=${token}`)
          const timer = setTimeout(() => reject(new Error('ws 连接超时')), 20000)
          ws.onopen = () => { clearTimeout(timer); w.__shotWs = ws; w.__shotPending = new Map(); resolve(ws) }
          ws.onerror = () => { clearTimeout(timer); reject(new Error('ws 连接失败')) }
          ws.onmessage = (ev) => {
            const msg = JSON.parse(ev.data)
            if (msg.type === 'response' && typeof msg.reqId === 'string') {
              const p = w.__shotPending?.get(msg.reqId)
              if (p) { w.__shotPending.delete(msg.reqId); p(msg.error ? { ok: false, error: msg.error } : { ok: true, data: msg.data ?? null }) }
            }
          }
        })
      }
      return w.__shotConnecting
    }
    const ws = await conn()
    const reqId = `shot-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { w.__shotPending.delete(reqId); reject(new Error('wsapi 超时')) }, 30000)
      w.__shotPending.set(reqId, (r) => { clearTimeout(timer); resolve(r) })
      ws.send(JSON.stringify({ type: 'request', module: m, cmd: c, data: d, reqId }))
    })
  }, [moduleName, cmd, data])
}

// chat.send 是三级协议 message 帧（type:'message'）——裸发排队条唯一正确形态。
function sendChatFrame(sid, content) {
  return page.evaluate(([s, c]) => {
    window.__shotWs.send(JSON.stringify({
      type: 'message', module: 'chat', cmd: 'send',
      data: s === '' ? { content: c } : { content: c, session_id: s },
    }))
  }, [sid, content])
}

async function send(text) {
  const input = page.locator('textarea[placeholder*="输入消息"]')
  await input.fill(text)
  await input.press('Control+Enter')
}

// 通用 Node 侧轮询。
async function pollNode(desc, fn, { timeout = 30000, interval = 600 } = {}) {
  const deadline = Date.now() + timeout
  let last = null
  while (Date.now() < deadline) {
    last = await fn()
    if (last) return last
    await new Promise((r) => setTimeout(r, interval))
  }
  throw new Error(`${desc} 轮询超时（${timeout}ms）——最后现场: ${JSON.stringify(last)?.slice(0, 300)}`)
}

async function wq(sid) {
  const r = await wsapi('session', 'resources', { session_id: sid })
  return r.data?.categories?.waiting_queue
}

// 长轮完成判定锚 jsonl 终稿（exec.running 形态不稳，别拿它判完成）：
// 最后一条排队条目的模型回复出现 = 长轮+插队续轮+排队新轮全部跑完。
function jsonlText(sid) {
  const p = path.join(SESSION_LOGS, `agent_main_session_${sid}.jsonl`)
  return fs.existsSync(p) ? fs.readFileSync(p, 'utf8') : ''
}

async function waitAllExecuted(sid, finalMarker, timeoutMs = 300000) {
  await pollNode('[T1] 全部执行完（jsonl 终稿）', () => {
    const t = jsonlText(sid)
    return t.includes(finalMarker) ? t : null
  }, { timeout: timeoutMs, interval: 3000 })
}

// 资源窗展开态跨页面加载持久化——面板可能已挂载且 resp 是陈旧数据（本会话
// 播种前的快照，队列卡不渲染）。收起态 v-if 卸载组件，强制关→开 = 重挂载
// = sessionId watch immediate 重拉新鲜数据。
async function openPanel() {
  const toggle = page.locator('button.toolbar-toggle[title*="资源窗"]').first()
  if ((await toggle.getAttribute('title')) === '收起资源窗') {
    await toggle.click()
    await page.waitForTimeout(400)
  }
  const closed = page.locator('button.toolbar-toggle[title="打开资源窗"]').first()
  await closed.click()
  await page.waitForSelector('.srp-root', { timeout: 10000 })
}

async function newSessionIdle() {
  const sidebarToggle = page.locator('button.toolbar-toggle[title="显示会话列表"]')
  if (await sidebarToggle.isVisible().catch(() => false)) await sidebarToggle.click()
  await page.waitForTimeout(600)
  await page.locator('button.fn-new-chat').first().click()
  await page.waitForSelector('.session-item.active', { timeout: 15000 })
  await page.waitForTimeout(800)
  const sid = ((await page.locator('.session-item.active').first()
    .getAttribute('data-session-id').catch(() => null)) ?? '')
  if (sid === '') throw new Error('新会话 sid 为空——侧栏活动行缺 data-session-id')
  // WS 单例预热——后续 sendChatFrame 依赖 window.__shotWs 已连接。
  await wq(sid).catch(() => null)
  return sid
}

// 长轮（单次长生成、无工具边界——插话条稳定留队的确定性载体）。
const LONG_TURN = '请从1数到5000，每行一个数字。只输出数字，不要任何解释。'

await page.goto(BASE_URL)
await page.waitForTimeout(3000)

// ==========================================================================
console.log('===== T1：删除的条永不执行，剩余条按序执行 =====')
{
  const sid = await newSessionIdle()
  console.log(`[T1] 会话 ${sid}`)

  // 长轮占住会话（新会话首条消息即长轮——物化由它完成）。
  await send(LONG_TURN)
  await page.waitForSelector('.btn-stop', { timeout: 60000 })
  await page.waitForTimeout(3000) // 等首次 LLM 调用在飞（调用前注入窗口已过）
  console.log('[T1] 长轮进行中')

  // 播种 4 条：插队甲/乙（UI 显式 `!`）+ 排队甲/乙（裸 message 帧）。
  await send('!插队甲号任务：收到后只回复「甲号插队已收到」')
  await send('!插队乙号任务：收到后只回复「乙号插队已收到」')
  await sendChatFrame(sid, '排队甲号任务：收到后只回复「甲号排队已收到」')
  await sendChatFrame(sid, '排队乙号任务：收到后只回复「乙号排队已收到」')
  const seeded = await pollNode('[T1] 四条入队', async () => {
    const w = await wq(sid)
    return (w?.summary?.steer?.length ?? 0) === 2 && (w?.summary?.follow_up?.length ?? 0) === 2 ? w : null
  }, { timeout: 20000 })
  console.log(`[T1] 播种完成 steer=2 follow_up=2 total=${seeded.total}`)

  await openPanel()
  await page.screenshot({ path: path.join(SHOT_DIR, 't1-05-panel-opened.png') })
  const dbg = await page.evaluate(() => {
    const root = document.querySelector('.srp-root')
    const cards = Array.from(document.querySelectorAll('.srp-root .srp-card')).map((e) => e.className)
    const text = root?.textContent ?? ''
    const rect = root?.getBoundingClientRect()
    return {
      rootExists: !!root,
      rect: rect ? { x: rect.x, y: rect.y, w: rect.width, h: rect.height } : null,
      cards,
      hasQueueWord: text.includes('等待队列'),
      fullTextTail: text.slice(-600),
    }
  })
  dbg.resFrameCount = resFrames.length
  dbg.resFrames = resFrames
  console.log(`[T1] 面板调试: ${JSON.stringify(dbg, null, 1)}`)
  await page.waitForSelector('.srp-queue', { timeout: 15000 })
  await page.waitForFunction(() => document.querySelectorAll('.srp-queue .srp-wq-row').length === 4, null, { timeout: 15000 })
  await page.waitForTimeout(500)
  await page.screenshot({ path: path.join(SHOT_DIR, 't1-10-four-rows.png') })

  // UI 删除：插队乙（row[1]）→ 删后行序 [甲, 排队甲, 排队乙] → 删排队甲（row[1]）。
  const rows = page.locator('.srp-queue .srp-wq-row')
  await rows.nth(1).hover()
  await rows.nth(1).locator('.srp-wq-del').click()
  await page.waitForFunction(() => document.querySelectorAll('.srp-queue .srp-wq-row').length === 3, null, { timeout: 15000 })
  console.log('[T1] 插队乙已删（剩 3 行）')
  await rows.nth(1).hover()
  await rows.nth(1).locator('.srp-wq-del').click()
  await page.waitForFunction(() => document.querySelectorAll('.srp-queue .srp-wq-row').length === 2, null, { timeout: 15000 })
  console.log('[T1] 排队甲已删（剩 2 行）')

  // 剩余行内容断言（DOM 面）：[插队甲, 排队乙]。
  const remainTexts = await rows.allTextContents()
  console.log(`[T1] 剩余行: ${JSON.stringify(remainTexts)}`)
  if (!remainTexts[0].includes('插队甲号') || !remainTexts[1].includes('排队乙号')) {
    throw new Error(`剩余行序不符: ${JSON.stringify(remainTexts)}`)
  }
  await page.screenshot({ path: path.join(SHOT_DIR, 't1-20-two-rows.png') })

  // 让轮自然跑完：锚 jsonl 终稿（最后排队条的模型回复出现）。
  await waitAllExecuted(sid, '乙号排队已收到')
  const wEnd = await wq(sid)
  console.log(`[T1] 全部执行完（终稿到），队列残留 total=${wEnd?.total ?? 0}`)
  if ((wEnd?.total ?? 0) !== 0) throw new Error(`执行完成后队列应清空: ${JSON.stringify(wEnd)}`)

  // 落盘断言：被删两条无 user 行；剩余两条有且按序。
  const raw = fs.readFileSync(path.join(SESSION_LOGS, `agent_main_session_${sid}.jsonl`), 'utf8')
  const rowsAll = raw.split('\n').filter((l) => l.trim() !== '').map((l) => JSON.parse(l))
  const userRows = rowsAll.filter((r) => r.role === 'user')
  const has = (needle) => userRows.some((r) => (r.content ?? '').includes(needle))
  const idxOf = (needle) => userRows.findIndex((r) => (r.content ?? '').includes(needle))
  const checks = {
    插队甲_执行: has('插队甲号任务'),
    插队乙_永不执行: !has('插队乙号任务'),
    排队甲_永不执行: !has('排队甲号任务'),
    排队乙_执行: has('排队乙号任务'),
  }
  console.log(`[T1] user 行总数=${userRows.length} 断言: ${JSON.stringify(checks)}`)
  if (!checks.插队甲_执行 || !checks.排队乙_执行 || checks.插队乙_永不执行 !== true || checks.排队甲_永不执行 !== true) {
    throw new Error(`落盘断言失败: ${JSON.stringify(checks)}`)
  }
  const iA = idxOf('插队甲号任务')
  const iB = idxOf('排队乙号任务')
  if (iA < 0 || iB < 0 || iA >= iB) throw new Error(`执行序不符: 插队甲@${iA} 排队乙@${iB}`)
  console.log(`[T1] 执行序正确: 插队甲@user${iA} → 排队乙@user${iB}`)

  // 响应面对应：甲/乙收到在场，被删者响应缺席。
  const allText = rowsAll.map((r) => r.content ?? '').join('\n')
  const resp = {
    甲号插队已收到: allText.includes('甲号插队已收到'),
    乙号插队已收到_缺席: !allText.includes('乙号插队已收到'),
    甲号排队已收到_缺席: !allText.includes('甲号排队已收到'),
    乙号排队已收到: allText.includes('乙号排队已收到'),
  }
  console.log(`[T1] 响应断言: ${JSON.stringify(resp)}`)
  if (!resp.甲号插队已收到 || !resp.乙号排队已收到 || !resp.乙号插队已收到_缺席 || !resp.甲号排队已收到_缺席) {
    throw new Error(`响应断言失败: ${JSON.stringify(resp)}`)
  }
  await page.screenshot({ path: path.join(SHOT_DIR, 't1-30-executed-chat.png') })
  console.log('[T1] ✅ 通过：删除的条永不执行，剩余条按序执行且响应对应')
}

// ==========================================================================
console.log('===== T2：后台偷删后 UI 点击 → 诚实错误 toast + 自动刷新 =====')
{
  const sid = await newSessionIdle()
  console.log(`[T2] 会话 ${sid}`)
  await send(LONG_TURN)
  await page.waitForSelector('.btn-stop', { timeout: 60000 })
  await page.waitForTimeout(3000)
  await sendChatFrame(sid, '排队丙号任务：收到后只回复「丙号排队已收到」')
  await pollNode('[T2] 条目入队', async () => {
    const w = await wq(sid)
    return (w?.summary?.follow_up?.length ?? 0) === 1 ? w : null
  }, { timeout: 20000 })
  await openPanel()
  await page.waitForSelector('.srp-queue', { timeout: 15000 })
  await page.waitForFunction(() => document.querySelectorAll('.srp-queue .srp-wq-row').length === 1, null, { timeout: 15000 })
  await page.waitForTimeout(400)

  // 后台偷删（绕过 UI——模拟另一端/竞态消费），随即 UI 点同一行的 ✕。
  const victim = (await wq(sid))?.summary?.follow_up?.[0]
  const sneak = await wsapi('session', 'queue_remove', {
    session_id: sid, queue: 'follow_up', index: 0,
    sender_id: victim.sender_id, timestamp: victim.timestamp,
  })
  if (sneak.ok !== true) throw new Error(`后台偷删失败: ${JSON.stringify(sneak)}`)
  console.log('[T2] 后台偷删完成（面板尚不知情）——立即点 UI ✕')
  await page.locator('.srp-queue .srp-wq-row').first().hover()
  await page.locator('.srp-queue .srp-wq-row').first().locator('.srp-wq-del').click()
  await page.waitForTimeout(1200)

  const toastText = (await page.locator('.toast-container, [class*="toast"]').allTextContents()).join(' | ')
  console.log(`[T2] toast: ${toastText}`)
  if (!toastText.includes('删除失败') || !(toastText.includes('已不在队列') || toastText.includes('队列已变化'))) {
    throw new Error(`错误 toast 缺失或文案不符: ${toastText}`)
  }
  await page.screenshot({ path: path.join(SHOT_DIR, 't2-10-error-toast.png') })

  // 自动刷新兜底：行消失（卡回 empty）。
  await page.waitForFunction(
    () => !document.querySelector('.srp-queue'), null, { timeout: 15000 },
  )
  console.log('[T2] 面板已自动刷新，队列卡消失')
  await page.screenshot({ path: path.join(SHOT_DIR, 't2-20-refreshed.png') })
  console.log('[T2] ✅ 通过：诚实错误 toast + 自动刷新，无误删')

  // 收尾：停掉长轮（后台偷删的那条已不会执行，长轮不必跑完）。
  const stopBtn = page.locator('button.btn-stop').first()
  if (await stopBtn.isVisible().catch(() => false)) {
    await stopBtn.click()
    console.log('[T2] 已点停止生成收尾')
  }
}

console.log(`[CONSOLE_ERRORS] ${consoleErrors.length}`)
for (const e of consoleErrors) console.log(`  ${e}`)
await browser.close()
console.log('ALL DONE')
