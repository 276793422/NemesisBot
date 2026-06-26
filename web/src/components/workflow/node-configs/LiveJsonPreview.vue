<script setup lang="ts">
/**
 * Read-only JSON preview of the current node config. Updates in real time
 * as the user edits form fields above. Power users can still glance here
 * to confirm the shape that will be persisted.
 */
import { computed } from 'vue'

const props = defineProps<{
  config: Record<string, unknown>
  /** When true, shows an empty-state message instead of `{}`. */
  emptyHint?: string
}>()

const pretty = computed(() => {
  const keys = Object.keys(props.config)
  if (keys.length === 0) return props.emptyHint ?? '{}'
  return JSON.stringify(props.config, null, 2)
})
</script>

<template>
  <div class="json-preview">
    <div class="json-preview-header">生成的 JSON（只读）</div>
    <pre class="json-preview-body">{{ pretty }}</pre>
  </div>
</template>

<style scoped>
.json-preview {
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

.json-preview-header {
  font-size: var(--text-xs);
  font-weight: 600;
  text-transform: uppercase;
  color: var(--text-secondary);
  padding: var(--space-1) var(--space-2);
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border);
}

.json-preview-body {
  margin: 0;
  padding: var(--space-2);
  font-family: 'Consolas', 'Courier New', monospace;
  font-size: var(--text-xs);
  color: var(--text-primary);
  max-height: 180px;
  overflow: auto;
  white-space: pre-wrap;
  word-break: break-all;
}
</style>
