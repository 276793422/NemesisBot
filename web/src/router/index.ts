import { createRouter, createWebHashHistory, type RouteRecordRaw } from 'vue-router'
import { MERGED_ROUTE_REDIRECTS } from '../lib/navConfig'

/**
 * Top-level routes are hubs only. Former standalone pages redirect into hub tabs.
 */
const routes: RouteRecordRaw[] = [
  { path: '/', name: 'chat', component: () => import('../views/ChatView.vue') },
  { path: '/overview', name: 'overview', component: () => import('../views/OverviewView.vue') },
  { path: '/persona', name: 'persona', component: () => import('../views/PersonaView.vue') },
  { path: '/models', name: 'models', component: () => import('../views/ModelsView.vue') },
  /** 能力 = 技能 + MCP + 通道 + 工作流 */
  { path: '/skills', name: 'skills', component: () => import('../views/SkillsView.vue') },
  /** 高级 = 集群 + Forge */
  ...(import.meta.env.VITE_FEATURE_CLUSTER !== 'false' || import.meta.env.VITE_FEATURE_FORGE !== 'false'
    ? [{ path: '/cluster', name: 'cluster', component: () => import('../views/ClusterView.vue') }]
    : []),
  { path: '/settings', name: 'settings', component: () => import('../views/SettingsView.vue') },
  ...(import.meta.env.VITE_FEATURE_SECURITY !== 'false'
    ? [{ path: '/security', name: 'security', component: () => import('../views/SecurityView.vue') }]
    : []),
  { path: '/about', name: 'about', component: () => import('../views/AboutView.vue') },

  ...MERGED_ROUTE_REDIRECTS.map(({ from, to }) => ({
    path: from,
    redirect: to,
  })),
]

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
})
