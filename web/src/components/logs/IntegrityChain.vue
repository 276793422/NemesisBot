<script setup lang="ts">
import { ref, computed } from 'vue'
import { formatTime, type ChainSegment } from './mockData'

const props = defineProps<{
  segments: ChainSegment[]
  verifyResult?: {
    valid: boolean
    first_broken_index: number | null
    broken_count: number
    total_segments: number
  } | null
}>()

const emit = defineEmits<{
  (e: 'verify'): void
  (e: 'reload'): void
}>()

const selected = ref<ChainSegment | null>(null)
const verifying = computed(() => false) // parent manages loading state

const totalCount = computed(() => props.segments.length)
const brokenCount = computed(() => props.segments.filter(s => !s.valid).length)
const statusText = computed(() => {
  if (!props.verifyResult) return '尚未验证'
  if (props.verifyResult.valid) return '✅ 全部有效'
  return `⚠️ ${props.verifyResult.broken_count} 处断裂`
})

function verifyAll() {
  emit('verify')
}
</script>

<template>
  <div class="chain-view">
    <div class="chain-header">
      <div class="chain-status">
        <div class="status-label">审计链状态</div>
        <div class="status-value" :class="{ broken: brokenCount > 0 && verifyResult }">
          {{ statusText }}
        </div>
        <div class="status-meta">
          {{ totalCount }} 个链段
          <span v-if="verifyResult">（共 {{ verifyResult.total_segments }} 条事件）</span>
        </div>
      </div>
      <div class="chain-actions">
        <button
          class="btn btn-primary"
          @click="verifyAll"
        >
          🔍 验证全部完整性
        </button>
        <button class="btn btn-ghost" @click="emit('reload')">⟳ 刷新</button>
      </div>
    </div>

    <div class="chain-list scrollable">
      <div
        v-for="s in segments"
        :key="s.index"
        class="chain-row"
        :class="{ selected: selected && selected.index === s.index, broken: !s.valid }"
        @click="selected = s"
      >
        <div class="chain-index">#{{ s.index }}</div>
        <div class="chain-time">{{ formatTime(s.timestamp) }}</div>
        <div class="chain-hash">
          <span class="hash-label">hash</span>
          <code class="hash-value">{{ s.hash.slice(0, 16) }}...</code>
        </div>
        <div class="chain-prev">
          <span class="hash-label">prev</span>
          <code class="hash-value">{{ s.prevHash.slice(0, 16) }}...</code>
        </div>
        <div class="chain-status-icon">
          <span v-if="s.valid" class="valid">✅</span>
          <span v-else class="broken-icon" :title="s.breakReason">⚠️</span>
        </div>
        <div v-if="!s.valid" class="chain-break-reason">
          {{ s.breakReason }}
        </div>
      </div>
      <div v-if="segments.length === 0" class="empty-state">
        <h3>暂无审计链段</h3>
        <p>审计事件触发后将自动生成链段</p>
      </div>
    </div>

    <Transition name="slide">
      <div v-if="selected" class="chain-detail">
        <div class="detail-header">
          <h3>链段 #{{ selected.index }}</h3>
          <button class="btn btn-sm btn-ghost" @click="selected = null">✕</button>
        </div>
        <div class="detail-body scrollable">
          <div class="detail-row">
            <span>timestamp</span>
            <code>{{ selected.timestamp }}</code>
          </div>
          <div class="detail-row">
            <span>hash</span>
            <code class="hash-full">{{ selected.hash }}</code>
          </div>
          <div class="detail-row">
            <span>prev_hash</span>
            <code class="hash-full">{{ selected.prevHash }}</code>
          </div>

          <div class="detail-section">
            <div class="section-title">Payload Summary</div>
            <div class="section-body">{{ selected.payloadSummary }}</div>
          </div>

          <div class="detail-section">
            <div class="section-title">验证</div>
            <div class="section-body">
              <div class="verify-row">
                <span v-if="selected.valid">✅ hash 算法正确</span>
                <span v-else class="broken-text">❌ hash 不匹配</span>
              </div>
              <div class="verify-row">
                <span v-if="selected.valid">✅ prev_hash 链接 #{{ selected.index - 1 }}</span>
                <span v-else class="broken-text">❌ {{ selected.breakReason }}</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.chain-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-primary);
  position: relative;
}

