<script setup lang="ts">
// 执行单元复合页（2026-11-07 布局重构，用户拍板）：子代理 + 后台任务两 TAB
// 合并进侧栏一个入口——左侧标签页收敛（管理组 15 → 14 项）。
//
// - TAB 状态纯 route 派生（单一真相源 = URL）：面板渲染只跟随已提交的
//   route。曾用本地 ref + watch 双向同步——switchTab 同步写 ref、replace
//   异步落盘，两者解耦；快速点 TAB/深链导航落在在途 replace 窗口里时，
//   旧导航（从点击时刻的 query 构建、不含 focus）后到提交，会把 focus 深链
//   参数整体覆盖掉（充分测试战役 trace 实证：URL 终态有 tab 无 focus、
//   面板已按 tab 挂载但 focus 分支从未执行）。改 computed 后回退/前进、
//   redirect 落位自动跟随 route，无需 watch。
// - 两个现有 View（SubagentsView / ProcessesView）降为 TAB 面板，script 逻辑
//   零改动；?focus= 消费后的 URL 清理是 query-only replace，落在本页路径上
//   天然兼容（只清 focus、保留 tab）。
// - TAB 切换 = 组件销毁/重建（轮询定时器随 onMounted/onBeforeUnmount 正确
//   清理重建）；两 TAB 数据面本就独立（subagents vs background WSAPI）。
// - 已归档 TAB（2026-11-15）：SubagentsView 挂 archived prop 复用（归档是
//   子代理台账的过滤视角，不是独立数据面）；TAB 标签带已归档计数
//   （N != null 时渲染 `(N)`）——计数由面板 loadList 后 emit('archived-count')
//   投影（列表加载时点的全库投影），宿主 ref 收敛；进程 TAB 先行时计数
//   缺席（null → 不渲染括号），进入子代理/归档任一面板即补上。
// - 布局契约（2026-11-09 对齐全仓骨架）：本页是路由页根（page- 前缀拿
//   `.main-content > [class^="page-"]` 的 flex+height:100%），page-header
//   归宿主页；TAB 条用全局 .tabs/.tab 下划线式（components.css，与通道/
//   看板页签等全仓一致）；两个面板 View 均无自身 page-header（标题层级
//   = 大标题→TAB→内容，与 Scanner/Board 等同款）。
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import SubagentsView from './SubagentsView.vue'
import ProcessesView from './ProcessesView.vue'
import BrowserView from './BrowserView.vue'

// 浏览器接管（2026-11-15，计划 D10）：第四 TAB，feature gate（`!== 'false'`
// = 默认包含，同 router 门控惯例）；关 = TAB 不渲染，路由/侧栏零改动。
const browserEnabled = import.meta.env.VITE_FEATURE_BROWSER !== 'false'

type Tab = 'subagents' | 'processes' | 'archived' | 'browser'
const TABS: { key: Tab; label: string }[] = [
  { key: 'subagents', label: '子代理' },
  { key: 'processes', label: '后台任务' },
  { key: 'archived', label: '已归档' },
  ...(browserEnabled ? [{ key: 'browser' as Tab, label: '浏览器' }] : []),
]

const route = useRoute()
const router = useRouter()

const tab = computed<Tab>(() => {
  if (route.query.tab === 'processes') return 'processes'
  if (route.query.tab === 'archived') return 'archived'
  if (route.query.tab === 'browser' && browserEnabled) return 'browser'
  return 'subagents'
})

// 已归档计数（TAB 徽标数据源）：面板每次 loadList 后 emit 投影；
// null = 尚未有任何面板加载过（如直链后台任务 TAB——不渲染括号；
// 缺省/归档 TAB 一挂载即投影，0 也渲染括号）。
const archivedCount = ref<number | null>(null)

function switchTab(t: Tab) {
  if (t === tab.value) return
  // replace 而非 push：TAB 切换不产生历史栈噪音；保留 focus 等其余 query。
  // 面板渲染等导航落盘（route 更新 → computed 更新），不预写本地状态。
  router.replace({ query: { ...route.query, tab: t } })
}
</script>

<template>
  <div class="page-executors">
    <div class="page-header"><h2>执行单元</h2></div>
    <div class="page-body">
      <div class="tabs">
        <button
          v-for="t in TABS"
          :key="t.key"
          class="tab"
          :class="{ active: tab === t.key }"
          @click="switchTab(t.key)"
        >
          {{ t.label }}<template v-if="t.key === 'archived' && archivedCount != null"> ({{ archivedCount }})</template>
        </button>
      </div>
      <SubagentsView
        v-if="tab === 'subagents'"
        @archived-count="(n: number) => (archivedCount = n)"
      />
      <ProcessesView v-else-if="tab === 'processes'" />
      <BrowserView v-else-if="tab === 'browser'" />
      <SubagentsView
        v-else
        :archived="true"
        @archived-count="(n: number) => (archivedCount = n)"
      />
    </div>
  </div>
</template>
