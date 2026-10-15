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
    ],
    {
      cwd: E2E.repoRoot,
      stdio: ['ignore', fs.openSync(path.join(NEMESISBOT_DIR, 'supervisor-e2e.log'), 'a'), fs.openSync(path.join(NEMESISBOT_DIR, 'supervisor-e2e.log'), 'a')],
    },
  )

  await waitHttp(`http://127.0.0.1:${E2E.mockPort}/v1/models`, 20_000, 'mock LLM')
  await waitHttp(`${SUPERVISOR_BASE}/health`, 20_000, 'gateway supervisor')
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
