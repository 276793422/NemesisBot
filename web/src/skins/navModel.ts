/**
 * 导航模型（宿主侧单一真相源）。
 *
 * P2b 契约层（docs/PLAN/2026-09-29_newui-style-skin-and-script-skin.md §6）：
 * 皮肤脚本只对这份结构化模型编程（`window.NemesisSkin.nav.model()`），
 * 不许抓取宿主 DOM——宿主内部重构（Sidebar 换实现/路由重组）不破契约。
 * Sidebar.vue 与契约 API 都消费本模块（单一真相源，两处列表漂移 = 测试红）。
 *
 * feature 门控与 router/index.ts 同构（同一 `!== 'false'` 判据、同一
 * VITE_FEATURE_<NAME> 映射）：customize 裁剪构建时双方同步消失。
 */

export interface NavItemModel {
  /** 稳定路由 id（navigate 的入参；与 router route name 一致） */
  id: string
  label: string
  /** SVG path data（24×24 viewBox stroke 图标） */
  icon: string
  /** 路由路径（hash 模式） */
  route: string
}

export interface NavItemContract extends NavItemModel {
  /** 所属分组标题（P3-a 重组按 group 聚合） */
  group: string
}

export interface NavGroupModel {
  title: string
  items: NavItemModel[]
}

/** feature 门控：item id → VITE_FEATURE_<NAME>。不在表内 = 恒显示。
 * 与 router/index.ts 的门控保持同一映射（两边都要改，契约测试兜底）。 */
const ITEM_FEATURE: Record<string, string> = {
  usage: 'USAGE',
  memory: 'MEMORY',
  workflows: 'WORKFLOW',
  forge: 'FORGE',
  cluster: 'CLUSTER',
  security: 'SECURITY',
  scanner: 'SECURITY',
  sandbox: 'SANDBOX',
  board: 'BOARD',
  terminal: 'TERMINAL',
}

