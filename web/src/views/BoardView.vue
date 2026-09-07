<script setup lang="ts">
import { ref } from 'vue'
import BoardTabs from '../components/board/BoardTabs.vue'
import BoardKanban from '../components/board/BoardKanban.vue'
import IssueListView from './IssueListView.vue'
import ProjectPanel from '../components/board/ProjectPanel.vue'
import InboxPanel from '../components/board/InboxPanel.vue'
import AutopilotPanel from '../components/board/AutopilotPanel.vue'

// 托管 Agent 看板容器（W2 P3/P4）：项目 / 列表 / 看板（Kanban 拖拽） /
// 收件箱 / 自动化 五个页签（顺序=使用依赖链，见 BoardTabs.vue）。
// 结构同 ClusterView（页签容器 + components/board/ 子组件）。

// 默认落「项目」页签——排序即依赖链：没有项目就没有列表/看板可看。
const activeTab = ref('projects')

const tabMap: Record<string, any> = {
  kanban: BoardKanban,
  list: IssueListView,
  projects: ProjectPanel,
  inbox: InboxPanel,
  autopilot: AutopilotPanel,
}
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
