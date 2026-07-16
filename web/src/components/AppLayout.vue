<script setup lang="ts">
import { useAppStore } from '../stores/app'
import { useUiShellStore } from '../stores/uiShell'
import Sidebar from './Sidebar.vue'
import FriendlySidebar from './FriendlySidebar.vue'
import ToastContainer from './ToastContainer.vue'

const appStore = useAppStore()
const uiShell = useUiShellStore()
</script>

<template>
  <div
    class="app-layout"
    :class="{
      'focus-mode': appStore.focusMode,
      'shell-friendly': uiShell.isFriendly,
      'shell-classic': uiShell.isClassic,
    }"
    :data-ui-shell="uiShell.mode"
  >
    <div class="mobile-overlay" :class="{ show: appStore.showMobileSidebar }" @click="appStore.toggleMobileSidebar()"></div>

    <FriendlySidebar v-if="uiShell.isFriendly" />
    <Sidebar v-else />

    <main class="main-content">
      <div class="mobile-header">
        <button class="hamburger-btn" type="button" aria-label="打开菜单" @click="appStore.toggleMobileSidebar()">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="3" y1="6" x2="21" y2="6"/><line x1="3" y1="12" x2="21" y2="12"/><line x1="3" y1="18" x2="21" y2="18"/></svg>
        </button>
        <span class="mobile-title">NemesisBot</span>
      </div>

      <router-view />
    </main>

    <ToastContainer />
  </div>
</template>

<style scoped>
.mobile-title {
  font-weight: 600;
}
</style>
