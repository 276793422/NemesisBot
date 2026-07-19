<script setup lang="ts">
import { useRouter, useRoute } from 'vue-router'
import { computed, ref, onMounted, onUnmounted } from 'vue'
import { useAppStore } from '../stores/app'
import { useAuthStore } from '../stores/auth'
import { useTheme } from '../composables/useTheme'
import { useWSAPI } from '../composables/useWSAPI'
import { useUiShellStore } from '../stores/uiShell'
import { useToast } from '../composables/useToast'
import { CLASSIC_NAV_GROUPS, filterNavGroups, type NavItem } from '../lib/navConfig'
import ColorPicker from './ColorPicker.vue'

const router = useRouter()
const route = useRoute()
const appStore = useAppStore()
const auth = useAuthStore()
const uiShell = useUiShellStore()
const toast = useToast()
const { theme, toggleTheme } = useTheme()
const { request } = useWSAPI()

const estopEngaged = ref(false)
const estopBusy = ref(false)
let estopTimer: ReturnType<typeof setInterval> | undefined

function navigate(item: NavItem) {
  router.push(item.id === 'chat' ? '/' : item.path)
  appStore.showMobileSidebar = false
}

function handleLogout() {
  auth.logout()
}

async function refreshEstop() {
  try {
    const resp = await request('estop', 'status', {}, 5000)
    estopEngaged.value = !!(resp && resp.engaged)
  } catch { /* keep */ }
}

async function toggleEstop() {
  if (estopBusy.value) return
  if (!estopEngaged.value) {
    if (!confirm('确定触发急停？将冻结全部 Agent 活动。')) return
  }
  estopBusy.value = true
  try {
    const cmd = estopEngaged.value ? 'release' : 'trigger'
    const resp = await request('estop', cmd, {}, 5000)
    estopEngaged.value = !!(resp && resp.engaged)
    toast.success(estopEngaged.value ? '已急停' : '已释放急停')
  } catch (e) {
    toast.error('急停操作失败: ' + e)
  } finally {
    estopBusy.value = false
  }
}

function switchToFriendly() {
  uiShell.setMode('friendly')
  toast.success('已切换到简洁界面')
}

const visibleNavGroups = computed(() => filterNavGroups(CLASSIC_NAV_GROUPS, { includeUnfinished: false }))

onMounted(() => {
  refreshEstop()
  estopTimer = setInterval(refreshEstop, 10000)
})
onUnmounted(() => {
  if (estopTimer) clearInterval(estopTimer)
})
</script>

<template>
  <aside
    class="sidebar"
    :class="{ collapsed: appStore.sidebarCollapsed, 'mobile-open': appStore.showMobileSidebar }"
    data-shell="classic"
  >
    <div class="sidebar-header">
      <div class="sidebar-logo">
        <svg class="sidebar-logo-icon" width="28" height="28" viewBox="0 0 256 256" fill="none" xmlns="http://www.w3.org/2000/svg">
          <g transform="translate(8, 18)">
            <line x1="120" y1="30" x2="120" y2="5" stroke="var(--accent)" stroke-width="4" stroke-linecap="round"/>
            <circle cx="120" cy="5" r="6" fill="var(--accent)"/>
            <rect x="68" y="30" width="104" height="80" rx="15" fill="var(--accent)" stroke="var(--accent)" stroke-width="4" fill-opacity="0.15"/>
            <circle cx="95" cy="60" r="9" fill="var(--accent)"/>
            <circle cx="145" cy="60" r="9" fill="var(--accent)"/>
            <path d="M 105 85 Q 120 100 135 85" stroke="var(--accent)" stroke-width="4" fill="transparent" stroke-linecap="round"/>
          </g>
        </svg>
        <h1>NemesisBot</h1>
      </div>
    </div>

    <div class="sidebar-status">
      <span class="status-dot" :class="appStore.connected ? 'connected' : 'disconnected'"></span>
      <span>{{ appStore.connected ? '已连接' : '未连接' }}</span>
    </div>

    <nav class="sidebar-nav" aria-label="导航">
      <div v-for="group in visibleNavGroups" :key="group.title" class="nav-section">
        <div class="nav-section-title">{{ group.title }}</div>
        <button
          v-for="item in group.items"
          :key="item.id"
          type="button"
          class="nav-item"
          :class="{ active: route.path === item.path }"
          :title="item.label"
          @click="navigate(item)"
        >
          <span class="nav-icon">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path :d="item.icon"/></svg>
          </span>
          <span class="nav-label">{{ item.label }}</span>
        </button>
      </div>
    </nav>

    <div class="sidebar-footer">
      <button type="button" class="nav-item estop-btn" :class="{ engaged: estopEngaged }" :title="estopEngaged ? '急停中——点击释放' : '触发急停'" @click="toggleEstop()">
        <span class="nav-icon">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><line x1="9" y1="9" x2="15" y2="15"/><line x1="15" y1="9" x2="9" y2="15"/></svg>
        </span>
        <span class="nav-label">{{ estopBusy ? '处理中…' : (estopEngaged ? '⛔ 急停中（点此释放）' : '急停 E-Stop') }}</span>
      </button>
      <button type="button" class="nav-item" title="切换到简洁界面" @click="switchToFriendly">
        <span class="nav-icon">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 3l1.5 4.5L18 9l-4.5 1.5L12 15l-1.5-4.5L6 9l4.5-1.5L12 3z"/></svg>
        </span>
        <span class="nav-label">简洁界面</span>
      </button>
      <button type="button" class="nav-item" :title="theme === 'dark' ? '浅色模式' : '深色模式'" @click="toggleTheme()">
        <span class="nav-icon">
          <svg v-if="theme === 'dark'" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
          <svg v-else width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>
        </span>
        <span class="nav-label">{{ theme === 'dark' ? '浅色模式' : '深色模式' }}</span>
      </button>
      <div class="nav-item color-picker-row">
        <span class="nav-icon">
          <ColorPicker />
        </span>
        <span class="nav-label">配色</span>
      </div>
      <button type="button" class="nav-item" title="退出" @click="handleLogout()">
        <span class="nav-icon">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/></svg>
        </span>
        <span class="nav-label">退出</span>
      </button>
    </div>

    <div class="sidebar-toggle" :title="appStore.sidebarCollapsed ? '展开侧栏' : '收起侧栏'" @click="appStore.toggleSidebar()">
      <span>{{ appStore.sidebarCollapsed ? '\u00BB' : '\u00AB' }}</span>
    </div>
  </aside>
</template>

<style scoped>
.nav-item {
  width: calc(100% - 16px);
  border: none;
  background: transparent;
  text-align: left;
  font: inherit;
  cursor: pointer;
}

.estop-btn.engaged {
  color: var(--error);
  background: var(--error-bg);
  font-weight: 600;
}

.estop-btn.engaged .nav-icon {
  animation: estop-pulse 1.4s ease-in-out infinite;
}

@keyframes estop-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}

.color-picker-row {
  display: flex;
  align-items: center;
  cursor: default;
}

.color-picker-row .nav-icon {
  display: flex;
  align-items: center;
}
</style>
