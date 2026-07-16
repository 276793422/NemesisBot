import { describe, it, expect } from 'vitest'
import {
  CLASSIC_NAV_GROUPS,
  FRIENDLY_NAV_IDS,
  FRIENDLY_MORE_IDS,
  filterNavGroups,
  flattenNavItems,
  itemsByIds,
  MERGED_ROUTE_REDIRECTS,
} from './navConfig'

describe('navConfig flat friendly + hubs', () => {
  it('friendly nav is flat (no fold) and includes former more items', () => {
    expect(FRIENDLY_MORE_IDS.length).toBe(0)
    expect(FRIENDLY_NAV_IDS).toContain('chat')
    expect(FRIENDLY_NAV_IDS).toContain('skills')
    expect(FRIENDLY_NAV_IDS).toContain('security')
    expect(FRIENDLY_NAV_IDS).toContain('cluster')
    expect(FRIENDLY_NAV_IDS).toContain('about')
    expect(itemsByIds(FRIENDLY_NAV_IDS).length).toBe(FRIENDLY_NAV_IDS.length)
  })

  it('sidebar merges MCP/通道/工作流 into 能力, Forge into 高级', () => {
    const items = flattenNavItems(CLASSIC_NAV_GROUPS)
    const ids = items.map((i) => i.id)
    const labels = Object.fromEntries(items.map((i) => [i.id, i.label]))

    expect(labels.skills).toBe('能力')
    expect(ids).not.toContain('channels')
    expect(ids).not.toContain('workflows')
    expect(ids).not.toContain('mcp')
    expect(labels.cluster).toBe('高级')
    expect(ids).not.toContain('forge')
  })

  it('redirects map legacy routes into hubs', () => {
    const map = Object.fromEntries(MERGED_ROUTE_REDIRECTS.map((r) => [r.from, r.toLabel]))
    expect(map['/mcp']).toBe('/skills?tab=mcp')
    expect(map['/channels']).toBe('/skills?tab=channels')
    expect(map['/forge']).toBe('/cluster?tab=forge')
  })

  it('classic groups still filter', () => {
    expect(filterNavGroups(CLASSIC_NAV_GROUPS).length).toBeGreaterThan(0)
  })
})
