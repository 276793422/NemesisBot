<script setup lang="ts">
import { computed, provide, onMounted, onUnmounted, ref } from 'vue'
import ChatPanel from '../components/ChatPanel.vue'
import SessionSidebar from '../components/SessionSidebar.vue'
import FileTreePanel from '../components/chat/FileTreePanel.vue'
import BrowserPickPanel from '../components/chat/BrowserPickPanel.vue'
import SessionResourcesPanel from '../components/chat/SessionResourcesPanel.vue'
import { RESOURCES_PANEL_CTL } from '../composables/useResourcesPanel'
import { useSessionStore } from '../stores/session'
import { skinHasSlot } from '../composables/useSkin'

const sessionStore = useSessionStore()

// 浏览器点名面板（2026-11-15，计划 D10）：feature gate（`!== 'false'` =
// 默认包含，同 router 门控惯例）；关 = 面板不挂载（侧栏按钮同闸）。
const browserEnabled = import.meta.env.VITE_FEATURE_BROWSER !== 'false'

// ---- 会话资源看板（2026-11-17，B3 壳层；2026-11-19 交互改造）----
//
// 两种形态（按布局宽度自动切换，同一份开合状态）：
// - 并排形态（布局 ≥ RESOURCES_OPEN_AT）：右栏并排，ChatPanel 靠左固定
//   944px，资源窗紧贴其右吃掉剩余宽度（2026-11-18 用户拍板：对话列恒在
//   最左、宽度确定）；
// - overlay 抽屉形态（布局 < RESOURCES_OPEN_AT）：资源窗悬浮覆盖右侧 +
//   遮罩，不挤压对话列——任何宽度都能打开（2026-11-19 用户拍板，取代旧
//   「窄屏强制收起」：窄窗口点开不再失败，只是换形态）。
//
// 自动开（2026-11-19 用户提案 A）：布局从窄到宽穿越 OPEN_AT 那一刻自动
// 打开一次（边缘触发 + 滞后带防拖动抖动）；本次页面生命周期内用户手动
// 收起过则不再自动开（意志优先；手动开一次即恢复资格）。窄方向不强制
// 收——overlay 不挤压对话列，开着不碍事。
// 开合偏好存 localStorage（全局统一键，不分会话）；首次（无偏好）默认展开。
const resourcesOpen = ref(localStorage.getItem('nb_resources_panel') !== '0')
const viewportWide = ref(true)
/** overlay 抽屉形态判定：布局不足以并排时，打开的资源窗走悬浮形态。 */
const overlayMode = computed(() => !viewportWide.value)
/** 用户手动收起后置 true（本次生命周期内抑制自动开；手动开复位）。 */
let userClosedManually = localStorage.getItem('nb_resources_panel') === '0'
let resizeObserver: ResizeObserver | null = null
const layoutEl = ref<HTMLElement | null>(null)

// 并排下限 = ChatPanel 上限 944（含滚动条余量）+ 面板最小 340；
// 滞后带下缘低于它一截——拖动穿越阈值时不会在临界点反复翻转。
const RESOURCES_OPEN_AT = 1284
const RESOURCES_CLOSE_AT = 1180

/** 首屏判宽哨兵：首次 applyViewport 前为 true（窄窗口静默收起只此一次）。 */
let firstViewportCheck = true

function applyViewport(): void {
  const w = layoutEl.value?.clientWidth ?? 0
  const prev = viewportWide.value
  // 滞后带：≥OPEN_AT 判宽、<CLOSE_AT 判窄、带间保持原值。
  const wide = w >= RESOURCES_OPEN_AT ? true : w < RESOURCES_CLOSE_AT ? false : prev
  viewportWide.value = wide
  // 首屏：窄窗口 + 展开态 → 静默收起（不写偏好、不算手动关）——抽屉不该
  // 在首屏盖住对话列；转宽时边缘触发自动开会把它带回来。
  if (firstViewportCheck) {
    firstViewportCheck = false
    if (!wide && resourcesOpen.value && !userClosedManually) resourcesOpen.value = false
  }
  // 边缘触发自动开：仅窄→宽穿越那一刻（带间移动 prev===wide 不触发）。
  if (wide && !prev && !resourcesOpen.value && !userClosedManually) {
    setResourcesOpen(true)
  }
}

