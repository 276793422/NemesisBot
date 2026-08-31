import { onUnmounted } from 'vue'
import { on, off } from './useSSE'

/**
 * usage 数据变化推送订阅（A3 请求明细，2026-08-31）。
 *
 * 后端 gateway 轮询 usage.db 的 `PRAGMA data_version`，检测到任何写入方
 * （agent loop 记账 / workflow 节点 / retention sweep / CLI 跨进程）的新提交
 * 后向 SSE EventHub 发 `usage-changed`；本 composable 把该事件
 * **200ms 防抖**后转成回调——一次 LLM 轮次可能连续写多条明细，只刷一次。
 *
 * 用法：`useUsageChanged(() => loadLogs(true))`（silent：不闪 loading，
 * 数据静默换新）。组件卸载自动注销。
 */
export function useUsageChanged(handler: () => void, debounceMs = 200) {
  let timer: ReturnType<typeof setTimeout> | null = null

  const wrapped = () => {
    if (timer !== null) {
      clearTimeout(timer)
    }
    // 尾沿触发：事件流末尾后等 debounceMs 再刷，吞掉高频连发。
    timer = setTimeout(() => {
      timer = null
      handler()
    }, debounceMs)
  }

  on('usage-changed', wrapped)
  onUnmounted(() => {
    off('usage-changed', wrapped)
    if (timer !== null) {
      clearTimeout(timer)
      timer = null
    }
  })
}
