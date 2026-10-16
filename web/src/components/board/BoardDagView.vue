<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { useBoardChanged } from '../../composables/useBoardChanged'
import IssueDetailModal from './IssueDetailModal.vue'
import {
  STATUSES,
  statusLabel,
  statusNodeStyle,
  type IssueRow,
} from './boardMeta'
import {
  computeLayout,
  svgSize,
  nodeDims,
  type DagIssue,
  type DagEdge,
  type DagNode,
} from './dagLayout'

// 托管 Agent 看板 · 全景流程图（2026-10-15 需求②③）：项目级只读 DAG 大图。
// 布局与边走线纯函数在 dagLayout.ts（Kahn 分层 + 环桶 + compact 折叠 + v4
// 零交叉层级走线：传递约简 + Sugiyama 哑节点，硬要求「任何两条线不交叉；
// 目标一致的线合并汇入」）；本组件只做数据加载（board.* WSAPI）与 SVG
// 渲染。只看不改——节点点击开详情弹窗，悬停高亮其直接依赖边。
// 规模不设限：>300 紧凑色块、>2000 渲染照常 + 诚实提示建议筛选。

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const projects = ref<Array<{ id: number; name: string; status: string }>>([])
const selectedProjectId = ref<number | null>(null)
const issues = ref<IssueRow[]>([])

const detailIssueId = ref<number | null>(null)

// 状态点击高亮（统计条 chip 点选：其余节点压暗，再点取消）。
const highlightStatus = ref('')

// 悬停高亮（方案 A ④）：悬停节点 → 其直接入/出边全亮、其余边压暗——
// 「NB-118 到底依赖谁」的即时代答。
const hoveredId = ref<number | null>(null)

// 缩放（viewBox 不变，CSS 宽高缩放；0.3–2.5）。
const zoom = ref(1)
function zoomStep(d: number) {
  zoom.value = Math.min(2.5, Math.max(0.3, Math.round((zoom.value + d) * 10) / 10))
}

// --- 数据加载 ---
async function loadProjects() {
  try {
    const r = await request('board', 'project.list', {})
    projects.value = r?.projects || []
    if (selectedProjectId.value === null && projects.value.length) {
      selectedProjectId.value = projects.value[0].id
    }
  } catch {
    projects.value = []
  }
}

async function loadIssues(silent = false) {
  if (selectedProjectId.value === null) {
    issues.value = []
    return
  }
  try {
    // 已取消单保留（灰 ✕ 完整叙事）；hidden 单后端 issue.list 恒排除。
    const r = await request('board', 'issue.list', {
      project_id: selectedProjectId.value,
      include_cancelled: true,
    })
    issues.value = r?.issues || []
  } catch (e) {
    if (!silent) toast.error('加载流程图数据失败: ' + e)
  }
}

async function refresh(silent = false) {
  loading.value = !silent
  try {
    // 先项目后任务：selectedProjectId 由 loadProjects 兜底设置，loadIssues
    // 依赖它（并行会竞态出「首屏永远空」——曾实测）。
    await loadProjects()
    await loadIssues(silent)
  } finally {
    loading.value = false
  }
}

onMounted(() => refresh())
// board-changed 推送：静默换新（不闪 loading）。
useBoardChanged(() => loadIssues(true))

function onProjectChange() {
  highlightStatus.value = ''
  highlightDecision.value = false
  void loadIssues()
}

// --- 布局（纯函数消费） ---
const layout = computed(() => computeLayout(issues.value as DagIssue[]))
const size = computed(() => svgSize(layout.value))
const dims = computed(() => nodeDims(layout.value.mode))
const nodeById = computed(() => new Map(layout.value.nodes.map((n) => [n.id, n])))
const visibleNodes = computed(() => layout.value.nodes.filter((n) => !n.folded))

// --- 状态统计（点击 = 高亮过滤） ---
const statusCounts = computed(() => {
  const c: Record<string, number> = {}
  for (const s of STATUSES) c[s.key] = 0
  let other = 0
  for (const it of issues.value) {
    if (c[it.status] !== undefined) c[it.status]++
    else other++
  }
  if (other > 0) c.__other = other
  return c
})

// 决策可见性（2026-10-15 批）：「✋ 待决策 N」chip——点击压暗其余节点，
// 一眼圈出等裁决的单（statusCounts 同款交互语言）。
const decisionCount = computed(() => issues.value.filter((i) => !!i.needs_user).length)
const highlightDecision = ref(false)

