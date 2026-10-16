/**
 * e2e/env.ts — e2e 环境参数单一来源。
 *
 * 全部参数可被 E2E_* 环境变量覆盖（消灭历史脚本里的 C:/AI/... 绝对路径硬编码），
 * global-setup 与各 spec 各自调用本模块——两端默认值一致即无需跨进程传状态。
 */
import path from 'node:path'
import fs from 'node:fs'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..')

function firstExistingBin(): string {
  const candidates = [
    process.env.E2E_BIN,
    path.join(repoRoot, 'target', 'release', 'nemesisbot.exe'),
    path.join(repoRoot, 'target', 'debug', 'nemesisbot.exe'),
  ].filter((p): p is string => !!p)
  for (const p of candidates) {
    if (fs.existsSync(p)) return p
  }
  return candidates[candidates.length - 1] ?? 'nemesisbot.exe'
}

export const E2E = {
  repoRoot,
  /**
   * Dashboard（nemesis-web web server）端口——页面与 /ws WSAPI 都在这里。
   * ⚠️ 不用 49001/49000：本机 ghost socket 集（kernel LISTEN 无主，web server
   * 探到 busy 会静默 bump +1，测试端口随之漂移）；37300 是 test-harness 在用。
   */
  webPort: Number(process.env.E2E_WEB_PORT ?? '37311'),
  /** gateway health server 端口（config gateway.port；18791-96 属 e2e-tests，避开）。 */
  healthPort: Number(process.env.E2E_HEALTH_PORT ?? '18797'),
  /** mock LLM 端口。 */
  mockPort: Number(process.env.E2E_MOCK_PORT ?? '18901'),
  /** gateway supervisor 端口（gateway spawn 权收归 supervisor——worker 进程
   *  带 job object 清理语义，spec 内 spawn 的 gateway 会在 worker 回收时被
   *  连带杀掉；supervisor 由 global-setup 主 runner 血统拉起，实证活过全 suite）。 */
  supervisorPort: Number(process.env.E2E_SUPERVISOR_PORT ?? '18902'),
  /** channels.web.auth_token。 */
  token: process.env.E2E_TOKEN ?? 'e2e-local-token',
  /** 隔离 gateway home 根目录（--local 模式，实际数据在 <home>/.nemesisbot）。 */
  home: process.env.E2E_HOME ?? path.join(repoRoot, 'target', 'e2e-home'),
  /** nemesisbot 可执行文件路径。 */
  bin: firstExistingBin(),
  /** mock LLM 脚本路径。 */
  mockScript: process.env.E2E_MOCK_SCRIPT ?? path.join(repoRoot, 'scripts', 'verify-mock-llm.mjs'),
  /** 1 = 不拉起环境，直接连已在跑的实例（本地调试迭代用）。 */
  skipLaunch: process.env.E2E_SKIP_LAUNCH === '1',
  /** 1 = teardown 保留 home 目录（失败现场排查用）。 */
  keepHome: process.env.E2E_KEEP_HOME === '1',
}

export const NEMESISBOT_DIR = path.join(E2E.home, '.nemesisbot')
export const WORKSPACE_DIR = path.join(NEMESISBOT_DIR, 'workspace')
export const CONFIG_PATH = path.join(NEMESISBOT_DIR, 'config.json')
export const SECURITY_CONFIG_PATH = path.join(WORKSPACE_DIR, 'config', 'config.security.json')
export const DRAFTS_DIR = path.join(WORKSPACE_DIR, 'workflow', 'drafts')
export const EVIDENCE_LOG = path.join(NEMESISBOT_DIR, 'mock-evidence.log')
export const EVIDENCE_LAST_REQUEST = path.join(NEMESISBOT_DIR, 'mock-evidence.last-request.json')

export const BASE_URL = `http://127.0.0.1:${E2E.webPort}`
export const WS_URL = `ws://127.0.0.1:${E2E.webPort}/ws?token=${encodeURIComponent(E2E.token)}`
export const HEALTH_URL = `http://127.0.0.1:${E2E.healthPort}/health`
export const MOCK_BASE = `http://127.0.0.1:${E2E.mockPort}`
/** gateway supervisor（gateway 拥有者进程）的 HTTP 面。 */
export const SUPERVISOR_BASE = `http://127.0.0.1:${E2E.supervisorPort}`

export const MODEL_A = 'mock-e2e-a'
export const MODEL_B = 'mock-e2e-b'
