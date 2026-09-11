<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { useBoardChanged } from '../../composables/useBoardChanged'
import { fmtTime } from './boardMeta'

// 决策流审计面板（全自动流转 P5/E2）：agent 自动决策的时间倒序流水。
// 每行 = 一条 `auto_decide` 活动（决策词 + verdict + details JSON 展开），
// 对已自动收货（done）的决策可一键回滚（单据退回 in_review，防重由后端
// 仲裁）。后端唯一真相源：crates/nemesis-web/src/handlers/board.rs
// （board.audit.list / board.audit.rollback）+ nemesis-board store。

const { request } = useWSAPI()
const toast = useToast()

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

// 决策词 → 人话标签（与 board_review.rs 8 处处置臂 + A1 auto_confirm 对齐）。
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
  return DECISION_LABEL[key] ?? key ?? row.action
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

onMounted(load)
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
      </select>
      <span class="muted">最近 {{ rows.length }} 条 agent 自动决策（新→旧）</span>
    </div>

    <div v-if="loading" style="text-align: center; padding: var(--space-8);">
      <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
    </div>

    <div v-else-if="rows.length === 0" class="empty-state">
      <h3>暂无自动决策记录</h3>
      <p>开启「配置」页的自动化开关后，agent 的每一次自动处置（收货/重派/转人工/收口）都会流经这里，可在此一键回滚误判</p>
    </div>

    <div v-else class="audit-list">
      <div v-for="row in rows" :key="row.id" class="audit-item">
        <div class="audit-main">
          <span class="badge badge-info">{{ decisionLabel(row) }}</span>
          <strong class="issue-no">{{ row.issue_number }}</strong>
          <span class="issue-title">{{ row.issue_title }}</span>
          <span class="muted actor">@{{ row.actor.id }}</span>
          <span class="muted time">{{ fmtTime(row.created_at) }}</span>
          <button class="btn btn-sm btn-danger rollback-btn" @click="askRollback(row)">回滚</button>
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
