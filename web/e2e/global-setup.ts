/**
 * e2e/global-setup.ts — e2e 环境编排（取代历史脚本的"外部前置"）：
 *
 *   1. 清空并重建隔离 home（E2E_HOME，--local 模式）
 *   2. nemesisbot onboard default --local 提取模板工作区
 *   3. 改写 config.json：web/health 端口、auth token、双模型条目指向 mock LLM
 *   4. 改写 config.security.json：追加审批 e2e 的 ask 规则（*E2E_APPROVAL_MARKER*）
 *   5. 拉起 mock LLM（scripts/verify-mock-llm.mjs）+ gateway supervisor
 *      （gateway spawn 权归 supervisor——本进程/spec worker 的 job object
 *      清理语义会连带杀子进程；spec 内重启经 supervisor HTTP 执行）
 *
 * 任何一步失败：先杀已拉起进程、打印日志尾部（失败信息自带现场），exit 1。
 * E2E_SKIP_LAUNCH=1 跳过 1-5，只探活既有实例（本地调试迭代用）。
 */
import { spawn, spawnSync, type ChildProcess } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import {
  BASE_URL,
  CONFIG_PATH,
  DRAFTS_DIR,
  E2E,
  EVIDENCE_LOG,
  HEALTH_URL,
  MODEL_A,
  MODEL_B,
  NEMESISBOT_DIR,
  SECURITY_CONFIG_PATH,
  SUPERVISOR_BASE,
  WORKSPACE_DIR,
} from './env'

let mockProc: ChildProcess | null = null
let supervisorProc: ChildProcess | null = null

function logFileTail(file: string, lines = 30): string {
  try {
    const content = fs.readFileSync(file, 'utf-8')
    const all = content.split(/\r?\n/)
    return all.slice(-lines).join('\n')
  } catch (e) {
    return `<无法读取 ${file}: ${e}>`
  }
}

function killProc(proc: ChildProcess | null): void {
  if (!proc || !proc.pid || proc.exitCode !== null) return
  if (process.platform === 'win32') {
    try {
      spawnSync('taskkill', ['/F', '/T', '/PID', String(proc.pid)], { stdio: 'ignore' })
    } catch {
      /* 尽力而为 */
    }
  } else {
    try {
      proc.kill('SIGKILL')
    } catch {
      /* ignore */
    }
  }
}

function killGatewayFromState(): void {
  // gateway 归 supervisor 所有：直接杀 supervisor 会留孤儿 gateway，
  // 按它回写 state 文件的最新 PID 收尾。
  try {
    const state = JSON.parse(fs.readFileSync(path.join(NEMESISBOT_DIR, 'e2e-state.json'), 'utf-8'))
    const pid = state?.gatewayPid
    if (pid) {
      if (process.platform === 'win32') {
        spawnSync('taskkill', ['/F', '/T', '/PID', String(pid)], { stdio: 'ignore' })
      } else {
        spawnSync('kill', ['-9', String(pid)], { stdio: 'ignore' })
      }
    }
  } catch {
    /* state 缺失 = 无 gateway 可收 */
  }
}

function fail(message: string, error?: unknown): never {
  killProc(mockProc)
  killProc(supervisorProc)
  killGatewayFromState()
  console.error(`\n[e2e-global-setup] FAIL: ${message}`)
  if (error) console.error(error)
  console.error(`--- gateway log tail ---\n${logFileTail(path.join(NEMESISBOT_DIR, 'gateway-e2e.log'))}`)
  console.error(`--- mock log tail ---\n${logFileTail(path.join(NEMESISBOT_DIR, 'mock-e2e.log'))}`)
  process.exit(1)
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms))
}

async function waitHttp(url: string, timeoutMs: number, what: string): Promise<void> {
  const deadline = Date.now() + timeoutMs
  let lastErr = ''
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url)
      if (res.ok) return
      lastErr = `HTTP ${res.status}`
    } catch (e) {
      lastErr = String(e)
    }
    await sleep(500)
  }
  fail(`${what} 未在 ${timeoutMs}ms 内就绪（${url}）：${lastErr}`)
}

/**
 * 等 supervisor 把 gateway 真正拉起（/health body 的 gatewayPid 变为正数）。
 *
 * 不能用 waitHttp 探 supervisor /health：supervisor 的 HTTP listener 在模块
 * 加载时同步先起（gateway-supervisor.mjs 刻意设计——等端口 bind 期间探活
 * 不被拖爆），而 spawnGateway（内含 e2e-state.json 落盘）在 async IIFE 里
 * 要先过 portsBindable 的 TIME_WAIT 等待窗口（零间隙链式 invocation 时最长
 * 60s）——/health 恒 200（gatewayPid=null 也 200），waitHttp 通过 ≠ state
 * 文件已写。此处以 gatewayPid 非 null 为放行条件（gatewayProc 赋值与
 * writeState 相邻同步，严格等价于 state 文件已含 gatewayPid）。
 */
