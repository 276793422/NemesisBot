<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { useBoardChanged } from '../../composables/useBoardChanged'
import { PRIORITY_LABEL, PRIORITY_BADGE, STATUS_BADGE, statusLabel } from './boardMeta'

// 「需要人的事」（B35 注意力经济）：聚合面只显示需要人的信号，四组泳道。
// 数据源 = board.needs_human.list（一次返回，避免四处信号四连发拉全量）：
// - escalated 待裁决：停车档案在场（✋）或 🤷 评审转人工
// - fused 冻结：🛑 重派熔断停车
// - plan_failed 拆解失败：planner 链耗尽，可手动重拆
// - shadow 影子待 promote：影子规则已触发过（B34），确认行为后转全真
// 终态（done/cancelled）单后端已排除；空态 = 无需人工介入。

const { request } = useWSAPI()
const toast = useToast()

interface IssueRow {
  id: number
  number: string
  title: string
  status: string
  priority: number
  reason: string
  needs_user: {
    stop_kind_label: string
    suggested_action: string
    hint: string
    stopped_at: string | null
  } | null
}

interface ShadowRule {
  id: number
  name: string
  title: string
  cron: string
  last_run_at: number | null
}

interface NeedsHuman {
  groups: {
    escalated: IssueRow[]
    fused: IssueRow[]
    plan_failed: IssueRow[]
    shadow: ShadowRule[]
  }
  total: number
}

const loading = ref(true)
const data = ref<NeedsHuman | null>(null)
const promoting = ref(false)

async function load(silent = false) {
  if (!silent) loading.value = true
  try {
    data.value = await request('board', 'board.needs_human.list', {})
  } catch (e: any) {
    if (silent) console.warn('[BoardNeedsHumanPanel] silent refresh failed:', e)
    else toast.error('加载需要人的事失败: ' + e)
  } finally {
    loading.value = false
  }
}

async function promote(rule: ShadowRule) {
  promoting.value = true
  try {
    await request('board', 'autopilot.update', { id: rule.id, mode: 'live' })
    toast.success(`规则「${rule.name}」已转全真`)
    await load()
  } catch (e: any) {
    toast.error('转全真失败: ' + e)
  } finally {
    promoting.value = false
  }
}

const GROUP_META: Record<string, { label: string; icon: string; hint: string }> = {
  escalated: { label: '待裁决', icon: '✋', hint: '停车档案在场或验收转人工，需要人下结论' },
  fused: { label: '冻结', icon: '🛑', hint: '重派熔断停车，转人工处置' },
  plan_failed: { label: '拆解失败', icon: '🧩', hint: 'planner 链耗尽，可手动重拆或人工建子单' },
  shadow: { label: '影子待转正', icon: '🎭', hint: '影子模式规则已触发过，确认行为符合预期后转全真' },
}

onMounted(load)
// board-changed 推送（评审落评论/autopilot fire 等后端改动）时静默换新。
useBoardChanged(() => load(true))
</script>

<template>
  <div>
    <div v-if="loading" style="text-align: center; padding: var(--space-8);">
      <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
    </div>

    <div
      v-else-if="!data || data.total === 0"
      class="empty-state"
      data-test="needs-human-empty"
    >
      <h3>无需人工介入</h3>
      <p>所有在跑任务都按计划流转；需要人的信号（待裁决/冻结/拆解失败/影子待转正）会出现在这里</p>
    </div>

    <div v-else class="lanes" data-test="needs-human-lanes">
      <div
        v-for="(meta, key) in GROUP_META"
        :key="key"
        class="lane"
        :data-test="'lane-' + key"
      >
        <div class="lane-header">
          <span class="lane-title">{{ meta.icon }} {{ meta.label }}</span>
          <span class="badge badge-neutral">{{ data.groups[key as keyof NeedsHuman['groups']]?.length || 0 }}</span>
        </div>
        <div class="muted lane-hint">{{ meta.hint }}</div>
        <div
          v-if="(data.groups[key as keyof NeedsHuman['groups']]?.length || 0) === 0"
          class="muted lane-empty"
        >
          — 无 —
        </div>
        <template v-else-if="key === 'shadow'">
          <div
            v-for="rule in data.groups.shadow"
            :key="'sh-' + rule.id"
            class="lane-item"
          >
            <div class="item-main">
              <strong>{{ rule.name }}</strong>
              <span class="muted item-sub">{{ rule.title }} · <code>{{ rule.cron }}</code></span>
            </div>
            <button
              class="btn btn-sm btn-primary"
              :disabled="promoting"
              data-test="promote-btn"
              @click="promote(rule)"
            >
              转全真
            </button>
          </div>
        </template>
        <template v-else>
          <div
            v-for="row in data.groups[key as 'escalated' | 'fused' | 'plan_failed']"
            :key="key + '-' + row.id"
            class="lane-item"
          >
            <div class="item-main">
              <div>
                <span class="badge" :class="STATUS_BADGE[row.status] || 'badge-neutral'">{{ statusLabel(row.status) }}</span>
                <strong style="margin-left: var(--space-1);">{{ row.number }}</strong>
                <span style="margin-left: var(--space-1);">{{ row.title }}</span>
                <span class="badge" :class="PRIORITY_BADGE[row.priority] || 'badge-neutral'" style="margin-left: var(--space-1);">
                  {{ PRIORITY_LABEL[row.priority] ?? row.priority }}
                </span>
              </div>
              <div v-if="row.needs_user" class="muted item-sub">
                {{ row.needs_user.stop_kind_label }} · 建议：{{ row.needs_user.suggested_action }}
                <template v-if="row.needs_user.hint">（{{ row.needs_user.hint }}）</template>
              </div>
              <div v-else-if="row.reason" class="muted item-sub">{{ row.reason }}</div>
            </div>
          </div>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.muted {
  color: var(--text-muted);
  font-size: var(--text-sm);
}
.lanes {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
  gap: var(--space-3);
}
.lane {
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-3);
  min-height: 120px;
}
.lane-header {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}
.lane-title {
  font-weight: 600;
}
.lane-hint {
  margin-top: var(--space-1);
  margin-bottom: var(--space-2);
}
.lane-empty {
  padding: var(--space-2) 0;
}
.lane-item {
  display: flex;
  align-items: flex-start;
  gap: var(--space-2);
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  padding: var(--space-2);
  margin-bottom: var(--space-2);
}
.item-main {
  flex: 1;
  min-width: 0;
}
.item-sub {
  margin-top: var(--space-1);
  word-break: break-all;
}
</style>
