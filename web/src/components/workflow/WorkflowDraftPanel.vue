<script setup lang="ts">
/**
 * WorkflowDraftPanel — 对话生成的草稿面板。
 *
 * 列出 drafts/ 目录下的全部待应用草稿（后端 draft_list，mtime 降序），
 * 每条提供三个动作：
 *   - 画布预览：draft_get → store.enterDraftPreview（画布第三数据态，
 *     只读渲染 + diff 高亮，palette/交互禁用）
 *   - ✓ 应用：draft_apply（唯一转正入口；同名已注册定义会被备份到
 *     .history/ 后替换——应用前显式确认）
 *   - ✕ 放弃：draft_discard（幂等；确认后丢弃）
 *
 * 刷新时机：TAB 进入 / 目标切换（父组件触发）/ tool_event 提醒（父组件
 * 监听 workflow_create 工具完成后触发）。
 */
import { onMounted } from 'vue'
import { storeToRefs } from 'pinia'
import { useWorkflowStore } from '../../stores/workflow'
import { useToast } from '../../composables/useToast'
import { computeDraftDiff } from '../../composables/wfEditSessions'
import type { WorkflowDef } from '../../types/workflow'

const store = useWorkflowStore()
const { drafts, draftsLoading, draftsError } = storeToRefs(store)
const toast = useToast()

// C（2026-09-23）：应用成功回抛宿主——__new__ 引导会话原地重绑到正式
// 工作流名（WorkflowAgentGen.onDraftApplied）。
const emit = defineEmits<{ applied: [name: string] }>()

onMounted(() => {
  void store.fetchDrafts(true)
})

