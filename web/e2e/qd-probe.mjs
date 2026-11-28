/**
 * qd-probe.mjs — 等待队列删除批探针（2026-11-27）。
 * node web/e2e/qd-probe.mjs [clean]
 *   无参：打印 legacy 会话 waiting_queue 全量 + busy 态。
 *   clean：把队列里现存条目逐条 queue_remove（身份对取自本 probe 的 peek）。
 */
import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..')
const require = createRequire(path.join(repoRoot, 'web', 'package.json'))
const WebSocket = require('ws')

const BASE = process.env.LIVE_BASE_URL ?? 'ws://127.0.0.1:37355'
const TOKEN = process.env.LIVE_TOKEN ?? 'demo-local-token'
const wsUrl = `${BASE}/ws?token=${encodeURIComponent(TOKEN)}`
const ws = new WebSocket(wsUrl)
const pending = new Map()
let reqSeq = 0

function call(moduleName, cmd, data = null) {
  const reqId = `probe-${++reqSeq}`
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(reqId); reject(new Error('wsapi 超时')) }, 15000)
    pending.set(reqId, (r) => { clearTimeout(timer); resolve(r) })
    ws.send(JSON.stringify({ type: 'request', module: moduleName, cmd, data, reqId }))
  })
}

ws.on('open', async () => {
  ws.on('message', (raw) => {
    const msg = JSON.parse(raw.toString())
    if (msg.type === 'response' && typeof msg.reqId === 'string') {
      const p = pending.get(msg.reqId)
      if (p) { pending.delete(msg.reqId); p(msg.error ? { ok: false, error: msg.error } : { ok: true, data: msg.data ?? null }) }
    }
  })
  const mode = process.argv[2] ?? ''
  const sid = process.argv[3] ?? ''
  const r = await call('session', 'resources', { session_id: sid })
  const wq = r.data?.categories?.waiting_queue
  console.log(`[WQ] status=${wq?.status} total=${wq?.total}`)
  for (const [qi, arr] of [['steer', wq?.summary?.steer ?? []], ['follow_up', wq?.summary?.follow_up ?? []]]) {
    for (const [i, e] of arr.entries()) {
      console.log(`  [${qi}#${i}] ${e.timestamp} ${e.sender_id} ${e.content.slice(0, 30)}`)
    }
  }
  const ex = r.data?.categories?.execution
  console.log(`[EXEC] running=${ex?.summary?.running}`)
  if (mode === 'clean') {
    for (const [qi, arr] of [['steer', wq?.summary?.steer ?? []], ['follow_up', wq?.summary?.follow_up ?? []]]) {
      for (const [i, e] of arr.entries()) {
        const del = await call('session', 'queue_remove', {
          session_id: sid, queue: qi, index: i, sender_id: e.sender_id, timestamp: e.timestamp,
        })
        console.log(`  [DEL ${qi}#${i}] ok=${del.ok} ${del.ok ? `剩余 follow_up=${del.data?.follow_up} steer=${del.data?.steer}` : del.error}`)
      }
    }
  }
  ws.close()
  process.exit(0)
})
ws.on('error', (e) => { console.error(`ws error: ${e.message}`); process.exit(1) })
