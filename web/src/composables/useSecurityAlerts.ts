import { ref } from 'vue'
import { on } from './useSSE'

/**
 * #53 安全事件流（W3 批一，2026-11-20）：安全告警事件卡（AppLayout 顶部
 * 告警条，与 system-alert 告警条并列独立）。
 *
 * 两类 SSE 来源（gateway services.rs 装配的 "sse" 出口发布）：
 * - `security_alert`：单条安全事件（管线拦截/审批拒绝/急停/循环治理/
 *   记忆隔离/MCP 装前扫描/链验证失败…）。同 kind+会话 在条内聚合计数
 *   （cooldown 闸在后端，前端只做展示去重），最多保留 6 条最新；
 * - `security_alert_digest`：出口预算超限聚合（服务端 60s 窗口抑制 N 条
 *   的摘要）——固定一条 digest 条目，不与单条事件混排。
 *
 * 声音/徽标/态势页（#60）后置，本批只有事件卡。
 */

export interface SecurityAlertItem {
  /** kind:session_key（digest 条目为 digest:<outlet>）。 */
  id: string
  kind: string
  level: 'critical' | 'high' | 'medium' | 'low'
  message: string
  /** 同 id 聚合的条数（digest 条目 = 抑制条数）。 */
  count: number
  ts: number
}

export const securityAlerts = ref<SecurityAlertItem[]>([])

const MAX_ITEMS = 6

let initialized = false

function upsert(item: SecurityAlertItem) {
  const i = securityAlerts.value.findIndex(a => a.id === item.id)
  if (i >= 0) {
    const prev = securityAlerts.value[i]
    securityAlerts.value[i] = { ...item, count: prev.count + item.count }
  } else {
    securityAlerts.value.unshift(item)
    if (securityAlerts.value.length > MAX_ITEMS) securityAlerts.value.pop()
  }
}

function normalizeLevel(v: any): SecurityAlertItem['level'] {
  return v === 'critical' || v === 'high' || v === 'medium' || v === 'low' ? v : 'medium'
}

function applyEvent(data: any) {
  if (!data || typeof data !== 'object' || typeof data.kind !== 'string' || !data.kind) return
  const session = typeof data.session_key === 'string' ? data.session_key : ''
  const tool = typeof data.tool === 'string' ? data.tool : ''
  const summary = typeof data.summary === 'string' ? data.summary : ''
  const message = tool ? `[${tool}] ${summary}` : summary
  upsert({
    id: `${data.kind}:${session}`,
    kind: data.kind,
    level: normalizeLevel(data.level),
    message,
    count: 1,
    ts: Date.now(),
  })
}

function applyDigest(data: any) {
  if (!data || typeof data !== 'object' || typeof data.outlet !== 'string') return
  const suppressed = typeof data.suppressed === 'number' ? data.suppressed : 0
  if (suppressed <= 0) return
  const last = typeof data.last_summary === 'string' ? data.last_summary : ''
  upsert({
    id: `digest:${data.outlet}`,
    kind: 'digest',
    level: 'medium',
    message: `出口 ${data.outlet} 过去一分钟抑制 ${suppressed} 条告警（最后一条：${last}）`,
    count: suppressed,
    ts: Date.now(),
  })
}

export function useSecurityAlerts() {
  /** 幂等初始化：AppLayout 挂载时调用一次。 */
  function initSecurityAlerts() {
    if (initialized) return
    initialized = true
    on('security_alert', (data: any) => applyEvent(data))
    on('security_alert_digest', (data: any) => applyDigest(data))
  }

  function dismiss(id: string) {
    securityAlerts.value = securityAlerts.value.filter(a => a.id !== id)
  }

  return { securityAlerts, initSecurityAlerts, dismiss }
}

/** 测试辅助：重置模块级单例（生产代码勿用）。 */
export function _resetSecurityAlertsForTest() {
  securityAlerts.value = []
  initialized = false
}
