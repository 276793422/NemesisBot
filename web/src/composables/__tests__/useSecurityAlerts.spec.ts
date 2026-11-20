import { describe, it, expect, vi, beforeEach } from 'vitest'

// useSecurityAlerts（#53 安全事件流订阅，W3 批一）：
// - `security_alert`：kind+session 同 id 聚合计数，不同 id 并存（新条插队首）；
//   超 6 条裁最旧；tool 前缀进消息；未知 level 归 medium；
// - `security_alert_digest`：出口抑制摘要固定 digest:<outlet> 条目；
// - 手动 dismiss 只摘自己那条；非对象/缺 kind 载荷忽略。

const registered = new Map<string, (data?: unknown) => void>()

vi.mock('../useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    registered.set(type, handler)
  }),
  off: vi.fn(),
}))

import {
  useSecurityAlerts,
  securityAlerts,
  _resetSecurityAlertsForTest,
} from '../useSecurityAlerts'

function fire(data: unknown) {
  registered.get('security_alert')!(data)
}
function fireDigest(data: unknown) {
  registered.get('security_alert_digest')!(data)
}

beforeEach(() => {
  registered.clear()
  _resetSecurityAlertsForTest()
})

describe('useSecurityAlerts', () => {
  it('initSecurityAlerts 幂等：只挂一次订阅', () => {
    const { initSecurityAlerts } = useSecurityAlerts()
    initSecurityAlerts()
    initSecurityAlerts()
    expect(registered.size).toBe(2)
  })

  it('security_alert 同 kind+session 聚合计数；不同 session 并存', () => {
    const { initSecurityAlerts } = useSecurityAlerts()
    initSecurityAlerts()

    fire({ kind: 'pipeline_intercept', level: 'critical', session_key: '', tool: 'exec', summary: 'blocked rm -rf' })
    fire({ kind: 'pipeline_intercept', level: 'critical', session_key: '', tool: 'exec', summary: 'blocked again' })
    fire({ kind: 'pipeline_intercept', level: 'high', session_key: 's1', tool: '', summary: 'other session' })
    expect(securityAlerts.value).toHaveLength(2)

    // 不同 id 新条插队首（最新在前）；聚合条保持在后。
    expect(securityAlerts.value[0].id).toBe('pipeline_intercept:s1')
    expect(securityAlerts.value[0].count).toBe(1)
    const top = securityAlerts.value[1]
    expect(top.id).toBe('pipeline_intercept:')
    expect(top.count).toBe(2)
    expect(top.message).toBe('[exec] blocked again')
  })

  it('超 6 条裁最旧（最新插队首）', () => {
    const { initSecurityAlerts } = useSecurityAlerts()
    initSecurityAlerts()
    for (let i = 0; i < 8; i++) {
      fire({ kind: 'approval_denied', level: 'high', session_key: `s${i}`, tool: '', summary: `m${i}` })
    }
    expect(securityAlerts.value).toHaveLength(6)
    expect(securityAlerts.value[0].id).toBe('approval_denied:s7')
    expect(securityAlerts.value[5].id).toBe('approval_denied:s2')
  })

  it('digest 固定条目；suppressed<=0 忽略', () => {
    const { initSecurityAlerts } = useSecurityAlerts()
    initSecurityAlerts()
    fireDigest({ outlet: 'webhook', suppressed: 0, last_kind: 'x', last_summary: '' })
    expect(securityAlerts.value).toHaveLength(0)
    fireDigest({ outlet: 'webhook', suppressed: 4, last_summary: 'last one' })
    expect(securityAlerts.value).toHaveLength(1)
    expect(securityAlerts.value[0].id).toBe('digest:webhook')
    expect(securityAlerts.value[0].message).toContain('抑制 4 条')
    fireDigest({ outlet: 'webhook', suppressed: 2, last_summary: 'more' })
    expect(securityAlerts.value).toHaveLength(1)
    expect(securityAlerts.value[0].count).toBe(6)
  })

  it('未知 level 归 medium；缺 tool 无前缀', () => {
    const { initSecurityAlerts } = useSecurityAlerts()
    initSecurityAlerts()
    fire({ kind: 'estop_released', level: ' banana', session_key: 's', tool: '', summary: 'ok' })
    expect(securityAlerts.value[0].level).toBe('medium')
    expect(securityAlerts.value[0].message).toBe('ok')
  })

  it('dismiss 只摘指定 id；缺 kind 载荷忽略', () => {
    const { initSecurityAlerts, dismiss } = useSecurityAlerts()
    initSecurityAlerts()
    fire({ kind: 'estop_triggered', level: 'critical', session_key: '', tool: 'estop', summary: 'x' })
    fireDigest({ outlet: 'webhook', suppressed: 1, last_summary: 'y' })
    expect(securityAlerts.value).toHaveLength(2)
    dismiss('estop_triggered:')
    expect(securityAlerts.value.map(a => a.id)).toEqual(['digest:webhook'])

    fire(null)
    fire({ level: 'high', summary: 'no kind' })
    expect(securityAlerts.value).toHaveLength(1)
  })
})
