<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { useAppStore } from '../stores/app'
import Sidebar from './Sidebar.vue'
import SkinSlot from './SkinSlot.vue'
import ToastContainer from './ToastContainer.vue'
import ApprovalCard from './ApprovalCard.vue'
import QuestionCard from './QuestionCard.vue'
import CanvasPanel from './CanvasPanel.vue'
import { useApprovals } from '../composables/useApprovals'
import { useQuestions } from '../composables/useQuestions'
import { useCanvas } from '../composables/useCanvas'
import { useEditorMode } from '../composables/useEditorMode'
// 皮肤结构皮肤（v2）：槽位由引擎渲染（skinHasSlot 判定在场，CSS-only
// 包全部回落原生）；投影 + 动作白名单在此装配一次（模块单例）。
import { skinHasSlot, ensureSkinMeta } from '../composables/useSkin'
import { setupSkinProjection } from '../skins/projection'

const appStore = useAppStore()
// M7：审批卡单例订阅（SSE approval-requested + pending 补拉）。
const { initApprovals } = useApprovals()
// F7：提问卡单例订阅（SSE question-asked + pending 补拉）。
const { initQuestions } = useQuestions()
// P30（WS14）：Canvas 面板单例订阅（SSE canvas.open）。
const { initCanvas } = useCanvas()
// Full Access 放行开关单例订阅（SSE editor-mode + editor.get seed 对齐）。
const { initEditorMode } = useEditorMode()

// 皮肤数据投影 + 动作白名单装配（幂等单例；router 须从 setup 传）。
const router = useRouter()
setupSkinProjection(router)

// v3 page 槽名随路由（page:/models 等）；皮肤未声明该槽 = 内置视图。
const pageSlotName = computed(() => 'page:' + router.currentRoute.value.path)

onMounted(() => {
  initApprovals()
  initQuestions()
  initCanvas()
  initEditorMode()
  if (skinHasSlot('sidebar') || skinHasSlot('statusbar')) ensureSkinMeta()
})
</script>

<template>
  <div class="app-layout" data-nb-shell="root" :class="{ 'focus-mode': appStore.focusMode }">
    <!-- 皮肤槽位：顶栏（品牌 + 连接状态；结构皮肤才在场） -->
    <SkinSlot v-if="skinHasSlot('titlebar')" name="titlebar" />

    <!-- Mobile Overlay（data-nb-shell 标记 = 皮肤壳 CSS 唯一合法锚点，
         contract.spec 守护） -->
    <div class="mobile-overlay" data-nb-shell="mobile-overlay" :class="{ show: appStore.showMobileSidebar }" @click="appStore.toggleMobileSidebar()"></div>

    <!-- 皮肤槽位：左侧栏（新建对话+导航+会话历史+E-Stop），结构皮肤时
         替换主导航；CSS-only 包 / 无皮肤回落原生 Sidebar -->
    <SkinSlot v-if="skinHasSlot('sidebar')" name="sidebar" />
    <Sidebar v-else />

    <main class="main-content" data-nb-shell="main">
      <!-- Mobile Header -->
      <div class="mobile-header">
        <button class="hamburger-btn" @click="appStore.toggleMobileSidebar()">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="3" y1="6" x2="21" y2="6"/><line x1="3" y1="12" x2="21" y2="12"/><line x1="3" y1="18" x2="21" y2="18"/></svg>
        </button>
        <span style="font-weight:600;">NemesisBot</span>
      </div>

      <!-- v3 page 槽：皮肤声明 page:<path> 槽（如 page:/models）时该路由
           整页替换（数据由投影 pages 切片自装载）；无槽 = 内置视图 -->
      <SkinSlot v-if="skinHasSlot(pageSlotName)" :name="pageSlotName" />
      <router-view v-else />
    </main>

    <!-- 皮肤槽位：状态栏（连接 + 版本 + 模型 + 就绪态） -->
    <SkinSlot v-if="skinHasSlot('statusbar')" name="statusbar" />

    <ToastContainer />
    <!-- M7：安全审批卡（模态，任意页面可见） -->
    <ApprovalCard />
    <!-- F7：结构化提问卡（模态，任意页面可见） -->
    <QuestionCard />
    <!-- P30（WS14）：Canvas 面板（浮窗，任意页面可见；无画布时不渲染） -->
    <CanvasPanel />
  </div>
</template>
