<script setup lang="ts">
import { onMounted } from 'vue'
import { useAppStore } from '../stores/app'
import Sidebar from './Sidebar.vue'
import ToastContainer from './ToastContainer.vue'
import ApprovalCard from './ApprovalCard.vue'
import QuestionCard from './QuestionCard.vue'
import { useApprovals } from '../composables/useApprovals'
import { useQuestions } from '../composables/useQuestions'

const appStore = useAppStore()
// M7：审批卡单例订阅（SSE approval-requested + pending 补拉）。
const { initApprovals } = useApprovals()
// F7：提问卡单例订阅（SSE question-asked + pending 补拉）。
const { initQuestions } = useQuestions()
onMounted(() => {
  initApprovals()
  initQuestions()
})
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
    <!-- M7：安全审批卡（模态，任意页面可见） -->
    <ApprovalCard />
    <!-- F7：结构化提问卡（模态，任意页面可见） -->
    <QuestionCard />
  </div>
</template>
