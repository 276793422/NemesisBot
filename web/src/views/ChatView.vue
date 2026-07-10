<script setup lang="ts">
import { onMounted } from 'vue'
import ChatPanel from '../components/ChatPanel.vue'
import SessionSidebar from '../components/SessionSidebar.vue'
import { useSessionStore } from '../stores/session'

const sessionStore = useSessionStore()

// On entering the chat page, load the session list + auto-select a sensible
// default (legacy "历史对话" if present, else the most recent) so the user
// sees a real conversation immediately — without having to open the sidebar.
// This MUST live here (ChatView is always mounted), NOT in SessionSidebar,
// because the sidebar is v-if'd on showSidebar — its onMounted only fires
// when the user opens it, which is too late for the initial auto-select.
onMounted(async () => {
  await sessionStore.fetchList()
  if (!sessionStore.currentId) {
    const legacy = sessionStore.sessions.find(s => s.id === 'legacy')
    const target = legacy ? legacy.id : (sessionStore.sessions[0]?.id ?? '')
    if (target) sessionStore.switchTo(target)
  }
})
</script>

<template>
  <div class="chat-page-layout">
    <SessionSidebar v-if="sessionStore.showSidebar" />
    <ChatPanel />
  </div>
</template>

<style scoped>
.chat-page-layout {
  display: flex;
  height: 100%;
  min-height: 0;
  overflow: hidden;
}
.chat-page-layout > :deep(.page-chat) {
  flex: 1;
  min-width: 0;
  min-height: 0;
  /* In the two-column layout, .page-chat is NOT a direct child of
     .main-content, so it misses layout.css `.main-content > [class^="page-"]`
     (which gives flex column + height:100%). Restore it here, otherwise
     .chat-messages' flex:1 collapses and its overflow scrollbar is lost. */
  display: flex;
  flex-direction: column;
  height: 100%;
}
</style>
