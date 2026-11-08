import { createRouter, createWebHashHistory, type RouteRecordRaw } from 'vue-router'

const routes: RouteRecordRaw[] = [
  // Main
  { path: '/', name: 'chat', component: () => import('../views/ChatView.vue') },
  { path: '/overview', name: 'overview', component: () => import('../views/OverviewView.vue') },
  ...(import.meta.env.VITE_FEATURE_USAGE !== 'false' ? [{ path: '/usage', name: 'usage', component: () => import('../views/UsageView.vue') }] : []),
  { path: '/persona', name: 'persona', component: () => import('../views/PersonaView.vue') },
  // Management
  { path: '/logs', name: 'logs', component: () => import('../views/LogsView.vue') },
  { path: '/models', name: 'models', component: () => import('../views/ModelsView.vue') },
  // 本地模型页（voice feature / VITE_FEATURE_VOICE 门控）：消费 voice.status/
  // install_runtime，IoT 预设 voice=false 时模块整包退出。页面内 unavailable
  // 空态保留作纵深防御（非 Windows 等运行期场景仍需诚实提示）。
  ...(import.meta.env.VITE_FEATURE_VOICE !== 'false' ? [{ path: '/local-models', name: 'local-models', component: () => import('../views/LocalModelsView.vue') }] : []),
  // Feature-gated views: dropped from the bundle when the matching cargo
  // feature is off (Vite tree-shakes the dead `import()` when
  // `import.meta.env.VITE_FEATURE_X === 'false'`). `!== 'false'` = default
  // include, so a full build (no .env) keeps every view. See
  // nemesis-build-config `export --frontend-env` + scripts/customize.
  ...(import.meta.env.VITE_FEATURE_MEMORY !== 'false' ? [{ path: '/memory', name: 'memory', component: () => import('../views/MemoryView.vue') }] : []),
  { path: '/skills', name: 'skills', component: () => import('../views/SkillsView.vue') },
  { path: '/mcp', name: 'mcp', component: () => import('../views/McpView.vue') },
  // 2026-08-29：管理分组新增页（MCP 之后）。Hook 页自设置页迁移；
  // 命令/插件为实页（命令=slash 命令 CRUD，插件=三 Tab 含 WASM）。
  // 执行单元复合页（2026-11-07 布局重构）：子代理 + 后台任务两 TAB
  // （?tab=subagents|processes，默认子代理）；旧深链路由 redirect 保兼容。
  { path: '/hooks', name: 'hooks', component: () => import('../views/HookView.vue') },
  { path: '/commands', name: 'commands', component: () => import('../views/CommandsView.vue') },
  { path: '/plugins', name: 'plugins', component: () => import('../views/PluginsView.vue') },
  { path: '/executors', name: 'executors', component: () => import('../views/ExecutorsView.vue') },
  // 旧路由 redirect：query 全透传——聊天迷你 tail「完整输出→」深链
  // /processes?focus= 与历史工作块 spawn 行钻取 /subagents?focus= 不破。
  { path: '/subagents', redirect: (to) => ({ path: '/executors', query: { ...to.query, tab: 'subagents' } }) },
  { path: '/processes', redirect: (to) => ({ path: '/executors', query: { ...to.query, tab: 'processes' } }) },
  { path: '/channels', name: 'channels', component: () => import('../views/ChannelsView.vue') },
  // 行动连接器（2026-10-14 AI 原生行动代理）：只读面——连接器注册表 +
  // 行动回执（行动账本）+ 纸面券商模拟账本（WSAPI connectors 模块）。
  // connectors feature 门控：IoT/单机预设关掉后 WSAPI connectors.* 模块
  // 整包退出（features.toml「IoT/单机建议关」），页面必须同步裁剪
  //（R497：此前两预设关掉后页面仍注册，打开=对着已消失后端报 unknown command）。
  ...(import.meta.env.VITE_FEATURE_CONNECTORS !== 'false' ? [{ path: '/connectors', name: 'connectors', component: () => import('../views/ConnectorsView.vue') }] : []),
  // P2-1 (2026-08-24 UI entry gap): 「代码开发」页（LSP/CLI 委派配置）。
  // lsp feature 门控：CodingView 的 WSAPI 消费面（coding.config/lsp_status/
  // lsp_install）全部挂在 coding 模块，IoT 预设 lsp=false 时整模块退出。
  ...(import.meta.env.VITE_FEATURE_LSP !== 'false' ? [{ path: '/coding', name: 'coding', component: () => import('../views/CodingView.vue') }] : []),
  ...(import.meta.env.VITE_FEATURE_WORKFLOW !== 'false' ? [{ path: '/workflows', name: 'workflows', component: () => import('../views/WorkflowView.vue') }] : []),
  // Note: `/workflow/chat/<index>` is served as a standalone HTML page
  // (workflow-chat.html, see vite.config.ts + serve_embedded_static),
  // not as a Vue Router route. The dashboard SPA never renders this path;
  // WorkflowList opens it via window.open to a fresh tab.
  ...(import.meta.env.VITE_FEATURE_FORGE !== 'false' ? [{ path: '/forge', name: 'forge', component: () => import('../views/ForgeView.vue') }] : []),
  // B 线用户习惯画像（2026-10-14）：运行期 config 闸（user_profile.enabled），
  // 非 cargo feature——路由恒在，页面内诚实展示开关态。
  { path: '/profile', name: 'profile', component: () => import('../views/ProfileView.vue') },
  { path: '/persona-shop', name: 'persona-shop', component: () => import('../views/PersonaShopView.vue') },
  // Configuration
  { path: '/settings', name: 'settings', component: () => import('../views/SettingsView.vue') },
  { path: '/tools', name: 'tools', component: () => import('../views/ToolsView.vue') },
  { path: '/tasks', name: 'tasks', component: () => import('../views/TasksView.vue') },
  // W2 P3 (2026-08-31)：托管 Agent 看板（board feature / VITE_FEATURE_BOARD 门控）。
  // 容器视图 = 看板（Kanban 拖拽）/ 列表 / 项目 / 收件箱 四页签（ClusterView 同构）。
  ...(import.meta.env.VITE_FEATURE_BOARD !== 'false' ? [{ path: '/board', name: 'board', component: () => import('../views/BoardView.vue') }] : []),
  // 交付审核（P4 人审闭环，2026-10-25）：交付账本查询/裁决/发布留痕。
  // 发布命令（delivery.publish，flow 模板声明驱动）挂在 board 模块——与看板同 feature 门控。
  ...(import.meta.env.VITE_FEATURE_BOARD !== 'false' ? [{ path: '/deliveries', name: 'deliveries', component: () => import('../views/DeliveriesView.vue') }] : []),
  // 统一工作台（人工触点体系 P2，2026-11-07）：/workbench/touchpoint-{kind}/{id}
  // （kind=spec|prototype——前缀形态与后端 kind_of strip_prefix 对齐）。
  // 触点详情/裁决重页（规格全文/原型报告 markdown + 决定史 + 粘性操作条）。
  // board feature 门控（workbench.get/respond 挂 board 模块）。
  ...(import.meta.env.VITE_FEATURE_BOARD !== 'false' ? [{ path: '/workbench/:kind/:id', name: 'workbench', component: () => import('../views/WorkbenchView.vue') }] : []),
  ...(import.meta.env.VITE_FEATURE_CLUSTER !== 'false' ? [{ path: '/cluster', name: 'cluster', component: () => import('../views/ClusterView.vue') }] : []),
  ...(import.meta.env.VITE_FEATURE_SECURITY !== 'false' ? [{ path: '/security', name: 'security', component: () => import('../views/SecurityView.vue') }] : []),
  // L7c (2026-10-13)：凭据 Vault 管理页（后端 vault handler 挂 security
  // feature——与 SecurityView 同一 VITE_FEATURE_SECURITY 门控）。
  ...(import.meta.env.VITE_FEATURE_SECURITY !== 'false' ? [{ path: '/vault', name: 'vault', component: () => import('../views/VaultView.vue') }] : []),
  ...(import.meta.env.VITE_FEATURE_SCANNER !== 'false' ? [{ path: '/scanner', name: 'scanner', component: () => import('../views/ScannerView.vue') }] : []),
  ...(import.meta.env.VITE_FEATURE_SANDBOX !== 'false' ? [{ path: '/sandbox', name: 'sandbox', component: () => import('../views/SandboxView.vue') }] : []),
  // P5 第 4 步 ②：桌面级 VM 管理页（vm feature / VITE_FEATURE_VM 门控）。
  ...(import.meta.env.VITE_FEATURE_VM !== 'false' ? [{ path: '/vm', name: 'vm', component: () => import('../views/VmView.vue') }] : []),
  // 紫微系统（2026-10-22）：三方程序受控接管（【万物皆工具】）。
  // ziwei feature / VITE_FEATURE_ZIWEI 门控；config ziwei.enabled 是另一道
  // 运行闸（默认关——页面上诚实报未启用）。
  ...(import.meta.env.VITE_FEATURE_ZIWEI !== 'false' ? [{ path: '/ziwei', name: 'ziwei', component: () => import('../views/ZiweiView.vue') }] : []),
  // L8 (2026-09-07)：PTY 内嵌终端（terminal feature / VITE_FEATURE_TERMINAL 门控）。
  // 后端 /ws/pty 另有 config terminal.enabled 运行闸（默认关）——路由可达时
  // 页面会显示连接失败，属于诚实降级。
  ...(import.meta.env.VITE_FEATURE_TERMINAL !== 'false' ? [{ path: '/terminal', name: 'terminal', component: () => import('../views/TerminalView.vue') }] : []),
  // P2-2 (2026-08-24 UI entry gap): 「二次开发」SDK 内嵌导出页。
  { path: '/sdk', name: 'sdk', component: () => import('../views/SdkView.vue') },
  // 面板页（2026-10-31 插件 UI 面板扩展 P2）：静态参数路由，插件声明面
  // 动态挂载——深链免 addRoute 时窗；route 段唯一性由后端注册面保证。
  // 清单来自 plugins.ui.registry（见 composables/usePanels.ts）。
  { path: '/panel/:route', name: 'panel', component: () => import('../views/PanelView.vue') },
  // 代理设置（2026-09-17）：per-model 代理总览 + 环境变量 + lane 支持。
  { path: '/proxy-settings', name: 'proxy-settings', component: () => import('../views/ProxySettingsView.vue') },
  // Other
  // 更新页（selfupdate feature / VITE_FEATURE_SELFUPDATE 门控）：版本对照 +
  // 手动更新操作面。侧栏自更新长条（同 feature 门控）保留快速入口。
  ...(import.meta.env.VITE_FEATURE_SELFUPDATE !== 'false' ? [{ path: '/update', name: 'update', component: () => import('../views/UpdateView.vue') }] : []),
  { path: '/about', name: 'about', component: () => import('../views/AboutView.vue') },
  { path: '/license', name: 'license', component: () => import('../views/LicenseView.vue') },
]

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
})
