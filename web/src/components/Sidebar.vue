<script setup lang="ts">
import { useRouter, useRoute } from 'vue-router'
import { useAppStore } from '../stores/app'
import { useAuthStore } from '../stores/auth'
import { useTheme } from '../composables/useTheme'

const router = useRouter()
const route = useRoute()
const appStore = useAppStore()
const auth = useAuthStore()
const { theme, toggleTheme } = useTheme()

function navigate(page: string) {
  router.push(page === 'chat' ? '/' : '/' + page)
  appStore.showMobileSidebar = false
}

function handleLogout() {
  auth.logout()
}

const navGroups = [
  {
    title: '主要',
    items: [
      { id: 'chat', label: '聊天', path: '/', icon: 'M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z' },
      { id: 'overview', label: '概览', path: '/overview', icon: 'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z' },
      { id: 'usage', label: '使用统计', path: '/usage', icon: 'M18 20V10M12 20V4M6 20v-6' },
      { id: 'persona', label: '人格', path: '/persona', icon: 'M12 12c2.21 0 4-1.79 4-4s-1.79-4-4-4-4 1.79-4 4 1.79 4 4 4zm0 2c-2.67 0-8 1.34-8 4v2h16v-2c0-2.66-5.33-4-8-4z' },
    ],
  },
  {
    title: '管理',
    items: [
      { id: 'logs', label: '日志', path: '/logs', icon: 'M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z M14 2v6h6 M16 13H8 M16 17H8' },
      { id: 'models', label: '模型', path: '/models', icon: 'M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5' },
      { id: 'memory', label: '记忆', path: '/memory', icon: 'M12 2a7 7 0 0 1 7 7c0 5.25-7 13-7 13S5 14.25 5 9a7 7 0 0 1 7-7z M12 9a1 1 0 1 0 0-2 1 1 0 0 0 0 2z' },
      { id: 'persona-shop', label: '人格超市', path: '/persona-shop', icon: 'M3 3h18v18H3V3zm3 3h12v3H6V6zm0 5h12v3H6v-3zm0 5h8v3H6v-3z' },
      { id: 'skills', label: 'Skills', path: '/skills', icon: 'M9.663 17h4.673M12 3v1m6.364 1.636l-.707.707M21 12h-1M4 12H3m3.343-5.657l-.707-.707m2.828 9.9a5 5 0 1 1 7.072 0l-.548.547A3.374 3.374 0 0 0 14 18.469V19a2 2 0 1 1-4 0v-.531c0-.895-.356-1.754-.988-2.386l-.548-.547z' },
      { id: 'mcp', label: 'MCP', path: '/mcp', icon: 'M4 6h16M4 12h16M4 18h16' },
      { id: 'channels', label: '通道', path: '/channels', icon: 'M22 12h-4l-3 9L9 3l-3 9H2' },
      { id: 'workflows', label: '工作流', path: '/workflows', icon: 'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7zM10 6.5h4M10 17.5h4M6.5 10v4M17.5 10v4' },
    ],
  },
  {
    title: '自进化',
    items: [
      { id: 'forge', label: 'Forge', path: '/forge', icon: 'M13 10V3L4 14h7v7l9-11h-7z' },
    ],
  },
  {
    title: '配置',
    items: [
      { id: 'settings', label: '设置', path: '/settings', icon: 'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-2.82 1.18V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1.08-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0-1.18-2.82H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1.08 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 2.82-1.18V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1.08 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0 1.18 2.82H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1.08z' },
      { id: 'tools', label: 'Tools', path: '/tools', icon: 'M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z' },
      { id: 'tasks', label: '任务', path: '/tasks', icon: 'M9 11l3 3L22 4 M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11' },
      { id: 'cluster', label: '集群', path: '/cluster', icon: 'M6 3v18 M18 3v18 M3 6h18 M3 18h18 M3 12h18' },
      { id: 'security', label: '安全', path: '/security', icon: 'M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z' },
      { id: 'scanner', label: '扫描器', path: '/scanner', icon: 'M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z' },
      { id: 'local-models', label: '本地模型', path: '/local-models', icon: 'M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z M3.27 6.96L12 12.01l8.73-5.05 M12 22.08V12' },
    ],
  },
  {
    title: '其他',
    items: [
      { id: 'about', label: '关于', path: '/about', icon: 'M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10zM12 8v4M12 16h.01' },
      { id: 'license', label: 'License', path: '/license', icon: 'M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z M14 2v6h6 M16 13H8 M16 17H8 M10 9H8' },
    ],
  },
]
</script>

