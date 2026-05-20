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
</script>

<template>
  <aside class="sidebar" :class="{ collapsed: appStore.sidebarCollapsed, 'mobile-open': appStore.showMobileSidebar }">
    <!-- Header -->
    <div class="sidebar-header">
      <div class="sidebar-logo">
        <h1>NemesisBot</h1>
      </div>
    </div>

    <!-- Connection Status -->
    <div class="sidebar-status">
      <span class="status-dot" :class="appStore.connected ? 'connected' : 'disconnected'"></span>
      <span>{{ appStore.connected ? '已连接' : '未连接' }}</span>
    </div>

    <!-- Navigation -->
    <nav class="sidebar-nav">
      <div class="nav-section">
        <div class="nav-section-title">主要</div>
        <a class="nav-item" :class="{ active: route.path === '/' || route.path === '/chat' }" @click="navigate('chat')">
          <span class="nav-icon">
            <svg width="20" height="20" viewBox="0 0 24 24"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
          </span>
          <span class="nav-label">聊天</span>
        </a>
        <a class="nav-item" :class="{ active: route.path === '/overview' }" @click="navigate('overview')">
          <span class="nav-icon">
            <svg width="20" height="20" viewBox="0 0 24 24"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/></svg>
          </span>
          <span class="nav-label">概览</span>
        </a>
      </div>

      <div class="nav-section">
        <div class="nav-section-title">管理</div>
        <a class="nav-item" :class="{ active: route.path === '/logs' }" @click="navigate('logs')">
          <span class="nav-icon">
            <svg width="20" height="20" viewBox="0 0 24 24"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/></svg>
          </span>
          <span class="nav-label">日志</span>
        </a>
        <a class="nav-item" :class="{ active: route.path === '/scanner' }" @click="navigate('scanner')">
          <span class="nav-icon">
            <svg width="20" height="20" viewBox="0 0 24 24"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/></svg>
          </span>
          <span class="nav-label">扫描器</span>
        </a>
        <a class="nav-item" :class="{ active: route.path === '/settings' }" @click="navigate('settings')">
          <span class="nav-icon">
            <svg width="20" height="20" viewBox="0 0 24 24"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
          </span>
          <span class="nav-label">设置</span>
        </a>
      </div>
    </nav>

    <!-- Footer -->
    <div class="sidebar-footer">
      <a class="nav-item" @click="toggleTheme()">
        <span class="nav-icon">
          <svg v-if="theme === 'dark'" width="20" height="20" viewBox="0 0 24 24"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
          <svg v-else width="20" height="20" viewBox="0 0 24 24"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>
        </span>
        <span class="nav-label">{{ theme === 'dark' ? '浅色模式' : '深色模式' }}</span>
      </a>
      <a class="nav-item" @click="handleLogout()">
        <span class="nav-icon">
          <svg width="20" height="20" viewBox="0 0 24 24"><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/></svg>
        </span>
        <span class="nav-label">退出</span>
      </a>
    </div>

    <!-- Toggle -->
    <div class="sidebar-toggle" @click="appStore.toggleSidebar()">
      <span>{{ appStore.sidebarCollapsed ? '\u00BB' : '\u00AB' }}</span>
    </div>
  </aside>
</template>
