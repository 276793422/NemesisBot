/**
 * e2e/global-teardown.ts — 杀掉 global-setup 拉起的进程 + 清理隔离 home。
 *
 * - 从 e2e-state.json 读 PID，taskkill /F /T（Windows 进程树强杀）
 * - E2E_SKIP_LAUNCH=1 时不动任何进程（复用的实例归外部管理）
 * - E2E_KEEP_HOME=1 保留 home（失败现场排查）；否则删除
 *   （Windows AV/索引器瞬时句柄是常态：rmSync 带重试）
 */
import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { E2E, NEMESISBOT_DIR } from './env'

interface E2eState {
  gatewayPid: number | null
  mockPid: number | null
  supervisorPid?: number | null
}

function killTree(pid: number): void {
  if (process.platform === 'win32') {
    spawnSync('taskkill', ['/F', '/T', '/PID', String(pid)], { stdio: 'ignore' })
  } else {
    spawnSync('kill', ['-9', String(pid)], { stdio: 'ignore' })
  }
}

function rmWithRetry(target: string, attempts = 5): void {
  for (let i = 0; i < attempts; i++) {
    try {
      fs.rmSync(target, { recursive: true, force: true })
      return
    } catch (e) {
      if (i === attempts - 1) {
        console.warn(`[e2e-global-teardown] home 删除失败（保留现场）: ${e}`)
        return
      }
      spawnSync(process.execPath, ['-e', 'setTimeout(()=>{},300)'], { stdio: 'ignore' })
    }
  }
}

export default function globalTeardown(): void {
  const stateFile = path.join(NEMESISBOT_DIR, 'e2e-state.json')
  if (E2E.skipLaunch) {
    console.log('[e2e-global-teardown] SKIP_LAUNCH 模式：不杀进程、不清 home')
    return
  }
  let state: E2eState | null = null
  try {
    state = JSON.parse(fs.readFileSync(stateFile, 'utf-8'))
  } catch {
    console.warn('[e2e-global-teardown] e2e-state.json 缺失（setup 未跑完或已被清理），跳过杀进程')
  }
  if (state?.gatewayPid) killTree(state.gatewayPid)
  if (state?.mockPid) killTree(state.mockPid)
  // supervisor 最后杀（杀早了 restart 竞态无害但没必要）；gateway PID 由
  // supervisor 维护在 state 里，上一步已按最新值收掉。
  if (state?.supervisorPid) killTree(state.supervisorPid)

  if (E2E.keepHome) {
    console.log(`[e2e-global-teardown] E2E_KEEP_HOME=1，保留 ${E2E.home}`)
  } else {
    rmWithRetry(E2E.home)
  }
}
