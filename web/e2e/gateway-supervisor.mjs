/**
 * e2e/gateway-supervisor.mjs — gateway 拥有者进程（e2e 专用）。
 *
 * 为什么存在：Playwright worker 进程在 Windows 上带 job object 清理语义，
 * spec 内直接 spawn 的 gateway 会在 worker 回收（文件边界）时被连带杀掉
 * （实录：project-history 重启后 gateway 恰在下一 spec 开始时消失，
 * ERR_CONNECTION_REFUSED 三连）。global-setup（主 runner 血统）拉起的
 * 进程实证活过整个 suite——所以 gateway 的 spawn 权收归本 supervisor
 * （也由 global-setup 拉起），spec 内重启走 HTTP 请求本进程执行。
 *
 * API：
 *   GET  /health   → 200 {gatewayPid}
 *   POST /restart  → 杀旧 gateway → 等端口释放 → 起新 gateway → 等
 *                    gateway health OK → 更新 state 文件 → 200 {pid}
 *                    （失败 500 + gateway 日志尾，失败信息自带现场）
 *
 * gateway PID 每次变更都回写 e2e-state.json（teardown 按它收尾）。
 */
import { spawn, spawnSync } from 'node:child_process'
import fs from 'node:fs'
import http from 'node:http'

const args = process.argv.slice(2)
function argOf(name, fallback) {
  const i = args.indexOf(name)
  return i >= 0 && i + 1 < args.length ? args[i + 1] : fallback
}

const PORT = Number(argOf('--port', '18902'))
const BIN = argOf('--bin')
const HOME = argOf('--home')
const STATE_FILE = argOf('--state')
const LOG_FILE = argOf('--log')
const HEALTH_URL = argOf('--health-url')

if (!BIN || !HOME || !STATE_FILE || !HEALTH_URL) {
  console.error('[gateway-supervisor] 缺参数：--bin/--home/--state/--health-url 必填')
  process.exit(1)
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

let gatewayProc = null

function readState() {
  try {
    return JSON.parse(fs.readFileSync(STATE_FILE, 'utf-8'))
  } catch {
    return {}
  }
}

function writeState(patch) {
  const state = { ...readState(), ...patch }
  fs.writeFileSync(STATE_FILE, JSON.stringify(state, null, 2))
}

function killGateway() {
  if (!gatewayProc || !gatewayProc.pid || gatewayProc.exitCode !== null) return
  if (process.platform === 'win32') {
    spawnSync('taskkill', ['/F', '/T', '/PID', String(gatewayProc.pid)], { stdio: 'ignore' })
  } else {
    try {
      gatewayProc.kill('SIGKILL')
    } catch {
      /* ignore */
    }
  }
  gatewayProc = null
}

async function gatewayGone(deadlineMs) {
  const deadline = Date.now() + deadlineMs
  for (;;) {
    let alive = true
    try {
      const res = await fetch(HEALTH_URL)
      alive = res.ok
    } catch {
      alive = false
    }
    if (!alive) return true
    if (Date.now() > deadline) return false
    await sleep(300)
  }
}

async function gatewayReady(deadlineMs) {
  const deadline = Date.now() + deadlineMs
  for (;;) {
    try {
      const res = await fetch(HEALTH_URL)
      if (res.ok) return true
    } catch {
      /* 未就绪 */
    }
    if (Date.now() > deadline) return false
    await sleep(500)
  }
}

function spawnGateway() {
  fs.mkdirSync(HOME, { recursive: true })
  const out = fs.openSync(LOG_FILE, 'a')
  const child = spawn(BIN, ['gateway', '--local'], {
    cwd: HOME,
    stdio: ['ignore', out, out],
  })
  // 闭包捕获 child：旧实例的 exit 事件晚于新实例 spawn 到达时不得误清新句柄
  child.on('exit', (code) => {
    console.error(`[gateway-supervisor] gateway 进程退出（code=${code}）`)
    if (gatewayProc === child) gatewayProc = null
  })
  // spawn 本身失败（bin 消失/ENOENT）：无人监听 error 事件 = supervisor 整个崩掉
  child.on('error', (err) => {
    console.error(`[gateway-supervisor] gateway spawn 失败: ${err}`)
    if (gatewayProc === child) gatewayProc = null
  })
  gatewayProc = child
  writeState({ gatewayPid: child.pid ?? null })
  return child.pid
}

function logTail(lines = 40) {
  try {
    return fs.readFileSync(LOG_FILE, 'utf-8').split(/\r?\n/).slice(-lines).join('\n')
  } catch {
    return '<无日志>'
  }
}

async function restart() {
  killGateway()
  // 端口释放确认：web server 撞 busy 会静默 bump +1，端口漂移让后续断言全空
  if (!(await gatewayGone(15_000))) {
    throw new Error('旧 gateway 15s 未退出（端口未释放，web 端口可能漂移）')
  }
  const pid = spawnGateway()
  if (!(await gatewayReady(90_000))) {
    throw new Error(`gateway 重启后 90s 未就绪（pid=${pid}）\n--- gateway log tail ---\n${logTail()}`)
  }
  return pid
}

const server = http.createServer((req, res) => {
  const respond = (code, body) => {
    res.writeHead(code, { 'content-type': 'application/json' })
    res.end(JSON.stringify(body))
  }
  if (req.method === 'GET' && req.url === '/health') {
    respond(200, { ok: true, gatewayPid: gatewayProc?.pid ?? null })
    return
  }
  if (req.method === 'POST' && req.url === '/restart') {
    restart()
      .then((pid) => respond(200, { ok: true, pid }))
      .catch((e) => respond(500, { ok: false, error: String(e?.message ?? e) }))
    return
  }
  respond(404, { ok: false, error: 'not found' })
})

// 启动即拉起首个 gateway（global-setup 不再直接持有 gateway）
spawnGateway()
server.listen(PORT, '127.0.0.1', () => {
  console.log(`[gateway-supervisor] listening on 127.0.0.1:${PORT} (gateway pid=${gatewayProc?.pid ?? '?'})`)
})

process.on('SIGTERM', () => {
  killGateway()
  server.close()
  process.exit(0)
})
process.on('SIGINT', () => {
  killGateway()
  process.exit(0)
})
