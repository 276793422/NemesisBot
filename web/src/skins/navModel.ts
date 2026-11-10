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
  // 行动连接器（connectors feature）：IoT/单机预设关 → connectors.* 模块退出。
  connectors: 'CONNECTORS',
  // 代码开发页（lsp feature）：CodingView 消费的 coding.lsp_* 全属 coding 模块。
  coding: 'LSP',
  // 本地模型页（voice feature）：消费 voice.status/install_runtime（同 VoiceTab）。
  'local-models': 'VOICE',
  // L7c：凭据 Vault 页与安全域同一 feature 闸（后端 handler 挂 security）。
  vault: 'SECURITY',
  // ── 安全分组（十层页面化 2026-11-10）：全部挂 SECURITY/SCANNER/
  // SANDBOX/VM 既有 feature 名——VITE_FEATURE 名集合不变，契约测试
  // 天然绿；条目级门控与后端模块一致（R497：门控错挂 = 路由 404）。
  'security-overview': 'SECURITY',
  'layer-injection': 'SECURITY',
  'layer-command': 'SECURITY',
  'layer-abac': 'SECURITY',
  'layer-credential': 'SECURITY',
  'layer-dlp': 'SECURITY',
  'layer-ssrf': 'SECURITY',
  'layer-audit': 'SECURITY',
  guardian: 'SECURITY',
  approval: 'SECURITY',
  // 扫描器挂独立 scanner feature（2026-09-28 从 security 拆出）。
  scanner: 'SCANNER',
  sandbox: 'SANDBOX',
  vm: 'VM',
  board: 'BOARD',
  // 交付审核页与看板同一 feature 闸（发布命令 delivery.publish 挂 board 模块）。
  deliveries: 'BOARD',
  terminal: 'TERMINAL',
  // 更新页（selfupdate feature / VITE_FEATURE_SELFUPDATE 门控）：版本对照 +
  // 手动更新完整面；侧栏自更新长条保留快速入口（同 feature）。
  update: 'SELFUPDATE',
  // 紫微系统（2026-10-22）：三方程序受控接管（VITE_FEATURE_ZIWEI 门控）。
  ziwei: 'ZIWEI',
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
    // 多智能体协作三件套（2026-10-25 用户决策独立成组）：集群=执行入口、
    // 看板=任务派发入口、交付=交付账本人审 + 发布留痕。三者同一工作域，
    // 原「配置」组混排语义不清。
    title: '团队',
    items: [
      { id: 'cluster', label: '集群', route: '/cluster', icon: 'M6 3v18 M18 3v18 M3 6h18 M3 18h18 M3 12h18' },
      // W2 P1 (2026-08-31)：托管 Agent 看板（board feature 门控，VITE_FEATURE_BOARD）。
      { id: 'board', label: '看板', route: '/board', icon: 'M4 4h4v10H4zM10 4h4v16h-4zM16 4h4v7h-4z' },
      // 交付（P4 人审闭环，2026-10-25 从「交付审核」更名并移入本组）：交付账本裁决 + 发布留痕。
      { id: 'deliveries', label: '交付', route: '/deliveries', icon: 'M12 2l4 4-4 4V7H8v10H6V7H2V3h4V2h6zm5 7h5v13H9v-5h2v3h7V11h-1V9z' },
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
      // 2026-08-29：MCP 之下新增 HOOK/命令/插件页。
      // （子Agent 骨架页 2026-10-20 下线；2026-11-03 子代理可见性以实页
      // 回归——spawn/后台/集群委派台账 + 事件环回放；2026-11-07 并入执行
      // 单元复合页 /executors。）
      { id: 'hooks', label: 'HOOK', route: '/hooks', icon: 'M18 16.98h-5.99c-1.1 0-1.95.94-2.48 1.9A4 4 0 0 1 2 17c.01-.7.2-1.4.57-2m6.65 2H5a2 2 0 0 1 0-4h.01L7.9 8.08A4 4 0 0 1 12 2a4 4 0 0 1 4 4c0 .35-.05.71-.14 1.05M12 8a4 4 0 0 1 4 4c0 1.1-.45 2.1-1.17 2.83L12 18' },
      { id: 'commands', label: '命令', route: '/commands', icon: 'M4 17l6-6-6-6M12 19h8' },
      { id: 'plugins', label: '插件', route: '/plugins', icon: 'M12 22v-5M9 8V2M15 8V2M6 8h12v4a6 6 0 0 1-12 0V8z' },
      // 执行单元（2026-11-07 布局重构，原「子代理」更名扩容）：agent 派生
      // 执行单元复合页——【子代理】spawn/后台/集群委派台账 + 【后台任务】
      // background_start 进程台账两 TAB（/executors?tab=）。旧 /subagents、
      // /processes 路由 redirect 保深链兼容（?focus= 透传）。原独立「后台
      // 任务」侧栏条目撤并（管理组 15 → 14 项）。
      { id: 'executors', label: '执行单元', route: '/executors', icon: 'M12 2a3 3 0 0 1 3 3c0 1.3-.84 2.4-2 2.82V10h3a3 3 0 0 1 3 3v1.18c1.16.42 2 1.52 2 2.82a3 3 0 1 1-5.83-1H13v1.18a3 3 0 1 1-2 0V14H7.83A3.001 3.001 0 1 1 2 17c0-1.3.84-2.4 2-2.82V13a3 3 0 0 1 3-3h3V7.82A3.001 3.001 0 0 1 12 2z' },
      { id: 'channels', label: '通道', route: '/channels', icon: 'M22 12h-4l-3 9L9 3l-3 9H2' },
      // 行动连接器（2026-10-14 AI 原生行动代理）：注册表/回执/纸面账本只读面。
      { id: 'connectors', label: 'Connectors', route: '/connectors', icon: 'M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71' },
      { id: 'coding', label: '代码开发', route: '/coding', icon: 'M16 18l6-6-6-6M8 6l-6 6 6 6' },
      { id: 'workflows', label: '工作流', route: '/workflows', icon: 'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7zM10 6.5h4M10 17.5h4M6.5 10v4M17.5 10v4' },
    ],
  },
  {
    title: '自进化',
    items: [
      { id: 'forge', label: 'Forge', route: '/forge', icon: 'M13 10V3L4 14h7v7l9-11h-7z' },
      // B 线用户习惯画像（2026-10-14）：运行期 config 闸，非 cargo feature。
      { id: 'profile', label: '用户习惯', route: '/profile', icon: 'M12 12c2.21 0 4-1.79 4-4s-1.79-4-4-4-4 1.79-4 4 1.79 4 4 4zm0 2c-2.67 0-8 1.34-8 4v2h16v-2c0-2.66-5.33-4-8-4z M9 5l3 3 3-3' },
    ],
  },
  {
    title: '配置',
    items: [
      { id: 'settings', label: '设置', route: '/settings', icon: 'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-2.82 1.18V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1.08-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0-1.18-2.82H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1.08 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 2.82-1.18V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1.08 1.51 1.65 1.65 0 0 0 1.82-.33l-.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0 1.18 2.82H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1.08z' },
      { id: 'tools', label: 'Tools', route: '/tools', icon: 'M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z' },
      { id: 'tasks', label: '任务', route: '/tasks', icon: 'M9 11l3 3L22 4 M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11' },
      // 紫微系统（2026-10-22）：三方程序受控接管【万物皆工具】
      // （VITE_FEATURE_ZIWEI 门控；config ziwei.enabled 运行闸默认关）。
      { id: 'ziwei', label: '紫微系统', route: '/ziwei', icon: 'M12 2l2.4 7.2H22l-6 4.4 2.3 7.2-6.3-4.5-6.3 4.5L8 13.6 2 9.2h7.6z' },
      // L8：PTY 内嵌终端（VITE_FEATURE_TERMINAL 门控）。
      { id: 'terminal', label: '终端', route: '/terminal', icon: 'M4 17l6-6-6-6M12 19h8' },
      { id: 'local-models', label: '本地模型', route: '/local-models', icon: 'M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z M3.27 6.96L12 12.01l8.73-5.05 M12 22.08V12' },
      { id: 'sdk', label: '二次开发', route: '/sdk', icon: 'M10 20l4-16m4 4l4 4-4 4M6 16l-4-4 4-4' },
      // 代理设置（2026-09-17）：per-model 代理总览 + 环境变量 + lane 支持。
      { id: 'proxy-settings', label: '代理设置', route: '/proxy-settings', icon: 'M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10zM2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z' },
    ],
  },
  {
    // 安全分组（十层页面化 2026-11-10，用户拍板插【配置】组下方）：
    // 总览 + 十层各一页（①②③④⑤⑥⑧配置驱动层页 + ⑦⑨⑩独立子系统
    // 页归组）+ Guardian 语义二审 + Approval 人工审批 + 凭据 vault 组尾。
    // 旧 SecurityView 四 tab 页退役，/security 深链 redirect 总览。
    title: '安全',
    items: [
      { id: 'security-overview', label: '总览', route: '/security-overview', icon: 'M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z' },
      // 十层（层号 = pipeline 执行序）：
      { id: 'layer-injection', label: '①注入检测', route: '/security/layer/injection', icon: 'M12 2v4M12 18v4M4.93 4.93l2.83 2.83M16.24 16.24l2.83 2.83M2 12h4M18 12h4M4.93 19.07l2.83-2.83M16.24 7.76l2.83-2.83' },
      { id: 'layer-command', label: '②命令守卫', route: '/security/layer/command', icon: 'M8 9l-4 3 4 3M16 9l4 3-4 3M13 5l-2 14' },
      { id: 'layer-abac', label: '③ABAC 访问控制', route: '/security/layer/abac', icon: 'M21 2l-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0l3 3L22 7l-3-3m-3.5 3.5L19 4' },
      { id: 'layer-credential', label: '④凭据扫描', route: '/security/layer/credential', icon: 'M19 11H5a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7a2 2 0 0 0-2-2zM7 11V7a5 5 0 0 1 10 0v4' },
      { id: 'layer-dlp', label: '⑤DLP 防泄漏', route: '/security/layer/dlp', icon: 'M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24M1 1l22 22' },
      { id: 'layer-ssrf', label: '⑥SSRF 防护', route: '/security/layer/ssrf', icon: 'M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10zM2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z' },
      // ⑦ 病毒扫描：独立子系统页归组（路由/组件不动，label 加层号）。
      { id: 'scanner', label: '⑦病毒扫描', route: '/scanner', icon: 'M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z' },
      { id: 'layer-audit', label: '⑧审计链', route: '/security/layer/audit', icon: 'M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z M14 2v6h6 M16 13H8 M16 17H8' },
      // ⑨⑩ 执行体隔离双页：独立子系统页归组（路由/组件不动）。
      { id: 'sandbox', label: '⑨沙盒', route: '/sandbox', icon: 'M5 11h14a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2zm0 0V7a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v4 M12 15v3' },
      { id: 'vm', label: '⑩虚拟化', route: '/vm', icon: 'M2 3h20v14H2zM8 21h8M12 17v4' },
      // 附加机制两页（不在十层编号内）：LLM judge 语义二审 + 人工审批
      // （eval 评估规则暂放 Approval 页一节，位置后议）。
      { id: 'guardian', label: 'Guardian 二审', route: '/security/guardian', icon: 'M9 12l2 2 4-4M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z' },
      { id: 'approval', label: '人工审批', route: '/security/approval', icon: 'M16 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2M8.5 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM17 11l2 2 4-4' },
      // L7c：凭据 Vault（组尾；VITE_FEATURE_SECURITY 门控）。
      { id: 'vault', label: '凭据', route: '/vault', icon: 'M18 8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h11a2 2 0 0 1 2 2v2zm-6 4a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4a2 2 0 0 1 2-2h7zM4 6v.01M8 6v.01' },
    ],
  },
  {
    title: '其他',
    items: [
      // 更新（selfupdate feature 门控）：当前运行版本 vs 云端最新对照 +
      // 手动更新操作面——更新成功重启后长条消失，此处闭环「现在跑的就是
      // 新版本」的确认。
      { id: 'update', label: '更新', route: '/update', icon: 'M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15' },
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
