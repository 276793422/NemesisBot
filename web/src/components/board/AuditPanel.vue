<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { useBoardChanged } from '../../composables/useBoardChanged'
import { useBoardActors } from '../../composables/useBoardActors'
import { fmtTime } from './boardMeta'

// 决策流审计面板（全自动流转 P5/E2）：agent 自动决策的时间倒序流水。
// 每行 = 一条 `auto_decide` 活动（决策词 + verdict + details JSON 展开），
// 对已自动收货（done）的决策可一键回滚（单据退回 in_review，防重由后端
// 仲裁）；S-O1：合并停车（merge_parked）行可一键重试合并。后端唯一真相
// 源：crates/nemesis-web/src/handlers/board.rs（board.audit.*）+ nemesis-board
// store + nemesisbot::board_archive_ingest。

const { request } = useWSAPI()
const toast = useToast()
// H1（goal P1）：决策流 actor 可读名（agent/Alex），未知回退短 id。
const { ensureNodes, displayActor } = useBoardActors()

interface AuditRow {
  id: number
  issue_id: number
  actor: { kind: string; id: string }
  action: string
  details: string | null
  created_at: number
  issue_number: string
  issue_title: string
}

// 决策词 → 人话标签（与 board_review.rs 8 处处置臂 + A1 auto_confirm 对齐；
// P5 冲突漏斗 4 词：conflict 停车 / conflict_auto_resolve 硬解 / 重派 / 换人）。
const DECISION_LABEL: Record<string, string> = {
  auto_accept: '验收 PASS · 自动收货',
  suggest_manual: '验收通过 · 建议人工确认',
  redispatch: '验收 FAIL · 自动重派',
  escalate_human: '验收 UNSURE · 转人工',
  parent_auto_close: '父单验收 PASS · 自动收口',
  parent_escalate_human: '父单 FAIL/UNSURE · 转人工',
  project_complete: '项目验收 PASS · 自动收口',
  project_escalate_human: '项目 FAIL/UNSURE · 转人工',
  auto_confirm_dispatch: '拆解计划 · 自动发车',
  conflict: '合并冲突 · 冻结转人工',
  conflict_auto_resolve: '合并冲突 · AI 硬解落定',
  conflict_redispatch: '冲突硬解失败 · 重派原 worker',
  conflict_switch_worker: '冲突原 worker 无应答 · 换节点重派',
  stale_review_discarded: '迟到评审结论 · 丢弃让位',
}

// 非 auto_decide 活动词 → 人话标签（S-O1：合并停车行要能被认出来）。
const ACTION_LABEL: Record<string, string> = {
  merge_parked: '合并停车（变更集未入库）',
  archive_overlimit: '执行档案超护栏',
  archive_orphaned: '执行档案无处安置',
  archive_superseded: '变更集丢弃（基线失配）',
}

interface ParsedDetails {
  decision?: string
  verdict?: string
  gap?: string
  note?: string
  round?: number
  unlimited?: boolean
  [k: string]: unknown
}

const loading = ref(true)
const rows = ref<AuditRow[]>([])
const actionFilter = ref('')

// A1（看板项目档案 goal P1）：只看可回滚——默认只留「验收 PASS · 自动收货」
// 行（回滚按钮有意义的行），纯客户端筛选不改后端语义；取消勾选看全量。
const onlyRollback = ref(true)
const visibleRows = computed(() =>
  onlyRollback.value ? rows.value.filter((r) => canRollback(r)) : rows.value
)

// details JSON 展开（解析失败 = 原文展示，不炸渲染）。
const expanded = ref<Record<number, boolean>>({})

function parseDetails(raw: string | null): ParsedDetails | null {
  if (!raw) return null
  try {
    const v = JSON.parse(raw)
    return typeof v === 'object' && v !== null ? (v as ParsedDetails) : null
  } catch {
    return null
  }
}