async function waitSupervisorGatewayReady(timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs
  let last = ''
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`${SUPERVISOR_BASE}/health`)
      if (res.ok) {
        const body = (await res.json()) as { gatewayPid?: number | null }
        if (typeof body.gatewayPid === 'number' && body.gatewayPid > 0) return
        last = `health OK 但 gatewayPid=${body.gatewayPid}（gateway 尚未 spawn：等 web/health 端口可 bind）`
      } else {
        last = `HTTP ${res.status}`
      }
    } catch (e) {
      last = String(e)
    }
    await sleep(500)
  }
  fail(
    `gateway supervisor ${timeoutMs}ms 内未拉起 gateway：${last}\n--- supervisor log tail ---\n${logFileTail(path.join(NEMESISBOT_DIR, 'supervisor-e2e.log'))}`,
  )
}

function runOnboard(): void {
  const res = spawnSync(E2E.bin, ['onboard', 'default', '--local'], {
    cwd: E2E.home,
    encoding: 'utf-8',
    timeout: 120_000,
  })
  if (res.error) fail(`onboard 进程启动失败（bin=${E2E.bin}）`, res.error)
  if (res.status !== 0) {
    fail(`onboard 退出码 ${res.status}\nstdout:\n${res.stdout}\nstderr:\n${res.stderr}`)
  }
  if (!fs.existsSync(CONFIG_PATH)) {
    fail(`onboard 完成但主配置缺失：${CONFIG_PATH}`)
  }
}

function patchMainConfig(): void {
  let config: Record<string, unknown>
  try {
    config = JSON.parse(fs.readFileSync(CONFIG_PATH, 'utf-8'))
  } catch (e) {
    fail(`主配置解析失败：${CONFIG_PATH}`, e)
  }
  const gateway = (config.gateway ?? {}) as Record<string, unknown>
  gateway.port = E2E.healthPort
  gateway.host = '127.0.0.1'
  config.gateway = gateway

  const channels = (config.channels ?? {}) as Record<string, unknown>
  const web = (channels.web ?? {}) as Record<string, unknown>
  web.enabled = true
  web.host = '127.0.0.1'
  web.port = E2E.webPort
  web.auth_token = E2E.token
  channels.web = web
  // 独立 websocket 通道显式关闭：onboard 产物 enabled:true + sync_to:["web"]，
  // 与 web 通道同抢 49001（非 upgrade 的 /health 探活会被 WebSocketChannel
  // 拒掉——升级失败风暴实录）。e2e 只走 web 通道（dashboard HTTP + /ws WSAPI）。
  const wsChan = (channels.websocket ?? {}) as Record<string, unknown>
  wsChan.enabled = false
  channels.websocket = wsChan
  config.channels = channels

  const mockBase = `http://127.0.0.1:${E2E.mockPort}/v1`
  const modelEntry = (name: string) => ({
    model_name: name,
    model: name,
    api_base: mockBase,
    api_key: 'e2e-mock-key',
    auth_method: '',
    connect_mode: '',
    protocol: 'openai',
    model_tier: 'big',
    proxy: '',
    reasoning_effort: '',
    workspace: '',
    stream: 'yes',
  })
  config.model_list = [modelEntry(MODEL_A), modelEntry(MODEL_B)]

  const agents = (config.agents ?? {}) as Record<string, unknown>
  const defaults = (agents.defaults ?? {}) as Record<string, unknown>
  defaults.llm = MODEL_A
  agents.defaults = defaults
  config.agents = agents

  // W2 批三（#47 记忆投毒隔离）：开启强化记忆让 gateway 装配 MemoryManager
  //（无 ONNX 插件自动降级 basic，闲置无副作用）——记忆内容闸 + 隔离区
  // spec（memory-scan-visual）依赖 manager 在场。
  const memory = (config.memory ?? {}) as Record<string, unknown>
  memory.enabled = true
  config.memory = memory

  // W2 批三（#14 MCP 装前扫描）：显式开启扫描档（缺省 Off = 现形态零突变，
  // memory-scan-visual 的 RequireConfirm 链路依赖 On）。合并写保留其余
  // security 键不动。
  const security = (config.security ?? {}) as Record<string, unknown>
  security.preinstall_scan = { enabled: true }
  config.security = security

  try {
    fs.writeFileSync(CONFIG_PATH, JSON.stringify(config, null, 2))
  } catch (e) {
    fail(`主配置写回失败：${CONFIG_PATH}`, e)
  }
}

function patchSecurityConfig(): void {
  let sec: Record<string, unknown>
  try {
    sec = JSON.parse(fs.readFileSync(SECURITY_CONFIG_PATH, 'utf-8'))
  } catch (e) {
    fail(`安全配置解析失败：${SECURITY_CONFIG_PATH}`, e)
  }
  const procRules = (sec.process_rules ?? {}) as Record<string, unknown>
  const exec = Array.isArray(procRules.exec) ? (procRules.exec as unknown[]) : []
  // glob 形态必须两侧带 *：exec ask 规则走 match_command_pattern = 整串锚定
  // （^pattern$，* → .*），裸子串 pattern 对 `echo ..._ok` 永不命中（审批卡
  // 永不出现实录②——①是 cmd /c 先撞 Layer 2 命令守卫）。
  exec.push({ pattern: '*E2E_APPROVAL_MARKER*', action: 'ask' })
  procRules.exec = exec
  sec.process_rules = procRules
  try {
    fs.writeFileSync(SECURITY_CONFIG_PATH, JSON.stringify(sec, null, 2))
  } catch (e) {
    fail(`安全配置写回失败：${SECURITY_CONFIG_PATH}`, e)
  }
}

