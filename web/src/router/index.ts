import { createRouter, createWebHashHistory, type RouteRecordRaw } from 'vue-router'

const routes: RouteRecordRaw[] = [
  // Main
  { path: '/', name: 'chat', component: () => import('../views/ChatView.vue') },
  { path: '/overview', name: 'overview', component: () => import('../views/OverviewView.vue') },
  { path: '/usage', name: 'usage', component: () => import('../views/UsageView.vue') },
  { path: '/persona', name: 'persona', component: () => import('../views/PersonaView.vue') },
  // Management
  { path: '/logs', name: 'logs', component: () => import('../views/LogsView.vue') },
  { path: '/models', name: 'models', component: () => import('../views/ModelsView.vue') },
  { path: '/local-models', name: 'local-models', component: () => import('../views/LocalModelsView.vue') },
  { path: '/memory', name: 'memory', component: () => import('../views/MemoryView.vue') },
  { path: '/skills', name: 'skills', component: () => import('../views/SkillsView.vue') },
  { path: '/mcp', name: 'mcp', component: () => import('../views/McpView.vue') },
  { path: '/channels', name: 'channels', component: () => import('../views/ChannelsView.vue') },
  { path: '/workflows', name: 'workflows', component: () => import('../views/WorkflowView.vue') },
  // Note: `/workflow/chat/<index>` is served as a standalone HTML page
  // (workflow-chat.html, see vite.config.ts + serve_embedded_static),
  // not as a Vue Router route. The dashboard SPA never renders this path;
  // WorkflowList opens it via window.open to a fresh tab.
  { path: '/forge', name: 'forge', component: () => import('../views/ForgeView.vue') },
  { path: '/persona-shop', name: 'persona-shop', component: () => import('../views/PersonaShopView.vue') },
  // Configuration
  { path: '/settings', name: 'settings', component: () => import('../views/SettingsView.vue') },
  { path: '/tools', name: 'tools', component: () => import('../views/ToolsView.vue') },
  { path: '/tasks', name: 'tasks', component: () => import('../views/TasksView.vue') },
  { path: '/cluster', name: 'cluster', component: () => import('../views/ClusterView.vue') },
  { path: '/security', name: 'security', component: () => import('../views/SecurityView.vue') },
  { path: '/scanner', name: 'scanner', component: () => import('../views/ScannerView.vue') },
  // Other
  { path: '/about', name: 'about', component: () => import('../views/AboutView.vue') },
  { path: '/license', name: 'license', component: () => import('../views/LicenseView.vue') },
]

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
})
