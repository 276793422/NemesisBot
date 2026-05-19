import { createRouter, createWebHashHistory, type RouteRecordRaw } from 'vue-router'

const routes: RouteRecordRaw[] = [
  {
    path: '/',
    name: 'chat',
    component: () => import('../views/ChatView.vue'),
  },
  {
    path: '/overview',
    name: 'overview',
    component: () => import('../views/OverviewView.vue'),
  },
  {
    path: '/logs',
    name: 'logs',
    component: () => import('../views/LogsView.vue'),
  },
  {
    path: '/scanner',
    name: 'scanner',
    component: () => import('../views/ScannerView.vue'),
  },
  {
    path: '/settings',
    name: 'settings',
    component: () => import('../views/SettingsView.vue'),
  },
]

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
})