function toggleHighlight(s: string) {
  highlightStatus.value = highlightStatus.value === s ? '' : s
}

function toggleDecisionHighlight() {
  highlightDecision.value = !highlightDecision.value
}

// --- 渲染辅助 ---
function nodeOpacity(n: DagNode): number {
  if (highlightDecision.value && !n.needsUser) return 0.12
  if (highlightStatus.value && n.status !== highlightStatus.value) return 0.12
  if (n.status === 'cancelled') return 0.5
  return 1
}

/** 决策可见性节点覆盖（2026-10-15 批）：needs_user 最高权重——amber 加粗
 *  描边 + ✋ 图标压状态图标；次之 plan_ready——📋 图标压状态图标（描边不动，
 *  与 in_review amber 底色消歧靠图标）。 */
function nodeStroke(n: DagNode): string {
  if (n.unlayered) return 'var(--error)'
  if (n.needsUser) return 'var(--warning)'
  return statusNodeStyle(n.status).stroke
}

function nodeStrokeWidth(n: DagNode): number {
  if (n.unlayered) return 1.6
  if (n.needsUser) return 3
  return statusNodeStyle(n.status).strokeWidth
}

function nodeIcon(n: DagNode): string {
  if (n.needsUser) return '✋'
  if (n.planReady) return '📋'
  return statusNodeStyle(n.status).icon
}

function edgeOpacity(e: DagEdge): number {
  if (highlightStatus.value) {
    const to = nodeById.value.get(e.to)
    const from = nodeById.value.get(e.from)
    const hit =
      to?.status === highlightStatus.value ||
      from?.status === highlightStatus.value ||
      (e.external && to?.status === highlightStatus.value)
    return hit ? 0.9 : 0.06
  }
  if (hoveredId.value !== null) {
    return e.from === hoveredId.value || e.to === hoveredId.value ? 0.95 : 0.08
  }
  // 拆解边是辅助叙事（父子关系），比已满足的执行边再淡一档。
  return e.blocking ? 0.95 : e.decomposition ? 0.4 : 0.55
}

// 边的 stroke / 线宽（方案 A ③：深色主题对比度——已满足边用前景色而非
// muted 灰，全 done 的图读起来「健康醒目」而不是幽灵线）。
function edgeStroke(e: DagEdge): string {
  if (e.external || e.decomposition) return 'var(--text-muted)'
  return e.blocking ? 'var(--warning)' : 'var(--text)'
}
function edgeWidth(e: DagEdge): number {
  if (e.external) return 1
  if (e.decomposition) return 1.2
  return e.blocking ? 1.8 : 1.5
}
function edgeMarker(e: DagEdge): string {
  if (e.blocking) return 'url(#dag-arrow-warn)'
  if (e.decomposition || e.external) return 'url(#dag-arrow-muted)'
  return 'url(#dag-arrow)'
}

function nodeTitle(n: DagNode): string {
  const lines = [`${n.number} ${n.title}`, `状态: ${statusLabel(n.status)}`]
  if (n.needsUser) lines.push('✋ 停车待你决策——点开查看决策面板')
  else if (n.planReady) lines.push('📋 拆解完成待确认发车')
  if (n.assigneeId) lines.push(`执行: ${n.assigneeId}`)
  // 传递约简省画的上游（+N 徽标的展开）：可达性语义仍在，只是线并入链路。
  if (n.indirectDeps.length)
    lines.push(`间接上游（并入链路不另画线）：${n.indirectDeps.join('、')}`)
  if (n.unlayered) lines.push('⚠ 未分层（环依赖）')
  return lines.join('\n')
}

function truncTitle(s: string, max: number): string {
  return s.length > max ? s.slice(0, max - 1) + '…' : s
}

function onNodeClick(n: DagNode) {
  detailIssueId.value = n.id
}

function onNodeEnter(n: DagNode) {
  hoveredId.value = n.id
}

function onNodeLeave() {
  hoveredId.value = null
}

function onFoldClick(f: { status: string }) {
  toggleHighlight(f.status)
}

const fmtPercent = computed(() => Math.round(zoom.value * 100) + '%')
</script>