function decisionLabel(row: AuditRow): string {
  const d = parseDetails(row.details)
  const key = d?.decision ?? ''
  return DECISION_LABEL[key] ?? ACTION_LABEL[row.action] ?? key ?? row.action
}

// 回滚只对「验收 PASS · 自动收货」决策有意义（后端 store.rollback_decision
// 三重校验：auto_decide + 单据 done；重派/转人工/发车类不支持）。按状态
// 隐藏按钮而非点了吃报错（UX 瑕疵修复，goal H 批顺带）。
function canRollback(row: AuditRow): boolean {
  return parseDetails(row.details)?.decision === 'auto_accept'
}

// S-O1：合并停车行可重试合并（按 issue 扫档案树未合并 placement 重走合并；
// estop 中由后端拒绝）。
function canRetryMerge(row: AuditRow): boolean {
  return row.action === 'merge_parked'
}

const retryBusy = ref(false)

async function doRetryMerge(row: AuditRow) {
  retryBusy.value = true
  try {
    const r = await request('board', 'audit.retry_merge', { id: row.issue_id })
    const rows: Array<{ task_id?: string; attempt?: string; skipped?: string }> = r?.retried || []
    if (rows.length === 0) {
      toast.info(`${row.issue_number}: 档案树无可重试的交付`)
    } else {
      const summary = rows
        .map((x) => x.task_id ? `${x.task_id} → ${x.skipped ?? x.attempt ?? '?'}` : String(x.skipped ?? '跳过'))
        .join('；')
      const allMerged = rows.every((x) => (x.attempt ?? '').includes('Merged') || x.skipped)
      if (allMerged) toast.success(`${row.issue_number} 重试完成：${summary}`)
      else toast.warn(`${row.issue_number} 重试结果：${summary}`)
    }
    await load()
  } catch (e: any) {
    toast.error('重试合并失败: ' + e)
  } finally {
    retryBusy.value = false
  }
}

async function load(silent = false) {
  if (!silent) loading.value = true
  try {
    const r = await request('board', 'audit.list', {
      limit: 200,
      action: actionFilter.value.trim() === '' ? undefined : actionFilter.value.trim(),
    })
    rows.value = r?.decisions || []
    expanded.value = {}
  } catch (e: any) {
    if (silent) console.warn('[AuditPanel] silent refresh failed:', e)
    else toast.error('加载决策流失败: ' + e)
  } finally {
    loading.value = false
  }
}

// 回滚确认弹窗（E2 goal：回滚按钮 + 确认流；防重/状态校验由后端仲裁）。
const confirmRollback = ref<AuditRow | null>(null)
const rollbackBusy = ref(false)

function askRollback(row: AuditRow) {
  confirmRollback.value = row
}

async function doRollback() {
  const row = confirmRollback.value
  if (!row) return
  rollbackBusy.value = true
  try {
    await request('board', 'audit.rollback', { activity_id: row.id })
    toast.success(`已回滚：${row.issue_number} 退回 in_review`)
    confirmRollback.value = null
    await load()
  } catch (e: any) {
    toast.error('回滚失败: ' + e)
  } finally {
    rollbackBusy.value = false
  }
}

onMounted(() => {
  load()
  ensureNodes().catch(() => {})
})
// board-changed 推送：决策产生/回滚发生时静默换新。
useBoardChanged(() => load(true))
</script>

