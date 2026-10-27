import { ref } from 'vue'
import { on } from './useSSE'

/**
 * 系统级降级告警（M2，2026-10-27 整改批次一）。
 *
 * 模块级单例（同 useApprovals）：AppLayout 顶部告警条消费。两类来源
 * （后端 SSE `system-alert`，gateway services.rs 装配）：
 * - `disk_watermark`：workspace 所在卷剩余空间升档（warn/critical）；
 *   降档回 ok = 自动清除（空间腾出来了，不再打扰）；
 * - `degraded_write`：关键写失败聚合（chat_log / checkpoint_tree /
 *   turn_wal）——写失败不致命但用户必须知道（回退网/历史落盘正在
 *   失效）；用户手动关闭，后端 10min 去抖窗口内不重复推。
 */

export interface SystemAlert {
  id: string
  kind: 'disk_watermark' | 'degraded_write'
  level: 'warn' | 'critical'
  message: string
  ts: number
}

export const systemAlerts = ref<SystemAlert[]>([])

let initialized = false

function upsert(alert: SystemAlert) {
  const i = systemAlerts.value.findIndex(a => a.id === alert.id)
  if (i >= 0) systemAlerts.value[i] = alert
  else systemAlerts.value.push(alert)
}

function dismissById(id: string) {
  systemAlerts.value = systemAlerts.value.filter(a => a.id !== id)
}

function applyFromPayload(data: any) {
  if (!data || typeof data !== 'object') return
  if (data.kind === 'disk_watermark') {
    if (data.level === 'ok' || data.level === undefined) {
      dismissById('disk_watermark')
      return
    }
    upsert({
      id: 'disk_watermark',
      kind: 'disk_watermark',
      level: data.level === 'critical' ? 'critical' : 'warn',
      message: String(data.message ?? `磁盘剩余空间不足（${data.free_gb} GB）`),
      ts: Date.now(),
    })
  } else if (data.kind === 'degraded_write') {
    const d = data.degraded ?? {}
    if (!d.kind) return
    upsert({
      id: `degraded_write:${d.kind}`,
      kind: 'degraded_write',
      level: 'warn',
      message: `${d.kind} 写入失败 ×${d.count}: ${d.detail}`,
      ts: Date.now(),
    })
  }
}

export function useSystemAlerts() {
  /** 幂等初始化：AppLayout 挂载时调用一次。 */
  function initSystemAlerts() {
    if (initialized) return
    initialized = true
    on('system-alert', (data: any) => applyFromPayload(data))
  }

  function dismiss(id: string) {
    dismissById(id)
  }

  return { systemAlerts, initSystemAlerts, dismiss }
}

/** 测试辅助：重置模块级单例（生产代码勿用）。 */
export function _resetSystemAlertsForTest() {
  systemAlerts.value = []
  initialized = false
}