function setResourcesOpen(v: boolean): void {
  resourcesOpen.value = v
  localStorage.setItem('nb_resources_panel', v ? '1' : '0')
  userClosedManually = !v
}

function toggleResources(): void {
  setResourcesOpen(!resourcesOpen.value)
}

// 提案 C（2026-11-19，用户原始提案）：输入框右侧【收起工具栏】按钮已改为
// 【打开/收起资源窗】——ChatPanel 经注入拿到这份控制（无 ChatView 的挂载
// 面 inject 缺省 null，按钮不渲染）。
provide(RESOURCES_PANEL_CTL, {
  open: computed(() => resourcesOpen.value),
  toggle: toggleResources,
})

// On entering the chat page, load the session list + auto-select a sensible
// default (legacy "历史对话" if present, else the most recent) so the user
// sees a real conversation immediately — without having to open the sidebar.
// This MUST live here (ChatView is always mounted), NOT in SessionSidebar,
// because the sidebar is v-if'd on showSidebar — its onMounted only fires
// when the user opens it, which is too late for the initial auto-select.
onMounted(async () => {
  // 判宽先行（overlay/并排形态判定在会话列表 await 之前生效，防面板闪一帧）。
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
    <ChatPanel :class="{ 'chat-main-narrow': resourcesOpen && !overlayMode }" />
    <!-- B3: 会话资源看板。并排 = 右栏；overlay = 悬浮抽屉 + 遮罩（遮罩
         点击收起；抽屉头部 ✕ 同款，经 @close 上抛）。 -->
    <div
      v-if="resourcesOpen && overlayMode"
      class="chat-resources-backdrop"
      @click="setResourcesOpen(false)"
    />
    <SessionResourcesPanel
      v-if="resourcesOpen"
      :session-id="sessionStore.currentId || null"
      :overlay="overlayMode"
      class="chat-resources-panel"
      :class="{ 'chat-resources-overlay': overlayMode }"
      @close="setResourcesOpen(false)"
    />
    <!-- #6 rail 竖条入口已删（2026-11-19）：收起后重开走 ChatPanel 输入
         行右侧资源窗开关（resourcesCtl 注入面，任何宽度可用）；两处入口
         语义重复且竖条与 overlay 形态并存时视觉冗余。 -->
  </div>
</template>

<style scoped>
.chat-page-layout {
  display: flex;
  position: relative;
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
  /* B3: 资源看板并排在场时（.chat-main-narrow 动态挂上）对话列固定 944
     靠左（上限含滚动条余量）；overlay 形态不挂——对话列撑满，抽屉悬浮。
     看板收起 = 布局与旧版一致（撑满）。 */
  max-width: none;
  margin-left: 0;
}
.chat-page-layout > :deep(.chat-main-narrow) {
  /* 固定 944 靠左（2026-11-18 用户拍板：flex-grow 分自由空间会把宽度变成
     随机数，flex:0 0 定死才「确定」；margin-left:auto 把对话列推到中间——
     旧版缺陷形态，勿恢复）。 */
  flex: 0 0 944px;
  max-width: 944px;
  margin-left: 0;
}
.chat-resources-panel {
  /* 并排形态：紧贴对话列右侧吃掉剩余宽度（不设 max-width——设了上限宽
     窗口右缘会留死空间；2026-11-18 用户拍板：资源窗 = 对话列右边的侧窗）。
     overlay 形态由 .chat-resources-overlay 接管（绝对定位脱离 flex 流）。 */
  flex: 1 1 340px;
  min-width: 300px;
  min-height: 0;
  border-left: 1px solid var(--border);
}
.chat-resources-overlay {
  position: absolute;
  top: 0;
  right: 0;
  bottom: 0;
  width: min(420px, 88vw);
  min-width: 0;
  border-left: 1px solid var(--border);
  box-shadow: -12px 0 32px rgba(0, 0, 0, 0.22);
  z-index: var(--z-overlay);
}
.chat-resources-backdrop {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.28);
  z-index: calc(var(--z-overlay) - 1);
}
</style>