<template>
  <div>
    <div class="panel-toolbar">
      <select v-model="actionFilter" class="form-input filter-select" @change="load()">
        <option value="">全部决策</option>
        <option value="auto_decide">自动处置决策</option>
        <option value="auto_confirm_dispatch">拆解自动发车</option>
        <option value="merge_parked">合并停车</option>
      </select>
      <label class="rollback-toggle">
        <input type="checkbox" v-model="onlyRollback" />
        只看可回滚
      </label>
      <span class="muted">最近 {{ visibleRows.length }}/{{ rows.length }} 条 agent 自动决策（新→旧）</span>
    </div>

    <div v-if="loading" style="text-align: center; padding: var(--space-8);">
      <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
    </div>

    <div v-else-if="rows.length === 0" class="empty-state">
      <h3>暂无自动决策记录</h3>
      <p>开启「配置」页的自动化开关后，agent 的每一次自动处置（收货/重派/转人工/收口）都会流经这里，可在此一键回滚误判</p>
    </div>

    <div v-else-if="visibleRows.length === 0" class="empty-state">
      <h3>没有可回滚的决策</h3>
      <p>当前筛选下没有「验收 PASS · 自动收货」类决策（只有这类支持回滚）；取消勾选「只看可回滚」可查看全部决策</p>
    </div>

    <div v-else class="audit-list">
      <div v-for="row in visibleRows" :key="row.id" class="audit-item">
        <div class="audit-main">
          <span class="badge badge-info">{{ decisionLabel(row) }}</span>
          <strong class="issue-no">{{ row.issue_number }}</strong>
          <span class="issue-title">{{ row.issue_title }}</span>
          <span class="muted actor" :title="row.actor.id">{{ displayActor(row.actor.kind || 'agent', row.actor.id) }}</span>
          <span class="muted time">{{ fmtTime(row.created_at) }}</span>
              <button v-if="canRollback(row)" class="btn btn-sm btn-danger rollback-btn" @click="askRollback(row)">回滚</button>
              <button v-if="canRetryMerge(row)" class="btn btn-sm btn-primary rollback-btn" :disabled="retryBusy" @click="doRetryMerge(row)">重试合并</button>
        </div>
        <button
          v-if="parseDetails(row.details)"
          class="btn btn-sm expand-btn"
          @click="expanded[row.id] = !expanded[row.id]"
        >
          {{ expanded[row.id] ? '收起' : '详情' }}
        </button>
        <pre v-if="expanded[row.id] && parseDetails(row.details)" class="audit-details">{{
          JSON.stringify(parseDetails(row.details), null, 2)
        }}</pre>
      </div>
    </div>

    <!-- 回滚确认弹窗 -->
    <div v-if="confirmRollback" class="modal-backdrop" @click.self="confirmRollback = null">
      <div class="modal" style="max-width: 480px;">
        <div class="modal-header"><h3>确认回滚自动决策</h3></div>
        <div class="modal-body">
          <p>
            将撤销对 <strong>{{ confirmRollback.issue_number }} {{ confirmRollback.issue_title }}</strong>
            的自动处置「{{ decisionLabel(confirmRollback) }}」：
          </p>
          <ul>
            <li>仅当单据当前处于 <strong>done</strong>（已自动收货）时可回滚</li>
            <li>单据退回 <strong>in_review</strong> 等待重新处置，并留下系统评论</li>
            <li>已回滚过的决策不能再次回滚</li>
          </ul>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="confirmRollback = null">取消</button>
          <button class="btn btn-danger" :disabled="rollbackBusy" @click="doRollback">确认回滚</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.muted {
  color: var(--text-muted);
  font-size: var(--text-sm);
}
.panel-toolbar {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-4);
}
.filter-select {
  width: 200px;
}
.rollback-toggle {
  display: flex;
  align-items: center;
  gap: var(--space-1);
  font-size: var(--text-sm);
  cursor: pointer;
  user-select: none;
}
.audit-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.audit-item {
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-2) var(--space-3);
}
.audit-main {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}
.issue-no {
  font-family: monospace;
}
.issue-title {
  font-size: var(--text-sm);
}
.actor,
.time {
  margin-left: auto;
}
.time {
  margin-left: 0;
}
.rollback-btn {
  margin-left: var(--space-2);
}
.expand-btn {
  margin-top: var(--space-1);
}
.audit-details {
  margin-top: var(--space-2);
  background: var(--bg-tertiary, rgba(0, 0, 0, 0.2));
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  padding: var(--space-2) var(--space-3);
  font-size: var(--text-xs, 12px);
  overflow-x: auto;
}
</style>