function formatTime(ms: number): string {
  if (!ms) return ''
  const d = new Date(ms)
  return `${d.getMonth() + 1}/${d.getDate()} ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

/** 同名正式定义是否已存在（应用会替换它——UI 提示依据，与后端备份无关）。 */
function willReplace(name: string): boolean {
  return store.workflowByName[name] !== undefined
}

async function preview(name: string) {
  try {
    const detail = await store.api.draftGet(name)
    // 同名正式定义存在 → 拉全量定义算节点级 diff（画布 added/removed/changed 高亮）
    let existing: WorkflowDef | null = null
    if (store.workflowByName[name]) {
      try {
        existing = (await store.api.get(name)).workflow
      } catch {
        existing = null // 旧定义读取失败：退化为「全部新增」视角，不阻断预览
      }
    }
    store.enterDraftPreview(detail, computeDraftDiff(detail.workflow, existing))
  } catch (e) {
    toast.error(typeof e === 'string' ? e : '读取草稿失败')
  }
}

async function apply(name: string) {
  const replace = willReplace(name)
  if (
    replace &&
    !window.confirm(
      `工作流「${name}」已有正式定义。\n\n应用草稿将替换它（旧定义自动备份到 .history/，可在文件系统找回）。\n\n确定应用？`,
    )
  ) {
    return
  }
  const res = await store.applyDraft(name)
  if (res.ok) {
    toast.success(
      res.replaced
        ? `草稿已应用，工作流「${name}」已更新（旧定义已备份）`
        : `草稿已应用，工作流「${name}」已注册`,
    )
    emit('applied', name)
  } else {
    toast.error(res.error)
  }
}

async function discard(name: string) {
  if (!window.confirm(`确定丢弃草稿「${name}」？（不可恢复）`)) return
  const res = await store.discardDraft(name)
  if (res.ok) toast.info(`草稿「${name}」已丢弃`)
  else toast.error(res.error)
}
</script>

<template>
  <div class="draft-panel">
    <div class="panel-head">
      <h3>待应用草稿</h3>
      <span v-if="drafts.length" class="count-chip">{{ drafts.length }}</span>
      <button class="mini-btn" title="刷新草稿列表" @click="store.fetchDrafts(true)">⟳</button>
    </div>

    <div v-if="draftsError" class="panel-error">{{ draftsError }}</div>
    <div v-else-if="draftsLoading && drafts.length === 0" class="panel-empty">⟳ 加载草稿...</div>
    <div v-else-if="drafts.length === 0" class="panel-empty">
      暂无草稿。<br />
      在左侧对话里描述你想要的工作流，AI 生成的定义会先落在这里，确认后再应用。
    </div>

    <ul v-else class="draft-list">
      <li v-for="d in drafts" :key="d.file_stem" class="draft-item" :class="{ invalid: !d.valid }">
        <div class="draft-line-1">
          <span class="draft-name" :title="d.name">{{ d.name }}</span>
          <span v-if="d.valid" class="badge ok">可应用</span>
          <span v-else class="badge bad">校验失败</span>
        </div>
        <div class="draft-line-2">
          <span>{{ d.node_count }} 节点</span>
          <span v-for="t in d.trigger_types" :key="t" class="trigger-chip">{{ t }}</span>
          <span class="draft-time">{{ formatTime(d.mtime_ms) }}</span>
        </div>
        <ul v-if="!d.valid" class="draft-errors">
          <li v-for="(err, i) in d.validation_errors" :key="i">{{ err }}</li>
        </ul>
        <div v-if="willReplace(d.name)" class="replace-hint">⚠ 同名正式定义已存在，应用将替换（旧定义备份）</div>
        <div class="draft-actions">
          <button class="act preview" :disabled="!d.valid" title="在画布中只读预览此草稿" @click="preview(d.name)">
            画布预览
          </button>
          <button class="act apply" :disabled="!d.valid" title="应用草稿（转正）" @click="apply(d.name)">
            ✓ 应用
          </button>
          <button class="act discard" title="丢弃草稿" @click="discard(d.name)">✕ 放弃</button>
        </div>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.draft-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}

.panel-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) 0;
}

.panel-head h3 {
  margin: 0;
  font-size: var(--text-sm);
  font-weight: 600;
}

.count-chip {
  font-size: var(--text-xs);
  color: var(--accent);
  background: color-mix(in srgb, var(--accent, #4f8cff) 12%, transparent);
  border-radius: 8px;
  padding: 0 6px;
}

.mini-btn {
  margin-left: auto;
  border: 1px solid var(--border);
  background: transparent;
  color: var(--text-secondary);
  border-radius: 6px;
  padding: 2px 8px;
  cursor: pointer;
}
.mini-btn:hover {
  color: var(--text-primary);
  border-color: var(--text-muted);
}

.panel-empty,
.panel-error {
  padding: var(--space-4);
  font-size: var(--text-xs);
  color: var(--text-muted);
  text-align: center;
  line-height: 1.8;
}
.panel-error {
  color: var(--danger, #e74c3c);
}

.draft-list {
  list-style: none;
  margin: 0;
  padding: 0;
  overflow-y: auto;
  flex: 1;
}

.draft-item {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: var(--space-2) var(--space-3);
  margin-bottom: var(--space-2);
}
.draft-item.invalid {
  border-color: color-mix(in srgb, var(--danger, #e74c3c) 45%, var(--border));
}

.draft-line-1 {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.draft-name {
  font-weight: 600;
  font-size: var(--text-sm);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.badge {
  font-size: var(--text-xs);
  border-radius: 8px;
  padding: 0 6px;
}
.badge.ok {
  color: var(--success, #2ecc71);
  background: color-mix(in srgb, var(--success, #2ecc71) 12%, transparent);
}
.badge.bad {
  color: var(--danger, #e74c3c);
  background: color-mix(in srgb, var(--danger, #e74c3c) 12%, transparent);
}

.draft-line-2 {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-top: 2px;
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.trigger-chip {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 0 6px;
}

.draft-time {
  margin-left: auto;
}

.draft-errors {
  margin: var(--space-1) 0 0;
  padding-left: 18px;
  font-size: var(--text-xs);
  color: var(--danger, #e74c3c);
}

.replace-hint {
  margin-top: var(--space-1);
  font-size: var(--text-xs);
  color: var(--warning, #f39c12);
}

.draft-actions {
  display: flex;
  gap: var(--space-2);
  margin-top: var(--space-2);
}

.act {
  font-size: var(--text-xs);
  border-radius: 6px;
  padding: 3px 10px;
  cursor: pointer;
  border: 1px solid var(--border);
  background: transparent;
  color: var(--text-secondary);
}
.act:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
.act.preview:hover:not(:disabled) {
  border-color: var(--accent);
  color: var(--accent);
}
.act.apply {
  background: var(--accent, #4f8cff);
  border-color: var(--accent, #4f8cff);
  color: #fff;
}
.act.apply:hover:not(:disabled) {
  filter: brightness(1.08);
}
.act.discard:hover {
  border-color: var(--danger, #e74c3c);
  color: var(--danger, #e74c3c);
}
</style>
