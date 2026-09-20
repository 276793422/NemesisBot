import { ref } from 'vue'
import { on } from './useSSE'
import { useWSAPI } from './useWSAPI'
import { useToast } from './useToast'

/**
 * Full Access 编辑器放行开关状态（2026-09-20 用户裁决，仿 codex）。
 *
 * 模块级单例（同 useApprovals）：聊天框旁按钮 / 设置页【编辑器】TAB /
 * Sidebar 徽标全部读这里——单一真相源。三通道状态收敛：
 * - initEditorMode 订阅 SSE `editor-mode`（任一窗口翻转，所有窗口跟随）；
 * - 挂载时 `editor.get` seed 对齐（后端是运行时态：进程重启一律双关，
 *   必须用户手动再开）；
 * - setEditorAccess 走 `editor.set`，以响应（服务端生效值）为准——服务端
 *   联动收口 ext=true ⇒ full=true，前端不自行推断。
 *
 * seed 失败静默（WS 未就绪可能只是暂时；开关保持安全缺省双关）；
 * 仅在收到明确「未装配」错误（security 未启用实例）时置
 * editorAvailable=false，按钮诚实禁用。
 */

export const fullAccess = ref(false)
export const externalWrite = ref(false)
export const editorAvailable = ref(true)

let initialized = false
/** seed 有限重试计数（WS 未就绪的暂时性失败不放弃收敛；上限 3 次）。 */
let seedRetries = 0
const SEED_MAX_RETRIES = 3

function applyFromPayload(data: any) {
  if (!data) return
  if (typeof data.full_access === 'boolean') fullAccess.value = data.full_access
  if (typeof data.external_write === 'boolean') externalWrite.value = data.external_write
}

export function useEditorMode() {
  const toast = useToast()

  /** 幂等初始化：AppLayout 挂载时调用一次。 */
  function initEditorMode() {
    if (initialized) return
    initialized = true
    on('editor-mode', (data: any) => applyFromPayload(data))
    seedEditorMode()
  }

  async function seedEditorMode() {
    const { request } = useWSAPI()
    try {
      const res = await request('editor', 'get', {}, 5000)
      applyFromPayload(res)
    } catch (err: any) {
      if (String(err ?? '').includes('未装配')) {
        editorAvailable.value = false
        return
      }
      // 暂时性失败（WS 未就绪）静默退避重试——否则晚连/重连窗口会停在
      // 旧态，直到下一次任意窗口 set 才被 SSE 纠正（收敛缺口）。
      if (seedRetries < SEED_MAX_RETRIES) {
        seedRetries += 1
        setTimeout(seedEditorMode, 3000 * seedRetries)
      }
    }
  }

  /** 翻转开关（UI 只发意图；联动与生效值以服务端为准）。 */
  async function setEditorAccess(full: boolean, ext: boolean) {
    const { request } = useWSAPI()
    try {
      const res = await request('editor', 'set', {
        full_access: full,
        external_write: ext,
      })
      applyFromPayload(res)
    } catch (err: any) {
      const msg = String(err ?? '')
      if (msg.includes('未装配')) editorAvailable.value = false
      toast.error(`切换放行开关失败: ${msg}`)
    }
  }

  return { fullAccess, externalWrite, editorAvailable, initEditorMode, setEditorAccess }
}

/** 测试辅助：重置模块级单例（生产代码勿用）。 */
export function _resetEditorModeForTest() {
  fullAccess.value = false
  externalWrite.value = false
  editorAvailable.value = true
  initialized = false
  seedRetries = 0
}