async function main(): Promise<void> {
  if (E2E.skipLaunch) {
    await waitHttp(HEALTH_URL, 10_000, '既有 gateway（E2E_SKIP_LAUNCH）')
    console.log(`[e2e-global-setup] SKIP_LAUNCH：复用运行中实例 ${BASE_URL}`)
    return
  }

  if (!fs.existsSync(E2E.bin)) {
    fail(`nemesisbot 可执行文件不存在：${E2E.bin}（先 cargo build -p nemesisbot，或设 E2E_BIN）`)
  }
  if (!fs.existsSync(E2E.mockScript)) {
    fail(`mock LLM 脚本不存在：${E2E.mockScript}`)
  }

  // 1. 重建隔离 home
  fs.rmSync(E2E.home, { recursive: true, force: true })
  fs.mkdirSync(E2E.home, { recursive: true })

  // 2. onboard 提取模板
  runOnboard()

  // 3/4. 配置改写
  patchMainConfig()
  patchSecurityConfig()

  // 草稿目录清零（模板不含；显式清一次防历史残留污染 workflow specs）
  fs.rmSync(DRAFTS_DIR, { recursive: true, force: true })
  fs.mkdirSync(DRAFTS_DIR, { recursive: true })
  fs.mkdirSync(WORKSPACE_DIR, { recursive: true })

  // 5. mock LLM + gateway supervisor
  mockProc = spawn(process.execPath, [E2E.mockScript, '--port', String(E2E.mockPort), '--evidence', EVIDENCE_LOG], {
    cwd: E2E.repoRoot,
    stdio: ['ignore', fs.openSync(path.join(NEMESISBOT_DIR, 'mock-e2e.log'), 'a'), fs.openSync(path.join(NEMESISBOT_DIR, 'mock-e2e.log'), 'a')],
  })
  mockProc.on('exit', (code) => {
    if (code !== null && code !== 0 && supervisorProc === null) {
      fail(`mock LLM 提前退出（code=${code}）`)
    }
  })

  // gateway 归 supervisor 拉起（spawn 权不落本进程/spec worker——worker 的
  // job object 清理语义会在回收时连带杀掉 spec 内 spawn 的子进程）；
  // supervisor 顺带持有 PID 真相源（e2e-state.json 每次重启回写）。
  const supervisorScript = path.join(E2E.repoRoot, 'web', 'e2e', 'gateway-supervisor.mjs')
  supervisorProc = spawn(
    process.execPath,
    [
      supervisorScript,
      '--port', String(E2E.supervisorPort),
      '--bin', E2E.bin,
      '--home', E2E.home,
      '--state', path.join(NEMESISBOT_DIR, 'e2e-state.json'),
      '--log', path.join(NEMESISBOT_DIR, 'gateway-e2e.log'),
      '--health-url', HEALTH_URL,
      '--web-port', String(E2E.webPort), // TIME_WAIT 容忍探测（supervisor spawn 前等端口可 bind）
    ],
    {
      cwd: E2E.repoRoot,
      stdio: ['ignore', fs.openSync(path.join(NEMESISBOT_DIR, 'supervisor-e2e.log'), 'a'), fs.openSync(path.join(NEMESISBOT_DIR, 'supervisor-e2e.log'), 'a')],
    },
  )

  await waitHttp(`http://127.0.0.1:${E2E.mockPort}/v1/models`, 20_000, 'mock LLM')
  // BUG-20261127-007：不能只探 supervisor /health 的 HTTP 200（listener 先起、
  // gatewayPid 可能还是 null）——提前返回会在下一步读 e2e-state.json 时
  // ENOENT 崩 setup 且不经过 fail()（mock/supervisor 成孤儿占住全部端口）。
  await waitSupervisorGatewayReady(90_000)
  await waitHttp(HEALTH_URL, 90_000, 'gateway')

  // supervisor/mock PID 落盘供 teardown 使用（gatewayPid 由 supervisor 维护）
  fs.writeFileSync(
    path.join(NEMESISBOT_DIR, 'e2e-state.json'),
    JSON.stringify({
      ...JSON.parse(fs.readFileSync(path.join(NEMESISBOT_DIR, 'e2e-state.json'), 'utf-8')),
      supervisorPid: supervisorProc?.pid ?? null,
      mockPid: mockProc?.pid ?? null,
      webPort: E2E.webPort,
      healthPort: E2E.healthPort,
      mockPort: E2E.mockPort,
      supervisorPort: E2E.supervisorPort,
      startedAt: new Date().toISOString(),
    }, null, 2),
  )
  console.log(`[e2e-global-setup] 就绪：${BASE_URL}（home=${E2E.home}）`)
}

export default main
