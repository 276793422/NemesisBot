<script setup lang="ts">
/**
 * 皮肤骨架槽位：左侧栏（WB 形态）——皮肤激活时由 AppLayout 渲染以替换
 * 主导航 Sidebar。结构照 Buddy 规格：新建对话 + 4 导航项 + 「更多」flyout
 * （全部管理页，功能可达性不缩水）+ 会话历史（时间分组）+ 空间（本机）+
 * footer（用户 + 设置齿轮）。根元素复用全局 .sidebar 定位壳（flex 布局、
 * --sidebar-width、主题 token），内部 .nb-sb-* 基线样式在 components.css
 * （html[data-skin] 门控），皮肤包 CSS 同选择器品牌化。
 */
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useRouter } from 'vue-router'
import { useAppStore } from '../stores/app'
import { useSessionStore } from '../stores/session'
import type { SessionEntry } from '../composables/useChatApi'
import { useToast } from '../composables/useToast'

const router = useRouter()
const appStore = useAppStore()
const sessionStore = useSessionStore()
const toast = useToast()

// 导航四项（WB 视觉形态，映射真实页面）：人格=助理、代码开发=项目、
// Skills=专家、定时任务=自动化
const NAVS: { label: string; path: string; icon: string }[] = [
  {
    label: '人格',
    path: '/persona',
    icon: 'M12 12c2.21 0 4-1.79 4-4s-1.79-4-4-4-4 1.79-4 4 1.79 4 4 4zm0 2c-2.67 0-8 1.34-8 4v2h16v-2c0-2.66-5.33-4-8-4z',
  },
  {
    label: '代码开发',
    path: '/coding',
    icon: 'M16 18l6-6-6-6M8 6l-6 6 6 6',
  },
  {
    label: 'Skills',
    path: '/skills',
    icon: 'M12 2l2.4 4.86 5.36.78-3.88 3.78.92 5.34L12 14.24l-4.8 2.52.92-5.34L4.24 7.64l5.36-.78L12 2z',
  },
  {
    label: '定时任务',
    path: '/tasks',
    icon: 'M9 11l3 3L22 4 M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11',
  },
]

// 「更多」flyout：其余全部管理页（Sidebar 导航清单的剩余项，内部滚动）
const MORE_ITEMS: { label: string; path: string }[] = [
  { label: '概览', path: '/overview' },
  { label: '使用统计', path: '/usage' },
  { label: '日志', path: '/logs' },
  { label: '模型', path: '/models' },
  { label: '记忆', path: '/memory' },
  { label: '人格超市', path: '/persona-shop' },
  { label: 'MCP', path: '/mcp' },
  { label: 'HOOK', path: '/hooks' },
  { label: '命令', path: '/commands' },
  { label: '插件', path: '/plugins' },
  { label: '子Agent', path: '/subagents' },
  { label: '通道', path: '/channels' },
  { label: '工作流', path: '/workflows' },
  { label: '看板', path: '/board' },
  { label: '集群', path: '/cluster' },
  { label: '安全', path: '/security' },
  { label: '扫描器', path: '/scanner' },
  { label: '沙盒', path: '/sandbox' },
  { label: '终端', path: '/terminal' },
  { label: '本地模型', path: '/local-models' },
  { label: 'Tools', path: '/tools' },
  { label: '二次开发', path: '/sdk' },
  { label: '代理设置', path: '/proxy-settings' },
  { label: '设置', path: '/settings' },
  { label: '关于', path: '/about' },
]

const showMore = ref(false)

// 定制构建（VITE_FEATURE_* 裁剪）下部分路由未注册——resolve().matched 空 =
// 路由不存在，flyout 不列（点了跳空的）。全量构建全部在列，行为不变。
const moreItems = computed(() =>
  MORE_ITEMS.filter((it) => router.resolve(it.path).matched.length > 0)
)

// flyout 点外部收起（「更多」按钮与 flyout 自身在 wrap 内，不触发）
function onDocClick(e: MouseEvent): void {
  if (!(e.target as HTMLElement).closest('.nb-sb-more-wrap')) showMore.value = false
}

function goto(path: string) {
  showMore.value = false
  router.push(path)
}

async function newTask() {
  const sid = await sessionStore.create()
  if (!sid) {
    toast.error('新建会话失败')
    return
  }
  router.push('/')
}

// 会话按时间归组（WB 同款：今天/昨天/7 天内/更早）
const sessionGroups = computed(() => {
  const now = new Date()
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()
  const dayMs = 86400000
  const groups: { label: string; items: SessionEntry[] }[] = [
    { label: '今天', items: [] },
    { label: '昨天', items: [] },
    { label: '7 天内', items: [] },
    { label: '更早', items: [] },
  ]
  for (const s of sessionStore.sessions) {
    const t = s.lastTime ? Date.parse(s.lastTime) : NaN
    if (Number.isNaN(t)) groups[3].items.push(s)
    else if (t >= startOfToday) groups[0].items.push(s)
    else if (t >= startOfToday - dayMs) groups[1].items.push(s)
    else if (t >= startOfToday - 7 * dayMs) groups[2].items.push(s)
    else groups[3].items.push(s)
  }
  return groups.filter((g) => g.items.length > 0)
})