<template>
  <div class="dag-view">
    <!-- 工具条：项目选择 + 规模 + 缩放 + 刷新 -->
    <div class="dag-toolbar">
      <label class="dag-field">
        项目
        <select v-model="selectedProjectId" class="input" @change="onProjectChange" :disabled="!projects.length">
          <option v-if="!projects.length" :value="null">（看板暂无项目）</option>
          <option v-for="p in projects" :key="p.id" :value="p.id">
            #{{ p.id }} {{ p.name }}{{ p.status === 'archived' ? '（已归档）' : '' }}
          </option>
        </select>
      </label>
      <span class="dag-meta" v-if="issues.length">
        {{ issues.length }} 节点 · {{ layout.edges.length }} 边 · {{ layout.layerCount }} 层
        <template v-if="layout.hiddenEdges > 0"> · +{{ layout.hiddenEdges }} 间接并入</template>
        <template v-if="layout.mode === 'compact'">（紧凑）</template>
      </span>
      <span class="flex-spacer"></span>
      <span class="dag-zoom">
        <button class="btn btn-sm" @click="zoomStep(-0.1)" title="缩小">−</button>
        <span class="dag-zoom-label">{{ fmtPercent }}</span>
        <button class="btn btn-sm" @click="zoomStep(0.1)" title="放大">＋</button>
      </span>
      <button class="btn btn-sm" @click="refresh()" :disabled="loading">
        {{ loading ? '加载中…' : '↻ 刷新' }}
      </button>
    </div>

    <!-- 状态统计条（点击 chip = 高亮该状态） -->
    <div class="dag-stats" v-if="issues.length">
      <button
        v-for="s in STATUSES"
        :key="s.key"
        class="dag-stat-chip"
        :class="{ active: highlightStatus === s.key, [`chip-${s.key}`]: true }"
        :disabled="!statusCounts[s.key]"
        @click="toggleHighlight(s.key)"
      >
        {{ s.label }} {{ statusCounts[s.key] || 0 }}
      </button>
      <button
        v-if="statusCounts.__other"
        class="dag-stat-chip"
        :class="{ active: highlightStatus === '__other' }"
        @click="toggleHighlight('__other')"
      >
        未知 {{ statusCounts.__other }}
      </button>
      <button
        class="dag-stat-chip chip-decision"
        :class="{ active: highlightDecision }"
        :disabled="!decisionCount"
        data-test="dag-decision-chip"
        title="只显示停车等你裁决的节点"
        @click="toggleDecisionHighlight"
      >
        ✋ 待决策 {{ decisionCount }}
      </button>
    </div>

    <!-- 图例（常驻：compact 模式色+图标是唯一编码） -->
    <div class="dag-legend" v-if="issues.length">
      <span class="legend-item" v-for="s in STATUSES" :key="s.key">
        <svg width="22" height="15">
          <rect
            x="1" y="1" width="20" height="13" rx="2.5"
            :fill="statusNodeStyle(s.key).fill"
            :fill-opacity="statusNodeStyle(s.key).fillOpacity"
            :stroke="statusNodeStyle(s.key).stroke"
            :stroke-width="statusNodeStyle(s.key).strokeWidth"
          />
          <text x="11" y="10.5" text-anchor="middle" font-size="8" fill="var(--text)" opacity="0.9">{{ statusNodeStyle(s.key).icon }}</text>
        </svg>
        {{ s.label }}
      </span>
      <span class="legend-sep"></span>
      <span class="legend-item"><svg width="22" height="15"><rect x="1" y="1" width="20" height="13" rx="2.5" fill="none" stroke="var(--warning)" stroke-width="3"/><text x="11" y="10.5" text-anchor="middle" font-size="8" fill="var(--text)">✋</text></svg> 待你决策</span>
      <span class="legend-item"><svg width="22" height="15"><rect x="1" y="1" width="20" height="13" rx="2.5" fill="var(--info)" fill-opacity="0.12" stroke="var(--info)" stroke-width="1.2"/><text x="11" y="10.5" text-anchor="middle" font-size="8" fill="var(--text)">📋</text></svg> 计划待确认</span>
      <span class="legend-item"><svg width="8" height="26"><line x1="4" y1="1" x2="4" y2="25" stroke="var(--text)" stroke-width="1.5" opacity="0.55"/></svg> 已满足</span>
      <span class="legend-item"><svg width="8" height="26"><line x1="4" y1="1" x2="4" y2="25" stroke="var(--warning)" stroke-width="2" opacity="0.9"/></svg> 阻塞中</span>
      <span class="legend-item"><svg width="8" height="26"><line x1="4" y1="1" x2="4" y2="25" stroke="var(--text-muted)" stroke-width="1.5" stroke-dasharray="4 3"/></svg> 外部依赖</span>
      <span class="legend-item"><svg width="8" height="26"><line x1="4" y1="1" x2="4" y2="25" stroke="var(--text-muted)" stroke-width="1" stroke-dasharray="2 4" opacity="0.6"/></svg> 拆解（父子）</span>
      <span class="legend-item" v-if="layout.hiddenEdges > 0">卡片 +N = 间接上游（可达性并入链路，悬停看明细）</span>
      <span class="legend-sep"></span>
      <span class="legend-item legend-hint">悬停节点 = 高亮其直接依赖边</span>
    </div>

    <!-- 诚实提示 -->
    <div class="dag-banner dag-banner-warn" v-if="layout.mode === 'hint'">
      ⚠ 节点数超过 {{ 2000 }}：已用紧凑渲染，交互可能卡顿。建议回【列表】页按状态/指派筛选后查看局部。
    </div>
    <div class="dag-banner dag-banner-error" v-if="layout.unlayeredCount > 0">
      ⚠ 检测到 {{ layout.unlayeredCount }} 个节点处于循环依赖中（红色虚线描边、置底层）——正常流程不应出现，请检查任务依赖。
    </div>

    <!-- 画布 -->
    <div class="dag-scroll" v-if="issues.length">
      <svg
        class="dag-svg"
        :viewBox="`0 0 ${size.w} ${size.h}`"
        :width="size.w * zoom"
        :height="size.h * zoom"
        role="img"
      >
        <defs>
          <marker id="dag-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M0,0 L8,4 L0,8 z" fill="var(--text)" />
          </marker>
          <marker id="dag-arrow-warn" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M0,0 L8,4 L0,8 z" fill="var(--warning)" />
          </marker>
          <marker id="dag-arrow-muted" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M0,0 L8,4 L0,8 z" fill="var(--text-muted)" />
          </marker>
        </defs>

        <!-- 边（先画，节点覆盖端点；几何全部来自布局期 e.path——v4 零交叉：
             传递约简后的相邻层短段垂直切线贝塞尔链，同目标汇合同一点）。
             key 带类型前缀：同对节点可能既有 depends_on 边又有拆解边
             （防 Vue key 撞车）。 -->
        <g class="dag-edges">
          <template v-for="e in layout.edges" :key="`${e.decomposition ? 'd' : 'x'}-${e.from}-${e.to}`">
            <path
              v-if="e.decomposition"
              :d="e.path"
              fill="none"
              stroke="var(--text-muted)"
              stroke-width="1.2"
              stroke-dasharray="2 4"
              :opacity="edgeOpacity(e)"
              :marker-end="edgeMarker(e)"
            >
              <title>拆解：{{ e.fromNumber }} 的子单（父子关系，非执行依赖）</title>
            </path>
            <path
              v-else-if="!e.external"
              :d="e.path"
              fill="none"
              :stroke="edgeStroke(e)"
              :stroke-width="edgeWidth(e)"
              :opacity="edgeOpacity(e)"
              :marker-end="edgeMarker(e)"
            >
              <title>{{ e.blocking ? `${e.fromNumber} → 未完成（阻塞）` : `${e.fromNumber} → 已满足` }}</title>
            </path>
            <g v-else :opacity="edgeOpacity(e) === 0.06 ? 0.06 : 0.65">
              <rect
                :x="nodeById.get(e.to)!.x + dims.w / 2 - 24"
                :y="nodeById.get(e.to)!.y - 28"
                width="48" height="18" rx="3"
                fill="none" stroke="var(--text-muted)" stroke-dasharray="4 3"
              />
              <text
                :x="nodeById.get(e.to)!.x + dims.w / 2"
                :y="nodeById.get(e.to)!.y - 15.5"
                text-anchor="middle" font-size="9" fill="var(--text-muted)"
              >{{ e.fromNumber }}</text>
              <path
                :d="e.path"
                fill="none" stroke="var(--text-muted)" stroke-width="1" stroke-dasharray="4 3"
                marker-end="url(#dag-arrow-muted)"
              />
              <title>外部依赖：{{ e.fromNumber }}（不在当前项目视图内）</title>
            </g>
          </template>
        </g>

        <!-- full 档节点：完整卡（编号 + 标题 + 图标） -->
        <g v-if="layout.mode === 'full'" class="dag-nodes">
          <g
            v-for="n in visibleNodes"
            :key="n.id"
            class="dag-node"
            :transform="`translate(${n.x}, ${n.y})`"
            :opacity="nodeOpacity(n)"
            @click="onNodeClick(n)"
            @mouseenter="onNodeEnter(n)"
            @mouseleave="onNodeLeave()"
          >
            <rect
              :width="dims.w" :height="dims.h" rx="7"
              :fill="statusNodeStyle(n.status).fill"
              :fill-opacity="statusNodeStyle(n.status).fillOpacity"
              :stroke="nodeStroke(n)"
              :stroke-width="nodeStrokeWidth(n)"
              :stroke-dasharray="n.unlayered ? '4 3' : undefined"
            />
            <text class="dag-num" x="10" y="21">{{ n.number }}</text>
            <text class="dag-icon" :x="dims.w - 12" y="21" text-anchor="middle">{{ nodeIcon(n) }}</text>
            <!-- +N 徽标：传递约简省画的上游数（线并入链路，语义在 tooltip）。 -->
            <text
              v-if="n.indirectDeps.length"
              class="dag-indirect"
              :x="dims.w - 22" y="21" text-anchor="end"
            >+{{ n.indirectDeps.length }}</text>
            <!-- 截断 12 字符：12px 中文 ×12=144px < 卡内宽 164px，防溢出压邻（全题在 tooltip）。 -->
            <text class="dag-title" x="10" y="42">{{ truncTitle(n.title, 12) }}</text>
            <title>{{ nodeTitle(n) }}</title>
          </g>
        </g>

        <!-- compact/hint 档节点：小色块（图标居中）+ 折叠组 ×N -->
        <g v-else class="dag-nodes">
          <g
            v-for="n in visibleNodes"
            :key="n.id"
            class="dag-node"
            :transform="`translate(${n.x}, ${n.y})`"
            :opacity="nodeOpacity(n)"
            @click="onNodeClick(n)"
            @mouseenter="onNodeEnter(n)"
            @mouseleave="onNodeLeave()"
          >
            <rect
              :width="dims.w" :height="dims.h" rx="4"
              :fill="statusNodeStyle(n.status).fill"
              :fill-opacity="statusNodeStyle(n.status).fillOpacity"
              :stroke="nodeStroke(n)"
              :stroke-width="n.needsUser ? 2.4 : (n.unlayered ? 1.4 : statusNodeStyle(n.status).strokeWidth)"
              :stroke-dasharray="n.unlayered ? '3 2' : undefined"
            />
            <text class="dag-icon-sm" :x="dims.w / 2" :y="dims.h / 2 + 4" text-anchor="middle">{{ nodeIcon(n) }}</text>
            <title>{{ nodeTitle(n) }}</title>
          </g>
          <g
            v-for="f in layout.folds"
            :key="`fold-${f.layer}-${f.x}-${f.y}`"
            class="dag-node dag-fold"
            :transform="`translate(${f.x}, ${f.y})`"
            :opacity="highlightStatus && f.status !== highlightStatus ? 0.12 : 1"
            @click="onFoldClick(f)"
          >
            <rect
              :width="dims.w" :height="dims.h" rx="4"
              :fill="statusNodeStyle(f.status).fill"
              :fill-opacity="Math.min(1, statusNodeStyle(f.status).fillOpacity + 0.2)"
              :stroke="statusNodeStyle(f.status).stroke"
              :stroke-width="statusNodeStyle(f.status).strokeWidth"
            />
            <text class="dag-fold-label" :x="dims.w / 2" :y="dims.h / 2 + 4" text-anchor="middle">×{{ f.size }}</text>
            <title>{{ statusLabel(f.status) }} ×{{ f.size }}：{{ f.numbers.join('、') }}（点击高亮该状态）</title>
          </g>
        </g>
      </svg>
    </div>

    <!-- 空态 -->
    <div class="dag-empty" v-else-if="!loading">
      <template v-if="!projects.length">看板暂无项目——先在【项目】页创建项目并拆解任务。</template>
      <template v-else>当前项目暂无任务单。</template>
    </div>
    <div class="dag-empty" v-else>加载中…</div>

    <IssueDetailModal
      v-if="detailIssueId !== null"
      :issue-id="detailIssueId"
      @close="detailIssueId = null"
      @changed="loadIssues(true)"
    />
  </div>
