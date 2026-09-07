<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import hljs from 'highlight.js/lib/core'
import diff from 'highlight.js/lib/languages/diff'
import { useWSAPI } from '../../composables/useWSAPI'

hljs.registerLanguage('diff', diff)

// M3：会话变更面板 —— 聚合 session_detail 行里的 file_changes
// （文件 → 次数/kind 徽标），点击文件拉取「最早 checkpoint 基线 vs 现
// 盘」unified diff（hljs diff 渲染）。聚合纯前端（file_changes 由 D3
// 透传），diff 走 sessions.file_diff。
const props = defineProps<{ session: string; messages: any[] }>()

const { request } = useWSAPI()

const expanded = ref(false)
const selectedPath = ref<string | null>(null)
const diffLoading = ref(false)
const diffText = ref('')
const diffNote = ref('')
const diffError = ref('')

interface ChangeAgg {
  path: string
  count: number
  kinds: string[]
}

const changes = computed<ChangeAgg[]>(() => {
  const map = new Map<string, ChangeAgg>()
  for (const m of (props.messages ?? []) as any[]) {
    for (const fc of (m?.file_changes ?? []) as any[]) {
      if (!fc?.path) continue
      const cur: ChangeAgg = map.get(fc.path) ?? { path: fc.path, count: 0, kinds: [] }
      cur.count++
      if (fc.kind && !cur.kinds.includes(fc.kind)) cur.kinds.push(fc.kind)
      map.set(fc.path, cur)
    }
  }
  return [...map.values()].sort((a, b) => b.count - a.count)
})

const totalChanges = computed(() => changes.value.reduce((s, c) => s + c.count, 0))

function kindBadge(kind: string): string {
  if (kind === 'Create') return '＋'
  if (kind === 'Delete') return '－'
  return '✎'
}

async function showDiff(path: string) {
  selectedPath.value = path
  diffLoading.value = true
  diffError.value = ''
  diffText.value = ''
  diffNote.value = ''
  try {
    const res = await request('sessions', 'file_diff', {
      session_id: props.session,
      path,
    })
    diffText.value = res?.diff ?? ''
    diffNote.value = res?.note ?? ''
  } catch (e: any) {
    diffError.value = String(e?.message ?? e)
  } finally {
    diffLoading.value = false
  }
}

const diffHtml = computed(() => {
  if (!diffText.value) return ''
  try {
    return hljs.highlight(diffText.value, { language: 'diff' }).value
  } catch {
    return diffText.value
  }
})

// 会话切换：清选中态与 diff（聚合随 props 自动重算）。
watch(
  () => props.session,
  () => {
    selectedPath.value = null
    diffText.value = ''
    diffNote.value = ''
    diffError.value = ''
  },
)
</script>

<template>
  <div v-if="changes.length > 0" class="changes-panel">
    <button class="changes-toggle" @click="expanded = !expanded">
      <span class="chevron">{{ expanded ? '▾' : '▸' }}</span>
      📝 会话变更（{{ changes.length }} 文件 / {{ totalChanges }} 次）
    </button>
    <div v-if="expanded" class="changes-body">
      <div class="change-files">
        <button
          v-for="c in changes"
          :key="c.path"
          class="change-file"
          :class="{ active: selectedPath === c.path }"
          :title="c.kinds.join(', ')"
          @click="showDiff(c.path)"
        >
          <span class="cf-badges">
            <span v-for="k in c.kinds" :key="k" class="cf-kind">{{ kindBadge(k) }}</span>
          </span>
          <span class="cf-path">{{ c.path }}</span>
          <span class="cf-count">×{{ c.count }}</span>
        </button>
      </div>
      <div v-if="selectedPath" class="change-diff">
        <div v-if="diffLoading" class="diff-hint">⟳ 加载 diff...</div>
        <div v-else-if="diffError" class="diff-hint diff-error">{{ diffError }}</div>
        <template v-else>
          <div v-if="diffNote" class="diff-hint">{{ diffNote }}</div>
          <pre v-if="diffText" class="diff-code"><code v-html="diffHtml"></code></pre>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.changes-panel {
  margin: 0 var(--space-4) var(--space-2);
  border: 1px solid var(--border-light);
  border-radius: var(--radius-md);
  background: var(--bg-secondary);
}

.changes-toggle {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: var(--space-2) var(--space-3);
  background: transparent;
  border: none;
  color: var(--text-primary);
  font-size: var(--text-sm);
  cursor: pointer;
  text-align: left;
}

.chevron {
  color: var(--text-muted);
  width: 12px;
}

.changes-body {
  padding: 0 var(--space-3) var(--space-3);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.change-files {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-1);
}

.change-file {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border: 1px solid var(--border-light);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  color: var(--text-secondary);
  font-size: var(--text-xs);
  cursor: pointer;
  max-width: 100%;
}

.change-file:hover { border-color: var(--accent); }
.change-file.active {
  border-color: var(--accent);
  color: var(--accent);
}

.cf-kind {
  color: var(--warning);
  font-weight: 600;
}

.cf-path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cf-count {
  color: var(--text-muted);
}

.change-diff {
  border: 1px solid var(--border-light);
  border-radius: var(--radius-sm);
  overflow: auto;
  max-height: 320px;
}

.diff-hint {
  padding: var(--space-2) var(--space-3);
  color: var(--text-muted);
  font-size: var(--text-xs);
}

.diff-error { color: var(--danger); }

.diff-code {
  margin: 0;
  padding: var(--space-2) var(--space-3);
  font-size: var(--text-xs);
  line-height: 1.5;
  background: var(--bg-primary);
}

.diff-code :deep(.hljs-deletion) {
  color: var(--danger);
  background: rgba(239, 68, 68, 0.08);
  display: inline-block;
  width: 100%;
}

.diff-code :deep(.hljs-addition) {
  color: var(--success);
  background: rgba(34, 197, 94, 0.08);
  display: inline-block;
  width: 100%;
}

.diff-code :deep(.hljs-meta) { color: var(--text-muted); }
</style>
