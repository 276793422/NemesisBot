<script setup lang="ts">
/**
 * ToolCallCard — M1b（2026-09-05，devtool-upgrade 阶段 3）。
 *
 * 单条工具调用卡片：工具名 / 参数摘要（首行 ≤120 字）/ 状态
 * （running 旋转 → ok ✓ / error ✗ 徽标）/ 耗时 / 结果预览折叠（默认收起）。
 * 数据来自 M1a AgentEvent 通道（WS push `tool_event` 帧），由 ChatPanel
 * 收集进 chat store 后按消息聚合传入。
 * 点击卡片展开/收起结果预览（无结果预览时点击无效果）。
 */
import { computed, ref } from 'vue'
import type { ToolEvent } from '../../stores/chat'

const props = defineProps<{
  event: ToolEvent
}>()

const expanded = ref(false)

/** 参数摘要：首行、≤120 字符（按 Unicode 码点截断，避免劈开代理对）。 */
const summary = computed(() => {
  const raw = (props.event.argsPreview ?? '').trim()
  if (!raw) return ''
  const firstLine = raw.split('\n', 1)[0] ?? raw
  const chars = [...firstLine]
  return chars.length > 120 ? chars.slice(0, 120).join('') + '…' : firstLine
})

/** 耗时显示：≥1s 显示小数秒，否则毫秒。 */
const durationText = computed(() => {
  const ms = props.event.durationMs
  if (ms == null) return ''
  return ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${ms}ms`
})

function onToggle() {
  if (props.event.resultPreview) expanded.value = !expanded.value
}
</script>

<template>
  <div
    class="tool-card"
    :class="[`is-${event.state}`, { 'is-expandable': !!event.resultPreview, 'is-expanded': expanded }]"
    @click="onToggle"
  >
    <div class="tool-card-row">
      <span class="tool-state" aria-hidden="true">
        <span v-if="event.state === 'running'" class="tool-spinner" />
        <template v-else>{{ event.state === 'ok' ? '✓' : '✗' }}</template>
      </span>
      <span class="tool-name">{{ event.tool }}</span>
      <span v-if="summary" class="tool-args">{{ summary }}</span>
      <span v-if="durationText" class="tool-duration">{{ durationText }}</span>
    </div>
    <pre v-if="expanded && event.resultPreview" class="tool-result">{{ event.resultPreview }}</pre>
  </div>
</template>

<style scoped>
.tool-card {
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg-elev, rgba(128, 128, 128, 0.06));
  font-size: var(--text-xs, 12px);
  overflow: hidden;
}

.tool-card.is-expandable {
  cursor: pointer;
}

.tool-card.is-error {
  border-color: var(--danger, #c66);
}

.tool-card-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 8px;
  min-width: 0;
}

.tool-state {
  flex: none;
  width: 14px;
  text-align: center;
  color: var(--text-muted);
}

.tool-card.is-ok .tool-state {
  color: var(--accent, #4a9);
}

.tool-card.is-error .tool-state {
  color: var(--danger, #c66);
}

.tool-spinner {
  display: inline-block;
  width: 10px;
  height: 10px;
  border: 2px solid var(--text-muted);
  border-top-color: transparent;
  border-radius: 50%;
  animation: tool-card-spin 0.8s linear infinite;
}

@keyframes tool-card-spin {
  to {
    transform: rotate(360deg);
  }
}

.tool-name {
  flex: none;
  font-family: var(--font-mono, monospace);
  font-weight: 600;
}

.tool-args {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-muted);
  font-family: var(--font-mono, monospace);
}

.tool-duration {
  flex: none;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}

.tool-result {
  margin: 0;
  padding: 6px 8px;
  border-top: 1px solid var(--border);
  max-height: 200px;
  overflow: auto;
  white-space: pre-wrap;
  word-break: break-word;
  font-family: var(--font-mono, monospace);
  font-size: var(--text-xs, 12px);
  color: var(--text-muted);
}
</style>
