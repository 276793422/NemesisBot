<script setup lang="ts">
import { onMounted } from 'vue'
import ChatPanel from '../components/ChatPanel.vue'
import SessionSidebar from '../components/SessionSidebar.vue'
import FileTreePanel from '../components/chat/FileTreePanel.vue'
import { useSessionStore } from '../stores/session'
import { skinState } from '../composables/useSkin'

const sessionStore = useSessionStore()

// On entering the chat page, load the session list + auto-select a sensible
// default (legacy "历史对话" if present, else the most recent) so the user
// sees a real conversation immediately — without having to open the sidebar.
// This MUST live here (ChatView is always mounted), NOT in SessionSidebar,
// because the sidebar is v-if'd on showSidebar — its onMounted only fires
// when the user opens it, which is too late for the initial auto-select.
onMounted(async () => {
  // P5（2026-09-21）：项目注册表提升到 ChatView 初始化。此前 fetchProjects
  // 全工程唯一调用点在 SessionSidebar.onMounted，而侧栏默认收起（showSidebar
  // =false → v-if 不挂载）→ 注册表恒空 → ChatPanel 的项目 chip（⟦项目名⟧）
  // 永不显示。fetchProjects 自带 5s 缓存与静默容错（store 内 try/catch），
  // 侧栏之后挂载时重复调用零开销。不 await：不阻塞会话列表主链。
  void sessionStore.fetchProjects()
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
    <!-- M4: 工作区文件树（默认折叠成左缘细条；点击文件 @引用进输入框） -->
    <FileTreePanel />
    <!-- 皮肤激活时会话列表由 SkinSidebar（AppLayout 层）承担，避免双侧栏 -->
    <SessionSidebar v-if="sessionStore.showSidebar && !skinState.id" />
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
