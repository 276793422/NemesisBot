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
  { path: '/local-models', name: 'local-models', component: () => import('../views/LocalModelsView.vue') },
  // Feature-gated views: dropped from the bundle when the matching cargo
  // feature is off (Vite tree-shakes the dead `import()` when
  // `import.meta.env.VITE_FEATURE_X === 'false'`). `!== 'false'` = default
  // include, so a full build (no .env) keeps every view. See
  // nemesis-build-config `export --frontend-env` + scripts/customize.
  ...(import.meta.env.VITE_FEATURE_MEMORY !== 'false' ? [{ path: '/memory', name: 'memory', component: () => import('../views/MemoryView.vue') }] : []),
  { path: '/skills', name: 'skills', component: () => import('../views/SkillsView.vue') },
  { path: '/mcp', name: 'mcp', component: () => import('../views/McpView.vue') },
  // 2026-08-29：管理分组新增页（MCP 之后）。Hook 页自设置页迁移；
  // 命令/插件/子Agent 为骨架页，细节待定。
  { path: '/hooks', name: 'hooks', component: () => import('../views/HookView.vue') },
  { path: '/commands', name: 'commands', component: () => import('../views/CommandsView.vue') },
  { path: '/plugins', name: 'plugins', component: () => import('../views/PluginsView.vue') },
  { path: '/subagents', name: 'subagents', component: () => import('../views/SubagentView.vue') },
  { path: '/channels', name: 'channels', component: () => import('../views/ChannelsView.vue') },
  // P2-1 (2026-08-24 UI entry gap): 「代码开发」页（LSP/CC/Codex 委派配置）。
  { path: '/coding', name: 'coding', component: () => import('../views/CodingView.vue') },
  ...(import.meta.env.VITE_FEATURE_WORKFLOW !== 'false' ? [{ path: '/workflows', name: 'workflows', component: () => import('../views/WorkflowView.vue') }] : []),
  // Note: `/workflow/chat/<index>` is served as a standalone HTML page
  // (workflow-chat.html, see vite.config.ts + serve_embedded_static),
  // not as a Vue Router route. The dashboard SPA never renders this path;
  // WorkflowList opens it via window.open to a fresh tab.
  ...(import.meta.env.VITE_FEATURE_FORGE !== 'false' ? [{ path: '/forge', name: 'forge', component: () => import('../views/ForgeView.vue') }] : []),
  { path: '/persona-shop', name: 'persona-shop', component: () => import('../views/PersonaShopView.vue') },
  // Configuration
  { path: '/settings', name: 'settings', component: () => import('../views/SettingsView.vue') },
  { path: '/tools', name: 'tools', component: () => import('../views/ToolsView.vue') },
  { path: '/tasks', name: 'tasks', component: () => import('../views/TasksView.vue') },
  ...(import.meta.env.VITE_FEATURE_CLUSTER !== 'false' ? [{ path: '/cluster', name: 'cluster', component: () => import('../views/ClusterView.vue') }] : []),
  ...(import.meta.env.VITE_FEATURE_SECURITY !== 'false' ? [{ path: '/security', name: 'security', component: () => import('../views/SecurityView.vue') }] : []),
  ...(import.meta.env.VITE_FEATURE_SECURITY !== 'false' ? [{ path: '/scanner', name: 'scanner', component: () => import('../views/ScannerView.vue') }] : []),
  ...(import.meta.env.VITE_FEATURE_SANDBOX !== 'false' ? [{ path: '/sandbox', name: 'sandbox', component: () => import('../views/SandboxView.vue') }] : []),
  // P2-2 (2026-08-24 UI entry gap): 「二次开发」SDK 内嵌导出页。
  { path: '/sdk', name: 'sdk', component: () => import('../views/SdkView.vue') },
  // Other
  { path: '/about', name: 'about', component: () => import('../views/AboutView.vue') },
  { path: '/license', name: 'license', component: () => import('../views/LicenseView.vue') },
]

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
})
