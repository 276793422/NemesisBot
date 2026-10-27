import { describe, it, expect, vi, beforeEach } from 'vitest'

// useSystemAlerts（M2 系统级降级告警订阅）：
// - `system-alert` 事件两分支：disk_watermark（升档 upsert / 回 ok 清除）
//   与 degraded_write（按失败源 kind 分条 upsert）；
// - 同 id 重复事件覆盖（水位降级/升级、计数增长），不同 id 并存；
// - 手动 dismiss 只摘自己那条。

const registered = new Map<string, (data?: unknown) => void>()

vi.mock('../useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    registered.set(type, handler)
  }),
  off: vi.fn(),
}))

import {
  useSystemAlerts,
  systemAlerts,
  _resetSystemAlertsForTest,
} from '../useSystemAlerts'

function fire(data: unknown) {
  registered.get('system-alert')!(data)
}

beforeEach(() => {
  registered.clear()
  _resetSystemAlertsForTest()
})

describe('useSystemAlerts', () => {
  it('initSystemAlerts 幂等：只挂一次订阅', () => {
    const { initSystemAlerts } = useSystemAlerts()
    initSystemAlerts()
    initSystemAlerts()
    expect(registered.size).toBe(1)
  })

  it('disk_watermark 升档 warn → critical 覆盖同条；回 ok 自动清除', () => {
    const { initSystemAlerts } = useSystemAlerts()
    initSystemAlerts()

    fire({ kind: 'disk_watermark', level: 'warn', free_gb: 4, message: 'warn 现场消息' })
    expect(systemAlerts.value).toHaveLength(1)
    expect(systemAlerts.value[0].id).toBe('disk_watermark')
    expect(systemAlerts.value[0].level).toBe('warn')

    fire({ kind: 'disk_watermark', level: 'critical', free_gb: 0, message: 'critical 现场消息' })
    expect(systemAlerts.value).toHaveLength(1)
    expect(systemAlerts.value[0].level).toBe('critical')
    expect(systemAlerts.value[0].message).toBe('critical 现场消息')

    fire({ kind: 'disk_watermark', level: 'ok' })
    expect(systemAlerts.value).toHaveLength(0)
  })

  it('disk_watermark 缺 level 视作清除（防御未知载荷形态）', () => {
    const { initSystemAlerts } = useSystemAlerts()
    initSystemAlerts()
    fire({ kind: 'disk_watermark', level: 'warn', message: 'x' })
    expect(systemAlerts.value).toHaveLength(1)
    fire({ kind: 'disk_watermark' })
    expect(systemAlerts.value).toHaveLength(0)
  })

  it('degraded_write 按失败源分条；同源重复覆盖计数', () => {
    const { initSystemAlerts } = useSystemAlerts()
    initSystemAlerts()

    fire({ kind: 'degraded_write', degraded: { kind: 'chat_log', count: 3, detail: 'write failed: x (os error 28)' } })
    fire({ kind: 'degraded_write', degraded: { kind: 'turn_wal', count: 1, detail: 'WAL 写入失败' } })
    expect(systemAlerts.value).toHaveLength(2)

    fire({ kind: 'degraded_write', degraded: { kind: 'chat_log', count: 9, detail: 'write failed: y (os error 28)' } })
    expect(systemAlerts.value).toHaveLength(2)
    const chat = systemAlerts.value.find(a => a.id === 'degraded_write:chat_log')!
    expect(chat.message).toContain('×9')
    expect(chat.message).toContain('os error 28')
  })

  it('degraded_write 缺 degraded.kind 忽略（防御）', () => {
    const { initSystemAlerts } = useSystemAlerts()
    initSystemAlerts()
    fire({ kind: 'degraded_write', degraded: {} })
    fire({ kind: 'degraded_write' })
    expect(systemAlerts.value).toHaveLength(0)
  })

  it('dismiss 只摘指定 id', () => {
    const { initSystemAlerts, dismiss } = useSystemAlerts()
    initSystemAlerts()
    fire({ kind: 'disk_watermark', level: 'warn', message: 'x' })
    fire({ kind: 'degraded_write', degraded: { kind: 'turn_wal', count: 1, detail: 'd' } })
    expect(systemAlerts.value).toHaveLength(2)

    dismiss('disk_watermark')
    expect(systemAlerts.value.map(a => a.id)).toEqual(['degraded_write:turn_wal'])
  })

  it('非对象载荷忽略', () => {
    const { initSystemAlerts } = useSystemAlerts()
    initSystemAlerts()
    fire(null)
    fire('x')
    fire(42)
    expect(systemAlerts.value).toHaveLength(0)
  })
})
