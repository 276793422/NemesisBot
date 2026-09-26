<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useAppStore } from '../stores/app'
import Sidebar from './Sidebar.vue'
import SkinSidebar from './SkinSidebar.vue'
import ToastContainer from './ToastContainer.vue'
import ApprovalCard from './ApprovalCard.vue'
import QuestionCard from './QuestionCard.vue'
import { useApprovals } from '../composables/useApprovals'
import { useQuestions } from '../composables/useQuestions'
import { useEditorMode } from '../composables/useEditorMode'
// 皮肤骨架槽位（顶栏/状态栏）：skinState.id 非空才渲染，默认观感零变化
import { skinState, ensureSkinMeta } from '../composables/useSkin'
import { httpGet } from '../composables/useWebSocket'
import { apiUrl } from '../lib/appBase'

const appStore = useAppStore()
// M7：审批卡单例订阅（SSE approval-requested + pending 补拉）。
const { initApprovals } = useApprovals()
// F7：提问卡单例订阅（SSE question-asked + pending 补拉）。
const { initQuestions } = useQuestions()
// Full Access 放行开关单例订阅（SSE editor-mode + editor.get seed 对齐）。
const { initEditorMode } = useEditorMode()

// 状态栏数据（OverviewView 同源：GET /api/status；只在皮肤激活时拉，
// 版本/活跃模型）。拉取失败静默——状态栏少两段展示，不炸。
interface StatusbarStatus {
  version?: string
  model?: string
}
const statusbar = ref<StatusbarStatus>({})

onMounted(() => {
  initApprovals()
  initQuestions()
  initEditorMode()
  if (skinState.id) {
    ensureSkinMeta()
    fetchStatusbar()
  }
})

/** 状态栏取数：/api/status 在 REST 鉴权闸内，按 useSSE 同款惯例附 ?token=。 */
function fetchStatusbar(): void {
  const stored = localStorage.getItem('nemesisbot_auth_token')
  const url = stored ? `${apiUrl('/api/status')}?token=${encodeURIComponent(stored)}` : apiUrl('/api/status')
  httpGet<StatusbarStatus>(url)
    .then((s) => (statusbar.value = s))
    .catch(() => {})
}

/** 活跃模型展示名（provider/name → name；OverviewView 同款口径）。 */
function shortModel(m?: string): string {
  if (!m) return ''
  const i = m.indexOf('/')
  return i >= 0 ? m.slice(i + 1) : m
}
</script>

<template>
  <div class="app-layout" :class="{ 'focus-mode': appStore.focusMode }">
    <!-- 皮肤骨架槽位 1/3：顶栏（品牌 + 连接状态；仅皮肤激活时在场） -->
    <header v-if="skinState.id" class="nb-titlebar">
      <span class="nb-titlebar-brand">
        <i class="nb-brand-mark" aria-hidden="true"></i>
        <span class="nb-titlebar-name">{{ skinState.meta?.brand || skinState.id }}</span>
      </span>
      <span class="nb-titlebar-status">
        <i class="nb-status-dot" :class="{ off: !appStore.connected }" aria-hidden="true"></i>
        {{ appStore.connected ? '已连接' : '未连接' }}
      </span>
    </header>

    <!-- Mobile Overlay -->
    <div class="mobile-overlay" :class="{ show: appStore.showMobileSidebar }" @click="appStore.toggleMobileSidebar()"></div>

    <!-- 皮肤骨架槽位 4：左侧栏（WB 形态：新建对话+导航+会话历史），皮肤激活时替换主导航 -->
    <SkinSidebar v-if="skinState.id" />
    <Sidebar v-else />

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

    <!-- 皮肤骨架槽位 2/3：状态栏（连接 + 版本 + 模型 + 就绪态；仅皮肤激活时在场） -->
    <footer v-if="skinState.id" class="nb-statusbar">
      <span class="nb-statusbar-left">
        <i class="nb-status-dot" :class="{ off: !appStore.connected }" aria-hidden="true"></i>
        {{ appStore.connected ? '已连接' : '未连接' }}
        <template v-if="statusbar.version">&nbsp;·&nbsp;v{{ statusbar.version }}</template>
        <template v-if="shortModel(statusbar.model)">&nbsp;·&nbsp;{{ shortModel(statusbar.model) }}</template>
      </span>
      <span class="nb-statusbar-right">{{ appStore.connected ? '就绪' : '离线' }}</span>
    </footer>

    <ToastContainer />
    <!-- M7：安全审批卡（模态，任意页面可见） -->
    <ApprovalCard />
    <!-- F7：结构化提问卡（模态，任意页面可见） -->
    <QuestionCard />
  </div>
</template>
