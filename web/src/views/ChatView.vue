<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import ChatPanel from '../components/ChatPanel.vue'
import SessionSidebar from '../components/SessionSidebar.vue'
import FileTreePanel from '../components/chat/FileTreePanel.vue'
import BrowserPickPanel from '../components/chat/BrowserPickPanel.vue'
import SessionResourcesPanel from '../components/chat/SessionResourcesPanel.vue'
import { useSessionStore } from '../stores/session'
import { skinHasSlot } from '../composables/useSkin'

const sessionStore = useSessionStore()

// 浏览器点名面板（2026-11-15，计划 D10）：feature gate（`!== 'false'` =
// 默认包含，同 router 门控惯例）；关 = 面板不挂载（侧栏按钮同闸）。
const browserEnabled = import.meta.env.VITE_FEATURE_BROWSER !== 'false'

// ---- 会话资源盘点看板（2026-11-17，B3 壳层）----
//
// 宽屏（容器 ≥ 1284px）右栏挂 SessionResourcesPanel，同时把 ChatPanel
// （.page-chat）压到 max-width:944px；窄屏自动收起。开合偏好存
// localStorage（全局统一键，不分会话）。收起态 = 右缘细竖条按钮
//（组件 v-if 卸载 = 订阅/轮询全停）；首次（无偏好记录）默认展开。
const resourcesOpen = ref(localStorage.getItem('nb_resources_panel') !== '0')
const viewportWide = ref(true)
let resizeObserver: ResizeObserver | null = null
const layoutEl = ref<HTMLElement | null>(null)

// 阈值 = ChatPanel 上限 944（含滚动条余量）+ 面板最小 340。
const RESOURCES_LAYOUT_MIN = 1284

function applyViewport(): void {
  const w = layoutEl.value?.clientWidth ?? 0
  const wide = w >= RESOURCES_LAYOUT_MIN
  viewportWide.value = wide
  if (!wide && resourcesOpen.value) resourcesOpen.value = false
}

// 窄屏强收后转宽屏不自动重开（用户意志优先；竖条按钮常在可手动开）。
function toggleResources(): void {
  resourcesOpen.value = !resourcesOpen.value
  localStorage.setItem('nb_resources_panel', resourcesOpen.value ? '1' : '0')
}

// On entering the chat page, load the session list + auto-select a sensible
// default (legacy "历史对话" if present, else the most recent) so the user
// sees a real conversation immediately — without having to open the sidebar.
// This MUST live here (ChatView is always mounted), NOT in SessionSidebar,
// because the sidebar is v-if'd on showSidebar — its onMounted only fires
// when the user opens it, which is too late for the initial auto-select.
onMounted(async () => {
  // B3: 判宽先行（窄屏强收在会话列表 await 之前生效，防面板闪一帧）。
  if (typeof ResizeObserver !== 'undefined') {
    resizeObserver = new ResizeObserver(applyViewport)
    if (layoutEl.value) resizeObserver.observe(layoutEl.value)
  }
  applyViewport()
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

onUnmounted(() => {
  resizeObserver?.disconnect()
  resizeObserver = null
})
</script>

<template>
  <div ref="layoutEl" class="chat-page-layout">
    <!-- M4: 工作区文件树（默认折叠成左缘细条；点击文件 @引用进输入框） -->
    <FileTreePanel />
    <!-- D10: 浏览器点名面板（对话中途点名工作 tab，不跳页；侧栏按钮开合） -->
    <BrowserPickPanel v-if="browserEnabled" />
    <!-- 皮肤 sidebar 槽（结构皮肤）自带会话历史（AppLayout 层），避免
         双侧栏；CSS-only 包 / 无皮肤回落原生 SessionSidebar -->
    <SessionSidebar v-if="sessionStore.showSidebar && !skinHasSlot('sidebar')" />
    <ChatPanel :class="{ 'chat-main-narrow': resourcesOpen }" />
    <!-- B3: 会话资源盘点看板（宽屏右栏；收起态右缘细竖条） -->
    <SessionResourcesPanel
      v-if="resourcesOpen"
      :session-id="sessionStore.currentId || null"
      class="chat-resources-panel"
    />
    <button
      v-if="!resourcesOpen && viewportWide"
      class="chat-resources-rail"
      type="button"
      title="打开资源看板"
      @click="toggleResources"
    >‹ 资源</button>
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
  /* B3: 资源看板在场时（.chat-main-narrow 动态挂上）把聊天主列压到 944
     （上限含滚动条余量），保对话行宽不随窗口无限拉宽；看板收起 = 布局
     与旧版一致（撑满）。margin-left:auto 让对话列靠右贴看板。 */
  max-width: none;
  margin-left: 0;
}
.chat-page-layout > :deep(.chat-main-narrow) {
  max-width: 944px;
  margin-left: auto;
}
.chat-resources-panel {
  flex: 1 1 340px;
  min-width: 300px;
  max-width: 480px;
  min-height: 0;
  border-left: 1px solid var(--border);
}
.chat-resources-rail {
  writing-mode: vertical-rl;
  align-self: center;
  border: 1px solid var(--border);
  border-right: none;
  border-radius: 8px 0 0 8px;
  background: var(--bg-elev);
  color: var(--text-muted);
  padding: 12px 3px;
  cursor: pointer;
  font-size: 11px;
  letter-spacing: 2px;
}
.chat-resources-rail:hover { color: var(--accent); border-color: var(--accent); }
</style>
