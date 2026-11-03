<script setup lang="ts">
import { computed } from 'vue'
import type { PanelSlotState, PanelViewDecl } from '../../composables/usePanels'

// 时间线块（v0 词汇）：guest 回包 `{events: [{title, detail, ts_ms, state}]}`。
// state → 语义色（done/ok→success、fail/error→error、其余→muted）。
const props = defineProps<{ decl: PanelViewDecl; state: PanelSlotState }>()
defineEmits<{ (e: 'refresh'): void }>()

interface TimelineEvent {
  title?: string
  detail?: string
  ts_ms?: number
  state?: string
}

const events = computed<TimelineEvent[]>(() => {
  const d = props.state.data
  if (Array.isArray(d?.events)) return d.events
  if (Array.isArray(d)) return d
  return []
})

function stateClass(e: TimelineEvent): string {
  const s = (e.state ?? '').toLowerCase()
  if (s === 'done' || s === 'ok' || s === 'success') return 'good'
  if (s === 'fail' || s === 'error' || s === 'failed') return 'bad'
  if (s === 'pending' || s === 'waiting' || s === 'running') return 'wait'
  return ''
}

function fmtTime(ts?: number): string {
  if (!ts) return ''
  try {
    return new Date(ts).toLocaleString()
  } catch {
    return String(ts)
  }
}
</script>

<template>
  <section class="panel-block">
    <header class="panel-block-head">
      <h3>时间线</h3>
      <button class="panel-mini" title="刷新" @click="$emit('refresh')">⟳</button>
    </header>
    <div v-if="state.loading" class="panel-note">加载中…</div>
    <div v-else-if="state.error" class="panel-error">{{ state.error }}</div>
    <div v-else-if="!events.length" class="panel-note">暂无事件</div>
    <ol v-else class="panel-timeline">
      <li v-for="(ev, i) in events" :key="i" :class="stateClass(ev)">
        <div class="ev-head">
          <span class="ev-title">{{ ev.title ?? `#${i + 1}` }}</span>
          <span v-if="ev.ts_ms" class="ev-ts">{{ fmtTime(ev.ts_ms) }}</span>
        </div>
        <div v-if="ev.detail" class="ev-detail">{{ ev.detail }}</div>
      </li>
    </ol>
  </section>
</template>

<style scoped>
.panel-block {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: var(--space-3);
  margin-bottom: var(--space-3);
}
.panel-block-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: var(--space-2);
}
.panel-block-head h3 {
  margin: 0;
  font-size: var(--text-sm);
  color: var(--text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
.panel-mini {
  background: transparent;
  border: 1px solid var(--border);
  color: var(--text-muted);
  border-radius: 6px;
  padding: 2px 8px;
  cursor: pointer;
  font-size: var(--text-xs);
}
.panel-mini:hover {
  color: var(--text-secondary);
  border-color: var(--text-muted);
}
.panel-note {
  color: var(--text-muted);
  font-size: var(--text-xs);
  padding: var(--space-2) 0;
}
.panel-error {
  color: var(--error);
  font-size: var(--text-xs);
  padding: var(--space-2) 0;
  white-space: pre-wrap;
}
.panel-timeline {
  list-style: none;
  margin: 0;
  padding: 0;
  font-size: var(--text-xs);
}
.panel-timeline li {
  position: relative;
  padding: 0 0 var(--space-3) 18px;
  border-left: 1px solid var(--border);
  margin-left: 6px;
}
.panel-timeline li:last-child {
  padding-bottom: 0;
}
.panel-timeline li::before {
  content: '';
  position: absolute;
  left: -4.5px;
  top: 2px;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--text-muted);
}
.panel-timeline li.good::before {
  background: var(--success);
}
.panel-timeline li.bad::before {
  background: var(--error);
}
.panel-timeline li.wait::before {
  background: var(--warning);
}
.ev-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--space-2);
}
.ev-title {
  color: var(--text-primary);
  font-size: var(--text-xs);
}
.ev-ts {
  color: var(--text-muted);
  font-size: var(--text-xs);
  white-space: nowrap;
}
.ev-detail {
  color: var(--text-secondary);
  margin-top: 2px;
  white-space: pre-wrap;
  word-break: break-word;
}
</style>
