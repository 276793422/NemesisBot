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
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import SubagentsView from './SubagentsView.vue'
import ProcessesView from './ProcessesView.vue'

type Tab = 'subagents' | 'processes'
const TABS: { key: Tab; label: string }[] = [
  { key: 'subagents', label: '子代理' },
  { key: 'processes', label: '后台任务' },
]

const route = useRoute()
const router = useRouter()

const tab = computed<Tab>(() => (route.query.tab === 'processes' ? 'processes' : 'subagents'))

function switchTab(t: Tab) {
  if (t === tab.value) return
  // replace 而非 push：TAB 切换不产生历史栈噪音；保留 focus 等其余 query。
  // 面板渲染等导航落盘（route 更新 → computed 更新），不预写本地状态。
  router.replace({ query: { ...route.query, tab: t } })
}
</script>

<template>
  <div class="page-executors">
    <div class="exec-tabbar">
      <button
        v-for="t in TABS"
        :key="t.key"
        class="exec-tab"
        :class="{ active: tab === t.key }"
        @click="switchTab(t.key)"
      >
        {{ t.label }}
      </button>
    </div>
    <SubagentsView v-if="tab === 'subagents'" />
    <ProcessesView v-else />
  </div>
</template>

<style scoped>
.exec-tabbar {
  display: flex;
  gap: var(--space-2);
  margin-bottom: var(--space-4);
}
.exec-tab {
  padding: var(--space-2) var(--space-4);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--bg-primary);
  color: var(--text-secondary);
  font-size: var(--text-sm);
  cursor: pointer;
  transition: background 0.15s, color 0.15s, border-color 0.15s;
}
.exec-tab:hover {
  color: var(--text-primary);
  border-color: var(--accent);
}
.exec-tab.active {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
  font-weight: 500;
}
</style>