export const NAV_GROUPS: NavGroupModel[] = [
  {
    title: '主要',
    items: [
      { id: 'chat', label: '聊天', route: '/', icon: 'M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z' },
      { id: 'overview', label: '概览', route: '/overview', icon: 'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z' },
      { id: 'usage', label: '使用统计', route: '/usage', icon: 'M18 20V10M12 20V4M6 20v-6' },
      { id: 'persona', label: '人格', route: '/persona', icon: 'M12 12c2.21 0 4-1.79 4-4s-1.79-4-4-4-4 1.79-4 4 1.79 4 4 4zm0 2c-2.67 0-8 1.34-8 4v2h16v-2c0-2.66-5.33-4-8-4z' },
    ],
  },
  {
    title: '管理',
    items: [
      { id: 'logs', label: '日志', route: '/logs', icon: 'M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z M14 2v6h6 M16 13H8 M16 17H8' },
      { id: 'models', label: '模型', route: '/models', icon: 'M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5' },
      { id: 'memory', label: '记忆', route: '/memory', icon: 'M12 2a7 7 0 0 1 7 7c0 5.25-7 13-7 13S5 14.25 5 9a7 7 0 0 1 7-7z M12 9a1 1 0 1 0 0-2 1 1 0 0 0 0 2z' },
      { id: 'persona-shop', label: '人格超市', route: '/persona-shop', icon: 'M3 3h18v18H3V3zm3 3h12v3H6V6zm0 5h12v3H6v-3zm0 5h8v3H6v-3z' },
      { id: 'skills', label: 'Skills', route: '/skills', icon: 'M9.663 17h4.673M12 3v1m6.364 1.636l-.707.707M21 12h-1M4 12H3m3.343-5.657l-.707-.707m2.828 9.9a5 5 0 1 1 7.072 0l-.548.547A3.374 3.374 0 0 0 14 18.469V19a2 2 0 1 1-4 0v-.531c0-.895-.356-1.754-.988-2.386l-.548-.547z' },
      { id: 'mcp', label: 'MCP', route: '/mcp', icon: 'M4 6h16M4 12h16M4 18h16' },
      // 2026-08-29：MCP 之下新增 HOOK/命令/插件/子Agent 四页。
      { id: 'hooks', label: 'HOOK', route: '/hooks', icon: 'M18 16.98h-5.99c-1.1 0-1.95.94-2.48 1.9A4 4 0 0 1 2 17c.01-.7.2-1.4.57-2m6.65 2H5a2 2 0 0 1 0-4h.01L7.9 8.08A4 4 0 0 1 12 2a4 4 0 0 1 4 4c0 .35-.05.71-.14 1.05M12 8a4 4 0 0 1 4 4c0 1.1-.45 2.1-1.17 2.83L12 18' },
      { id: 'commands', label: '命令', route: '/commands', icon: 'M4 17l6-6-6-6M12 19h8' },
      { id: 'plugins', label: '插件', route: '/plugins', icon: 'M12 22v-5M9 8V2M15 8V2M6 8h12v4a6 6 0 0 1-12 0V8z' },
      { id: 'subagents', label: '子Agent', route: '/subagents', icon: 'M12 8V4M8 4h8M5 8h14a1 1 0 0 1 1 1v9a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V9a1 1 0 0 1 1-1zM9 13h.01M15 13h.01' },
      { id: 'channels', label: '通道', route: '/channels', icon: 'M22 12h-4l-3 9L9 3l-3 9H2' },
      { id: 'coding', label: '代码开发', route: '/coding', icon: 'M16 18l6-6-6-6M8 6l-6 6 6 6' },
      { id: 'workflows', label: '工作流', route: '/workflows', icon: 'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7zM10 6.5h4M10 17.5h4M6.5 10v4M17.5 10v4' },
    ],
  },
  {
    title: '自进化',
    items: [
      { id: 'forge', label: 'Forge', route: '/forge', icon: 'M13 10V3L4 14h7v7l9-11h-7z' },
    ],
  },
  {
    title: '配置',
    items: [
      { id: 'settings', label: '设置', route: '/settings', icon: 'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-2.82 1.18V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1.08-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0-1.18-2.82H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1.08 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 2.82-1.18V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1.08 1.51 1.65 1.65 0 0 0 1.82-.33l-.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0 1.18 2.82H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1.08z' },
      { id: 'tools', label: 'Tools', route: '/tools', icon: 'M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z' },
      { id: 'tasks', label: '任务', route: '/tasks', icon: 'M9 11l3 3L22 4 M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11' },
      // W2 P1 (2026-08-31)：托管 Agent 看板（board feature 门控，VITE_FEATURE_BOARD）。
      { id: 'board', label: '看板', route: '/board', icon: 'M4 4h4v10H4zM10 4h4v16h-4zM16 4h4v7h-4z' },
      { id: 'cluster', label: '集群', route: '/cluster', icon: 'M6 3v18 M18 3v18 M3 6h18 M3 18h18 M3 12h18' },
      { id: 'security', label: '安全', route: '/security', icon: 'M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z' },
      { id: 'scanner', label: '扫描器', route: '/scanner', icon: 'M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z' },
      { id: 'sandbox', label: '沙盒', route: '/sandbox', icon: 'M5 11h14a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2zm0 0V7a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v4 M12 15v3' },
      // L8：PTY 内嵌终端（VITE_FEATURE_TERMINAL 门控）。
      { id: 'terminal', label: '终端', route: '/terminal', icon: 'M4 17l6-6-6-6M12 19h8' },
      { id: 'local-models', label: '本地模型', route: '/local-models', icon: 'M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z M3.27 6.96L12 12.01l8.73-5.05 M12 22.08V12' },
      { id: 'sdk', label: '二次开发', route: '/sdk', icon: 'M10 20l4-16m4 4l4 4-4 4M6 16l-4-4 4-4' },
      // 代理设置（2026-09-17）：per-model 代理总览 + 环境变量 + lane 支持。
      { id: 'proxy-settings', label: '代理设置', route: '/proxy-settings', icon: 'M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10zM2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z' },
    ],
  },
  {
    title: '其他',
    items: [
      { id: 'about', label: '关于', route: '/about', icon: 'M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10zM12 8v4M12 16h.01' },
      { id: 'license', label: 'License', route: '/license', icon: 'M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z M14 2v6h6 M16 13H8 M16 17H8 M10 9H8' },
    ],
  },
]

/** feature 门控判定（`VITE_FEATURE_X === 'false'` = 裁掉；缺省 = 开）。 */
export function navFeatureOn(id: string): boolean {
  const f = ITEM_FEATURE[id]
  if (!f) return true
  return (import.meta.env['VITE_FEATURE_' + f] as string | undefined) !== 'false'
}

/** 按 feature 门控过滤后的分组视图（Sidebar 渲染 + 契约模型共用）。 */
export function visibleNavGroups(): NavGroupModel[] {
  return NAV_GROUPS.map((g) => ({ ...g, items: g.items.filter((i) => navFeatureOn(i.id)) })).filter(
    (g) => g.items.length > 0,
  )
}

/** 扁平契约模型（`NemesisSkin.nav.model()` 载荷；group 字段补齐）。 */
export function flatNavModel(): NavItemContract[] {
  return visibleNavGroups().flatMap((g) => g.items.map((i) => ({ ...i, group: g.title })))
}