</template>

<style scoped>
.dag-view {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.dag-toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}
.dag-field {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--text-muted);
  font-size: 13px;
}
.dag-field .input {
  min-width: 220px;
}
.dag-meta {
  color: var(--text-muted);
  font-size: 12px;
}
.flex-spacer {
  flex: 1;
}
.dag-zoom {
  display: flex;
  align-items: center;
  gap: 4px;
}
.dag-zoom-label {
  min-width: 42px;
  text-align: center;
  font-size: 12px;
  color: var(--text-muted);
}

.dag-stats {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.dag-stat-chip {
  border: 1px solid var(--border);
  background: var(--bg-secondary);
  color: var(--text-muted);
  border-radius: 999px;
  padding: 3px 12px;
  font-size: 12px;
  cursor: pointer;
  transition: all 0.15s;
}
.dag-stat-chip:disabled {
  opacity: 0.4;
  cursor: default;
}
.dag-stat-chip.active {
  color: var(--text);
  border-color: var(--accent);
  box-shadow: 0 0 0 1px var(--accent);
}
.dag-stat-chip.chip-blocked:not(:disabled) {
  color: var(--error);
}
.dag-stat-chip.chip-done:not(:disabled) {
  color: var(--success);
}
.dag-stat-chip.chip-in_review:not(:disabled) {
  color: var(--warning);
}
.dag-stat-chip.chip-in_progress:not(:disabled),
.dag-stat-chip.chip-todo:not(:disabled) {
  color: var(--info);
}
/* 决策可见性（2026-10-15 批）：✋ chip amber 语义 + active 实底。 */
.dag-stat-chip.chip-decision:not(:disabled) {
  color: var(--warning);
  font-weight: 600;
}
.dag-stat-chip.chip-decision.active {
  background: var(--warning);
  color: #1a1206;
  border-color: var(--warning);
  box-shadow: none;
}

.dag-legend {
  display: flex;
  align-items: center;
  gap: 14px;
  flex-wrap: wrap;
  font-size: 12px;
  color: var(--text-muted);
}
.legend-item {
  display: inline-flex;
  align-items: center;
  gap: 5px;
}
.legend-hint {
  color: var(--text-muted);
  opacity: 0.85;
}
.legend-sep {
  width: 1px;
  height: 14px;
  background: var(--border);
}

.dag-banner {
  border-radius: 8px;
  padding: 8px 14px;
  font-size: 13px;
}
.dag-banner-warn {
  background: var(--warning-bg);
  color: var(--warning);
}
.dag-banner-error {
  background: var(--error-bg);
  color: var(--error);
}

.dag-scroll {
  overflow: auto;
  max-height: calc(100vh - 340px);
  min-height: 280px;
  border: 1px solid var(--border);
  border-radius: 10px;
  background: var(--bg-primary);
}
.dag-svg {
  display: block;
  /* 画布窄于容器时水平居中（布局本身每层已居中，双保险整图居中观感）。 */
  margin: 0 auto;
}
.dag-node {
  cursor: pointer;
}
.dag-node:hover rect {
  filter: brightness(1.25);
}
.dag-num {
  font-size: 12px;
  font-weight: 600;
  fill: var(--text);
}
.dag-title {
  font-size: 12px;
  /* 标题=节点主信息，恒用高对比前景色（状态语义由边框/底色/图标承担）；
     muted 灰压半透明彩色底对比度不足——用户实测「完全看不清」根因之二。 */
  fill: var(--text);
}
.dag-icon,
.dag-icon-sm {
  font-size: 11px;
  fill: var(--text);
}
.dag-indirect {
  font-size: 10px;
  font-weight: 600;
  fill: var(--text-muted);
}
.dag-fold {
  cursor: pointer;
}
.dag-fold-label {
  font-size: 10px;
  font-weight: 700;
  fill: var(--text);
}

.dag-empty {
  padding: 48px 0;
  text-align: center;
  color: var(--text-muted);
  font-size: 14px;
}
</style>
