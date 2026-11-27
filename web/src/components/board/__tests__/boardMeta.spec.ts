import { describe, expect, it } from 'vitest'
import { isInternalOrigin } from '../boardMeta'

// BUG-20261126-134 回归锁：内置 origin_type 值域与后端生产写入面对齐。
// 后端写入点：plan.rs "planner"（Swarm 拆解）/ "autopilot"（autopilot 建单）/
// "project"（项目建单）；flow_engine 写模板声明值（外部 → 亮徽标是设计意图）。
describe('isInternalOrigin', () => {
  it('后端生产写入的三个内置值都不亮外部徽标', () => {
    expect(isInternalOrigin('autopilot')).toBe(true)
    expect(isInternalOrigin('planner')).toBe(true)
    expect(isInternalOrigin('project')).toBe(true)
  })

  it('预留值仍判定为内置', () => {
    for (const t of ['cron', 'channel', 'cli', 'board', 'manual']) {
      expect(isInternalOrigin(t)).toBe(true)
    }
  })

  it('flow 模板声明的外部 connector 类型亮徽标', () => {
    expect(isInternalOrigin('github_issue')).toBe(false)
    expect(isInternalOrigin('rss')).toBe(false)
    expect(isInternalOrigin('')).toBe(false)
  })
})