<template>
  <aside class="sidebar" :class="{ collapsed: appStore.sidebarCollapsed, 'mobile-open': appStore.showMobileSidebar }">
    <div class="sidebar-header">
      <div class="sidebar-logo">
        <svg class="sidebar-logo-icon" width="20" height="20" viewBox="0 0 256 256" fill="none" xmlns="http://www.w3.org/2000/svg">
          <g transform="translate(8, 18)">
            <line x1="120" y1="30" x2="120" y2="5" stroke="#2C3E50" stroke-width="4" stroke-linecap="round"/>
            <circle cx="120" cy="5" r="6" fill="#FF4D4D"/>
            <circle cx="117" cy="3" r="2" fill="#FFF" opacity="0.6"/>
            <rect x="68" y="30" width="104" height="80" rx="15" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="53" y="55" width="15" height="30" rx="5" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="172" y="55" width="15" height="30" rx="5" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <circle cx="95" cy="60" r="9" fill="#2C3E50"/>
            <circle cx="145" cy="60" r="9" fill="#2C3E50"/>
            <circle cx="92" cy="57" r="3" fill="#FFF"/>
            <circle cx="142" cy="57" r="3" fill="#FFF"/>
            <circle cx="82" cy="80" r="5" fill="#FF6B6B" opacity="0.8"/>
            <circle cx="158" cy="80" r="5" fill="#FF6B6B" opacity="0.8"/>
            <path d="M 105 85 Q 120 100 135 85" stroke="#2C3E50" stroke-width="4" fill="transparent" stroke-linecap="round"/>
            <rect x="78" y="120" width="84" height="65" rx="12" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <circle cx="100" cy="145" r="5" fill="#FF8C42"/>
            <circle cx="120" cy="145" r="5" fill="#2ECC71"/>
            <circle cx="140" cy="145" r="5" fill="#FF6B6B"/>
            <rect x="45" y="135" width="33" height="16" rx="8" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="162" y="135" width="33" height="16" rx="8" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="92" y="185" width="14" height="22" rx="4" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="78" y="200" width="35" height="15" rx="7" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="134" y="185" width="14" height="22" rx="4" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
            <rect x="127" y="200" width="35" height="15" rx="7" fill="#4A90E2" stroke="#2C3E50" stroke-width="4"/>
          </g>
        </svg>
        <h1>NemesisBot</h1>
      </div>
    </div>

    <div class="sidebar-status">
      <span class="status-dot" :class="appStore.connected ? 'connected' : 'disconnected'"></span>
      <span>{{ appStore.connected ? '已连接' : '未连接' }}</span>
    </div>

    <nav class="sidebar-nav">
      <div v-for="group in navGroups" :key="group.title" class="nav-section">
        <div class="nav-section-title">{{ group.title }}</div>
        <a
          v-for="item in group.items"
          :key="item.id"
          class="nav-item"
          :class="{ active: route.path === item.path }"
          @click="navigate(item.id)"
        >
          <span class="nav-icon">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path :d="item.icon"/></svg>
          </span>
          <span class="nav-label">{{ item.label }}</span>
        </a>
      </div>
    </nav>

    <div class="sidebar-footer">
      <a class="nav-item" @click="toggleTheme()">
        <span class="nav-icon">
          <svg v-if="theme === 'dark'" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
          <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>
        </span>
        <span class="nav-label">{{ theme === 'dark' ? '浅色模式' : '深色模式' }}</span>
      </a>
      <a class="nav-item" @click="handleLogout()">
        <span class="nav-icon">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/></svg>
        </span>
        <span class="nav-label">退出</span>
      </a>
    </div>

    <div class="sidebar-toggle" @click="appStore.toggleSidebar()">
      <span>{{ appStore.sidebarCollapsed ? '\u00BB' : '\u00AB' }}</span>
    </div>
  </aside>
</template>
