<script setup lang="ts">
import { onMounted, onUnmounted } from 'vue'
import { useAuthStore } from './stores/auth'
import { useAppStore } from './stores/app'
import AuthOverlay from './components/AuthOverlay.vue'
import AppLayout from './components/AppLayout.vue'
// M6（devtool-upgrade 阶段 7）：Ctrl+K 命令面板（三源合一）。
import CommandPalette from './components/CommandPalette.vue'
import { useCommandPalette } from './composables/useCommandPalette'

const auth = useAuthStore()
const appStore = useAppStore()
const palette = useCommandPalette()

function handleKeydown(e: KeyboardEvent) {
  if (e.ctrlKey && e.key === 'b') {
    e.preventDefault()
    appStore.toggleSidebar()
  }
  // M6：Ctrl/Cmd+K 命令面板（preventDefault 拦浏览器地址栏聚焦；metaKey
  // 覆盖 macOS ⌘K）。认证覆盖层打开时无意义，可见性由 AppLayout 挂载点
  // 决定（AuthOverlay 分支不渲染面板）。
  if ((e.ctrlKey || e.metaKey) && (e.key === 'k' || e.key === 'K')) {
    e.preventDefault()
    palette.toggle()
  }
}

onMounted(async () => {
  document.addEventListener('keydown', handleKeydown)

  // Dev preview mode: URL 带 ?preview=1 直接绕过认证（仅 dev server 有效，生产构建不会触发）
  const isPreview = new URLSearchParams(location.search).has('preview')
  if (isPreview) {
    auth.authenticated = true
    return
  }

  // Auto-login: try various token sources
  let tokenFromURL = ''

  // URL fragment token
  if (location.hash.includes('__dashboard_token=')) {
    const match = location.hash.match(/__dashboard_token=([^&#]+)/)
    if (match) {
      tokenFromURL = decodeURIComponent(match[1])
      history.replaceState(null, '', location.pathname + location.search)
    }
  }

  if (tokenFromURL) {
    await auth.autoLogin(tokenFromURL)
  } else if (window.__DASHBOARD_TOKEN__) {
    await auth.autoLogin(window.__DASHBOARD_TOKEN__)
  } else if (window.runtime && window.runtime.EventsOn) {
    window.runtime.EventsOn('dashboard-token', async (token: string) => {
      if (token && !auth.authenticated) {
        await auth.autoLogin(token)
      }
    })
  } else {
    const token = localStorage.getItem('nemesisbot_auth_token')
    if (token) {
      await auth.autoLogin(token)
    }
  }
})

onUnmounted(() => {
  document.removeEventListener('keydown', handleKeydown)
})
</script>

<template>
  <AuthOverlay v-if="!auth.authenticated" />
  <AppLayout v-else />
  <!-- M6：Teleport to body，挂哪都行；认证后才有意义，跟随 AppLayout 分支。 -->
  <CommandPalette v-if="auth.authenticated" />
</template>
