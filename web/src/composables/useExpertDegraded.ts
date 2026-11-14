import { on } from './useSSE'
import { useToast } from './useToast'

/**
 * 专家自动降级通知（2026-11-13 专家总开关与自动降级）。
 *
 * 模块级单例（同 useApprovals/useSystemAlerts）：AppLayout 挂载时初始化，
 * 订阅 SSE `expert-degraded`（server.rs pump 全局广播，载荷为展平内层
 * `{session_key, chat_id, requested, reason}`）。服务端只在
 * `agents.experts.notify=true` 时发布（默认静默降级不发事件），前端
 * 收到即 toast——通知开关的「关」由后端闸，前端无重复判断。
 */
let initialized = false

export function useExpertDegraded() {
  /** 幂等初始化：AppLayout 挂载时调用一次。 */
  function initExpertDegraded() {
    if (initialized) return
    initialized = true
    const toast = useToast()
    on('expert-degraded', (data: any) => {
      if (!data || typeof data !== 'object') return
      const requested = typeof data.requested === 'string' ? data.requested.trim() : ''
      if (!requested) return
      toast.info(`专家「${requested}」不可用，已自动降级为通用子代理继续任务`)
    })
  }

  return { initExpertDegraded }
}

/** 测试辅助：重置模块级单例（生产代码勿用）。 */
export function _resetExpertDegradedForTest() {
  initialized = false
}