function relTime(s: SessionEntry): string {
  if (!s.lastTime) return ''
  const t = Date.parse(s.lastTime)
  if (Number.isNaN(t)) return ''
  const diff = Date.now() - t
  if (diff < 60000) return '刚刚'
  if (diff < 3600000) return `${Math.floor(diff / 60000)} 分钟前`
  if (diff < 86400000) return `${Math.floor(diff / 3600000)} 小时前`
  return `${Math.floor(diff / 86400000)} 天前`
}

function title(s: SessionEntry): string {
  return s.title || s.firstMessage || s.id.slice(0, 8)
}

function isActive(s: SessionEntry): boolean {
  return s.id === sessionStore.currentId && router.currentRoute.value.path === '/'
}

function switchSession(id: string) {
  sessionStore.switchTo(id)
  router.push('/')
}

async function removeSession(s: SessionEntry) {
  if (!window.confirm(`删除会话「${title(s)}」？此操作不可恢复。`)) return
  await sessionStore.remove(s.id)
}

onMounted(() => {
  // 会话列表数据（store 内带 5s 缓存与静默容错；ChatView 也会拉，重复零开销）
  void sessionStore.fetchList()
  void sessionStore.fetchProjects()
  document.addEventListener('click', onDocClick)
})

onUnmounted(() => {
  document.removeEventListener('click', onDocClick)
})
</script>

<template>
  <!-- mobile-open 跟原 Sidebar 同款抽屉机制（layout.css ≤768px
       translateX(-100%) 藏、hamburger 经 showMobileSidebar 开合） -->
  <aside class="sidebar nb-sb" :class="{ 'mobile-open': appStore.showMobileSidebar }">
    <button class="nb-sb-newtask" @click="newTask" title="新建对话">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
      新建对话
    </button>

    <nav class="nb-sb-nav">
      <button
        v-for="nav in NAVS"
        :key="nav.path"
        class="nb-sb-item"
        :class="{ 'nb-sb-item--active': router.currentRoute.value.path === nav.path }"
        @click="goto(nav.path)"
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path :d="nav.icon" /></svg>
        {{ nav.label }}
      </button>

      <div class="nb-sb-more-wrap">
        <button
          class="nb-sb-item"
          :class="{ 'nb-sb-item--active': showMore }"
          @click="showMore = !showMore"
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/></svg>
          更多
        </button>
        <div v-if="showMore" class="nb-sb-flyout">
          <button v-for="it in moreItems" :key="it.path" class="nb-sb-flyout-item" @click="goto(it.path)">
            {{ it.label }}
          </button>
        </div>
      </div>
    </nav>

    <div class="nb-sb-scroll">
      <div v-for="g in sessionGroups" :key="g.label" class="nb-sb-group">
        <div class="nb-sb-section">
          <span class="nb-sb-caret">
            <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="9 18 15 12 9 6"/></svg>
          </span>
          {{ g.label }}
        </div>
        <div
          v-for="s in g.items"
          :key="s.id"
          class="nb-sb-row"
          :class="{ 'nb-sb-row--active': isActive(s) }"
          :title="title(s)"
          @click="switchSession(s.id)"
        >
          <span class="nb-sb-row-title">{{ title(s) }}</span>
          <span class="nb-sb-row-time">{{ relTime(s) }}</span>
          <span class="nb-sb-row-del" title="删除会话" @click.stop="removeSession(s)">
            <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
          </span>
        </div>
      </div>
      <div v-if="sessionStore.sessions.length === 0" class="nb-sb-empty">暂无会话</div>

      <div class="nb-sb-section nb-sb-section--space">
        <span class="nb-sb-caret">
          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="9 18 15 12 9 6"/></svg>
        </span>
        空间
      </div>
      <div class="nb-sb-row" title="本机（单机模式）">
        <span class="nb-sb-dot" />
        <span class="nb-sb-row-title">本机</span>
      </div>
    </div>

    <div class="nb-sb-footer">
      <span class="nb-sb-avatar">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/></svg>
      </span>
      <span class="nb-sb-user">
        <span class="nb-sb-user-name">本机用户</span>
        <span class="nb-sb-user-sub">本地模式</span>
      </span>
      <button class="nb-sb-gear" title="设置" @click="goto('/settings')">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-2.82 1.18V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1.08-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 1.18-2.82H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1.08 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 2.82-1.18V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1.08 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-1.18 2.82H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1.08z"/></svg>
      </button>
    </div>
  </aside>
</template>
