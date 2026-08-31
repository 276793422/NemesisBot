import { onUnmounted } from 'vue'
import { on, off } from './useSSE'

/**
 * board 数据变化推送订阅（W2.5）。
 *
 * 后端 gateway 轮询 board.db 的 `PRAGMA data_version`，检测到任何写入方
 * （WSAPI handler / 集群回调 / autopilot cron / 派发 sweep / CLI 跨进程）
 * 的新提交后向 SSE EventHub 发 `board-changed`；本 composable 把该事件
 * **200ms 防抖**后转成回调——批量写入（一次 WSAPI 常触发多张表）只刷一次。
 *
 * 用法：`useBoardChanged(() => load(true))`（silent=true：不闪 loading，
 * 数据静默换新）。组件卸载自动注销。
 */
export function useBoardChanged(handler: () => void, debounceMs = 200) {
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

  on('board-changed', wrapped)
  onUnmounted(() => {
    off('board-changed', wrapped)
    if (timer !== null) {
      clearTimeout(timer)
      timer = null
    }
  })
}
