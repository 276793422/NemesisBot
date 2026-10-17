<script setup lang="ts">
import { onUnmounted, ref } from 'vue'
import BoardTabs from '../components/board/BoardTabs.vue'
import BoardKanban from '../components/board/BoardKanban.vue'
import IssueListView from './IssueListView.vue'
import BoardDagView from '../components/board/BoardDagView.vue'
import ProjectPanel from '../components/board/ProjectPanel.vue'
import InboxPanel from '../components/board/InboxPanel.vue'
import AutopilotPanel from '../components/board/AutopilotPanel.vue'
import DiscussionPanel from '../components/board/DiscussionPanel.vue'
import AuditPanel from '../components/board/AuditPanel.vue'
import BoardConfigPanel from '../components/board/BoardConfigPanel.vue'
import { useToast } from '../composables/useToast'
import { on as sseOn, off as sseOff } from '../composables/useSSE'
import { usePlanning, fmtElapsed, ABNORMAL_SECS } from '../composables/usePlanning'

// 托管 Agent 看板容器（W2 P3/P4）：项目 / 列表 / 看板（Kanban 拖拽） /
// 流程图（全景 DAG，2026-10-15） / 收件箱 / 自动化 / 讨论 / 决策流（P5/E2） /
// 配置 九个页签（顺序=使用依赖链，见 BoardTabs.vue）。结构同 ClusterView
// （页签容器 + components/board/ 子组件）。

// 默认落「项目」页签——排序即依赖链：没有项目就没有列表/看板可看。
const activeTab = ref('projects')

const tabMap: Record<string, any> = {
  kanban: BoardKanban,
  list: IssueListView,
  dag: BoardDagView,
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

// §1.2C 持续横幅（2026-10-11 Part 1 配套前端）：toast 只在事件瞬间闪一下，
// 用户停留期间的主体反馈是这条横幅——自计时 + 超时异常态（>20 分钟变黄，
// 正常 3~8 分钟）。数据源 = usePlanning 单例（SSE + issue.list 投影 reconcile）。
const { list: planningList, elapsed } = usePlanning()
</script>

<template>
  <div class="page-board">
    <div class="page-header"><h2>Agent 看板</h2></div>
    <div class="page-body">
      <!-- 拆解在途横幅（§1.2C）：每条在途计划一行自计时；超 ABNORMAL_SECS
           转「拆解异常」黄态（仅 planning 阶段——dispatching 是发车派发，
           逐子单 RPC 送达可达分钟级，不是异常）。数据源 = usePlanning 单例。 -->
      <div v-if="planningList.length" class="planning-banner" data-test="planning-banner">
        <div
          v-for="e in planningList"
          :key="e.issueId"
          class="planning-line"
          :class="{ abnormal: e.phase === 'planning' && elapsed(e.issueId) > ABNORMAL_SECS }"
          :data-test="'planning-line-' + e.issueNumber"
        >
          <template v-if="e.phase === 'dispatching'">
            🚗 {{ e.issueNumber }} 拆解完成，自动发车派发中…（已 {{ fmtElapsed(elapsed(e.issueId)) }}，子单已在看板可见）
          </template>
          <template v-else-if="elapsed(e.issueId) > ABNORMAL_SECS">
            ⚠ 拆解异常：{{ e.issueNumber }} 已拆解 {{ fmtElapsed(elapsed(e.issueId)) }}（远超正常 3~8 分钟）——请到日志页查 planner 现场或取消后重试
          </template>
          <template v-else>
            🧩 正在 AI 拆解 {{ e.issueNumber }}…（<template v-if="e.attempt > 1">第 {{ e.attempt }} 次尝试，</template>已 {{ fmtElapsed(elapsed(e.issueId)) }}，通常 3~8 分钟）
          </template>
        </div>
      </div>
      <BoardTabs v-model="activeTab" />
      <div style="margin-top:var(--space-4)">
        <component :is="tabMap[activeTab]" />
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 拆解在途横幅（§1.2C）：信息蓝常态 / 异常黄态（对齐 anchor-warn-bar 色板）。 */
.planning-banner {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  margin-bottom: var(--space-3);
}
.planning-line {
  background: var(--info-bg, rgba(0, 120, 212, 0.08));
  border: 1px solid var(--info-border, rgba(0, 120, 212, 0.35));
  border-radius: var(--radius-sm, 6px);
  color: var(--text-primary);
  font-size: var(--text-sm);
  padding: var(--space-2) var(--space-3);
}
.planning-line.abnormal {
  background: var(--warning-bg, rgba(255, 193, 7, 0.12));
  border-color: var(--warning-border, rgba(255, 193, 7, 0.45));
}
</style>
