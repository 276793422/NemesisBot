<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'

// G3：spill 状态卡。工具输出落盘（`<home>/logs/spill`）的只读聚合 +
// 「立即清理」按钮（按配置保留期跑一次 retention sweep）。

const { request } = useWSAPI()

const status = ref<any>(null)
const loading = ref(false)
const cleaning = ref(false)
const lastCleanup = ref<string | null>(null)

async function refresh() {
  loading.value = true
  try {
    status.value = await request('logs', 'spill_status', {})
  } catch {
    status.value = null
  } finally {
    loading.value = false
  }
}

async function cleanup() {
  cleaning.value = true
  try {
    const res = await request('logs', 'spill_cleanup', {})
    if (res?.retention_days === 0) {
      // 保留期 0 = 清理禁用：如实说明，不谎报「已清理 0 个文件」。
      lastCleanup.value = '保留期未启用（retention_days=0），未执行清理'
    } else {
      lastCleanup.value = `已清理 ${res?.deleted ?? 0} 个文件`
    }
    // cleanup 返回了清理后的新状态，直接采用。
    if (res) status.value = res
  } catch {
    lastCleanup.value = '清理失败'
  } finally {
    cleaning.value = false
  }
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(2)} MB`
}

function formatOldest(iso: string | null): string {
  if (!iso) return ''
  return iso.slice(0, 19).replace('T', ' ')
}

onMounted(refresh)
</script>

<template>
  <div class="spill-card">
    <span v-if="loading" class="spill-hint">⟳ 读取 spill 状态…</span>
    <template v-else-if="status">
      <span class="spill-title">💾 工具输出落盘</span>
      <template v-if="status.files > 0">
        <span class="spill-stat">{{ status.files }} 个文件</span>
        <span class="spill-stat">{{ formatBytes(status.bytes) }}</span>
        <span v-if="status.oldest" class="spill-stat spill-muted">最早 {{ formatOldest(status.oldest) }}</span>
      </template>
      <span v-else class="spill-stat spill-muted">暂无落盘文件</span>
      <span class="spill-stat spill-muted">保留 {{ status.retention_days }} 天</span>
      <span v-if="lastCleanup" class="spill-result">{{ lastCleanup }}</span>
      <button
        class="spill-clean-btn"
        :disabled="cleaning || status.files === 0 || status.retention_days === 0"
        :title="status.retention_days === 0 ? '保留期未启用（retention_days=0），清理已禁用' : '按保留期立即清理过期落盘文件'"
        @click="cleanup"
      >
        {{ cleaning ? '清理中…' : '立即清理' }}
      </button>
    </template>
    <span v-else class="spill-hint">spill 状态不可用</span>
  </div>
</template>

<style scoped>
.spill-card {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-1_5) var(--space-4);
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-light);
  font-size: var(--text-xs);
  color: var(--text-secondary);
}

.spill-title { font-weight: 600; color: var(--text-primary); }

.spill-stat { white-space: nowrap; }

.spill-muted { color: var(--text-muted); }

.spill-hint { color: var(--text-muted); }

.spill-result { color: var(--accent); white-space: nowrap; }

.spill-clean-btn {
  margin-left: auto;
  padding: 2px 10px;
  border: 1px solid var(--border-light);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  color: var(--text-secondary);
  cursor: pointer;
  font-size: var(--text-xs);
}

.spill-clean-btn:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.spill-clean-btn:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
