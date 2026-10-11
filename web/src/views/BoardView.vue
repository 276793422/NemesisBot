<script setup lang="ts">
import { onUnmounted, ref } from 'vue'
import BoardTabs from '../components/board/BoardTabs.vue'
import BoardKanban from '../components/board/BoardKanban.vue'
import IssueListView from './IssueListView.vue'
import ProjectPanel from '../components/board/ProjectPanel.vue'
import InboxPanel from '../components/board/InboxPanel.vue'
import AutopilotPanel from '../components/board/AutopilotPanel.vue'
import DiscussionPanel from '../components/board/DiscussionPanel.vue'
import AuditPanel from '../components/board/AuditPanel.vue'
import BoardConfigPanel from '../components/board/BoardConfigPanel.vue'
import { useToast } from '../composables/useToast'
import { on as sseOn, off as sseOff } from '../composables/useSSE'

// 托管 Agent 看板容器（W2 P3/P4）：项目 / 列表 / 看板（Kanban 拖拽） /
// 收件箱 / 自动化 / 讨论 / 决策流（P5/E2） / 配置 八个页签（顺序=使用依赖
// 链，见 BoardTabs.vue）。结构同 ClusterView（页签容器 + components/board/
// 子组件）。

// 默认落「项目」页签——排序即依赖链：没有项目就没有列表/看板可看。
const activeTab = ref('projects')

const tabMap: Record<string, any> = {
  kanban: BoardKanban,
  list: IssueListView,
  projects: ProjectPanel,
  inbox: InboxPanel,
  autopilot: AutopilotPanel,
  discussion: DiscussionPanel,
  audit: AuditPanel,
  config: BoardConfigPanel,
}

// 拆解运行期可观测（2026-10-11「停着」事故）：planner 单次 4~7 分钟此前
// 零反馈，用户体感=「停着」。容器层挂 plan_started 全局 toast——auto_start
// 建项目后用户停在「项目」页签也能看到「拆解中」信号（评论留痕是持久通道，
// toast 是实时通道）。plan_ready/failed 的模态内状态在 IssueDetailModal。
const toast = useToast()
function onPlanStarted(data: any) {
  toast.info(`🧩 单 ${data?.issue_number || '#' + (data?.issue_id ?? '?')} 开始 AI 拆解（通常 3~8 分钟，完成后自动发车）`)
}
sseOn('board.plan_started', onPlanStarted)
onUnmounted(() => sseOff('board.plan_started', onPlanStarted))
</script>

<template>
  <div class="page-board">
    <div class="page-header"><h2>Agent 看板</h2></div>
    <div class="page-body">
      <BoardTabs v-model="activeTab" />
      <div style="margin-top:var(--space-4)">
        <component :is="tabMap[activeTab]" />
      </div>
    </div>
  </div>
</template>