.chain-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: var(--space-4) var(--space-5);
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-light);
}

.chain-status {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.status-label {
  font-size: var(--text-xs);
  color: var(--text-muted);
  text-transform: uppercase;
}

.status-value {
  font-size: var(--text-lg);
  font-weight: 600;
  color: #10b981;
}

.status-value.broken {
  color: #ef4444;
}

.status-meta {
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.chain-actions {
  display: flex;
  gap: var(--space-2);
}

.chain-list {
  flex: 1;
  padding: var(--space-2) 0;
}

.chain-row {
  display: grid;
  grid-template-columns: 60px 100px 1fr 1fr 50px;
  gap: var(--space-3);
  padding: var(--space-2) var(--space-4);
  align-items: center;
  font-size: var(--text-xs);
  cursor: pointer;
  border-bottom: 1px solid var(--border-light);
  font-family: monospace;
}

.chain-row:hover { background: var(--bg-hover); }

.chain-row.selected {
  background: var(--accent-muted);
}

.chain-row.broken {
  background: rgba(239, 68, 68, 0.05);
  grid-template-rows: auto auto;
}

.chain-index {
  font-weight: 600;
  color: var(--accent);
}

.chain-time {
  color: var(--text-muted);
}

.chain-hash, .chain-prev {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.hash-label {
  font-size: 10px;
  color: var(--text-muted);
  text-transform: uppercase;
}

.hash-value {
  color: var(--text-primary);
  word-break: break-all;
}

.chain-status-icon {
  text-align: center;
  font-size: var(--text-lg);
}

.chain-break-reason {
  grid-column: 1 / -1;
  color: #ef4444;
  padding: var(--space-1) var(--space-2);
  font-size: var(--text-xs);
  background: rgba(239, 68, 68, 0.08);
  border-radius: var(--radius-sm);
  margin-top: var(--space-1);
}

.chain-detail {
  position: absolute;
  top: 0;
  right: 0;
  bottom: 0;
  width: 480px;
  background: var(--bg-secondary);
  border-left: 1px solid var(--border-light);
  box-shadow: -4px 0 12px rgba(0,0,0,0.08);
  display: flex;
  flex-direction: column;
  z-index: 10;
}

.detail-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: var(--space-3) var(--space-4);
  border-bottom: 1px solid var(--border-light);
}

.detail-body {
  flex: 1;
  padding: var(--space-3) var(--space-4);
  overflow-y: auto;
}

.detail-row {
  display: flex;
  flex-direction: column;
  padding: var(--space-2) 0;
  border-bottom: 1px solid var(--border-light);
  font-size: var(--text-sm);
}

.detail-row span {
  color: var(--text-muted);
  text-transform: uppercase;
  font-size: var(--text-xs);
  margin-bottom: var(--space-1);
}

.hash-full {
  font-family: monospace;
  font-size: var(--text-xs);
  color: var(--text-primary);
  background: var(--bg-tertiary);
  padding: var(--space-2);
  border-radius: var(--radius-sm);
  word-break: break-all;
}

.detail-section {
  margin-top: var(--space-3);
}

.section-title {
  font-size: var(--text-xs);
  font-weight: 600;
  color: var(--text-muted);
  text-transform: uppercase;
  margin-bottom: var(--space-2);
}

.section-body {
  font-size: var(--text-sm);
  color: var(--text-primary);
  background: var(--bg-primary);
  padding: var(--space-3);
  border-radius: var(--radius-md);
}

.verify-row {
  padding: var(--space-1) 0;
}

.broken-text {
  color: #ef4444;
  font-weight: 600;
}

.slide-enter-active, .slide-leave-active {
  transition: transform 0.2s ease;
}

.slide-enter-from, .slide-leave-to {
  transform: translateX(100%);
}

.scrollable { overflow-y: auto; }
</style>
