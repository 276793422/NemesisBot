<script setup lang="ts">
import { useAppStore } from '../stores/app'
import Sidebar from './Sidebar.vue'
import ToastContainer from './ToastContainer.vue'

const appStore = useAppStore()
</script>

<template>
  <div class="app-layout" :class="{ 'focus-mode': appStore.focusMode }">
    <!-- Mobile Overlay -->
    <div class="mobile-overlay" :class="{ show: appStore.showMobileSidebar }" @click="appStore.toggleMobileSidebar()"></div>

    <Sidebar />

    <main class="main-content">
      <!-- Mobile Header -->
      <div class="mobile-header">
        <button class="hamburger-btn" @click="appStore.toggleMobileSidebar()">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="3" y1="6" x2="21" y2="6"/><line x1="3" y1="12" x2="21" y2="12"/><line x1="3" y1="18" x2="21" y2="18"/></svg>
        </button>
        <span style="font-weight:600;">NemesisBot</span>
      </div>

      <router-view />
    </main>

    <ToastContainer />
  </div>
</template>
