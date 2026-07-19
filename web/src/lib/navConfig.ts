/**
 * Compact navigation: related features share one sidebar entry + page tabs.
 *
 * Hubs:
 * - 主页: 概览 / 用量 / 日志 / 记忆
 * - 能力: 技能 / 商店 / MCP / 通道 / 工作流
 * - 安全: 策略 / 扫描器 / 沙盒
 * - 高级(集群): 集群 / Forge
 * - 设置: Agent… / 工具笔记 / 任务
 */

export interface NavItem {
  id: string
  label: string
  path: string
  icon: string
  unfinished?: boolean
}

export interface NavGroup {
  title: string
  items: NavItem[]
}

export const NAV_ITEM_FEATURE: Record<string, string> = {
  usage: 'USAGE',
  memory: 'MEMORY',
  workflows: 'WORKFLOW',
  forge: 'FORGE',
  cluster: 'CLUSTER',
  security: 'SECURITY',
  sandbox: 'SANDBOX',
  overview: '',
  skills: '',
}

export function featureOn(id: string): boolean {
  const f = NAV_ITEM_FEATURE[id]
  if (!f) return true
  return (import.meta.env['VITE_FEATURE_' + f] as string | undefined) !== 'false'
}

/** Classic nav — ~8 real destinations */
export const CLASSIC_NAV_GROUPS: NavGroup[] = [
  {
    title: '主要',
    items: [
      { id: 'chat', label: '聊天', path: '/', icon: 'M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z' },
      { id: 'overview', label: '主页', path: '/overview', icon: 'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z' },
      { id: 'persona', label: '人格', path: '/persona', icon: 'M12 12c2.21 0 4-1.79 4-4s-1.79-4-4-4-4 1.79-4 4 1.79 4 4 4zm0 2c-2.67 0-8 1.34-8 4v2h16v-2c0-2.66-5.33-4-8-4z' },
      { id: 'models', label: '模型', path: '/models', icon: 'M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5' },
    ],
  },
  {
    title: '扩展',
    items: [
      { id: 'skills', label: '能力', path: '/skills', icon: 'M9.663 17h4.673M12 3v1m6.364 1.636l-.707.707M21 12h-1M4 12H3m3.343-5.657l-.707-.707m2.828 9.9a5 5 0 1 1 7.072 0l-.548.547A3.374 3.374 0 0 0 14 18.469V19a2 2 0 1 1-4 0v-.531c0-.895-.356-1.754-.988-2.386l-.548-.547z' },
      { id: 'cluster', label: '高级', path: '/cluster', icon: 'M6 3v18 M18 3v18 M3 6h18 M3 18h18 M3 12h18' },
    ],
  },
  {
    title: '系统',
    items: [
      { id: 'settings', label: '设置', path: '/settings', icon: 'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-2.82 1.18V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1.08-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0-1.18-2.82H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1.08 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 2.82-1.18V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1.08 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0 1.18 2.82H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1.08z' },
      { id: 'security', label: '安全', path: '/security', icon: 'M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z' },
      { id: 'about', label: '关于', path: '/about', icon: 'M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10zM12 8v4M12 16h.01' },
    ],
  },
]

/**
 * Friendly shell: flat list (no collapsible “更多”).
 * Hubs already absorbed MCP/通道/工作流/日志/记忆等，条目不会太长。
 */
export const FRIENDLY_NAV_IDS = [
  'chat',
  'overview',
  'persona',
  'models',
  'skills',
  'settings',
  'security',
  'cluster',
  'about',
] as const

/** @deprecated use FRIENDLY_NAV_IDS — kept so old imports/tests can migrate */
export const FRIENDLY_PRIMARY_IDS = FRIENDLY_NAV_IDS
export const FRIENDLY_MORE_IDS = [] as const

export function flattenNavItems(groups: NavGroup[]): NavItem[] {
  return groups.flatMap((g) => g.items)
}

export function filterNavGroups(
  groups: NavGroup[],
  opts: { includeUnfinished?: boolean } = {},
): NavGroup[] {
  const includeUnfinished = opts.includeUnfinished ?? false
  return groups
    .map((g) => ({
      ...g,
      items: g.items.filter((i) => featureOn(i.id) && (includeUnfinished || !i.unfinished)),
    }))
    .filter((g) => g.items.length > 0)
}

export function itemsByIds(ids: readonly string[]): NavItem[] {
  const all = flattenNavItems(CLASSIC_NAV_GROUPS)
  const map = new Map(all.map((i) => [i.id, i]))
  return ids
    .map((id) => map.get(id))
    .filter((i): i is NavItem => !!i && featureOn(i.id) && !i.unfinished)
}

export const MERGED_ROUTE_REDIRECTS: {
  from: string
  to: { path: string; query: { tab: string } }
  toLabel: string
}[] = [
  // 主页
  { from: '/usage', to: { path: '/overview', query: { tab: 'usage' } }, toLabel: '/overview?tab=usage' },
  { from: '/logs', to: { path: '/overview', query: { tab: 'logs' } }, toLabel: '/overview?tab=logs' },
  { from: '/memory', to: { path: '/overview', query: { tab: 'memory' } }, toLabel: '/overview?tab=memory' },
  // 能力 hub
  { from: '/mcp', to: { path: '/skills', query: { tab: 'mcp' } }, toLabel: '/skills?tab=mcp' },
  { from: '/channels', to: { path: '/skills', query: { tab: 'channels' } }, toLabel: '/skills?tab=channels' },
  { from: '/workflows', to: { path: '/skills', query: { tab: 'workflows' } }, toLabel: '/skills?tab=workflows' },
  // 高级 hub (cluster page)
  { from: '/forge', to: { path: '/cluster', query: { tab: 'forge' } }, toLabel: '/cluster?tab=forge' },
  // 设置
  { from: '/tasks', to: { path: '/settings', query: { tab: 'tasks' } }, toLabel: '/settings?tab=tasks' },
  { from: '/tools', to: { path: '/settings', query: { tab: 'tools-md' } }, toLabel: '/settings?tab=tools-md' },
  // 人格 / 安全 / 模型 / 关于
  { from: '/persona-shop', to: { path: '/persona', query: { tab: 'shop' } }, toLabel: '/persona?tab=shop' },
  { from: '/scanner', to: { path: '/security', query: { tab: 'scanner' } }, toLabel: '/security?tab=scanner' },
  { from: '/sandbox', to: { path: '/security', query: { tab: 'sandbox' } }, toLabel: '/security?tab=sandbox' },
  { from: '/license', to: { path: '/about', query: { tab: 'license' } }, toLabel: '/about?tab=license' },
  { from: '/local-models', to: { path: '/models', query: { tab: 'local' } }, toLabel: '/models?tab=local' },
]
