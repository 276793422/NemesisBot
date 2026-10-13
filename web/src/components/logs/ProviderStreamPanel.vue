<script setup lang="ts">
// L10（2026-10-13 内部专项 T3）：provider 流式输出观测面板。
// 消费 'provider_stream_event' SSE 帧（ephemeral——不参与断线补拉，本面板
// 只呈现「此刻正在发生的流」）。每会话一张流卡片：
// - start  帧 → 重置该会话卡片（新一轮 LLM 调用的信号，seq 归零重计）
// - delta  帧 → 追加文本（累计上限截断，防 DOM 无界膨胀）
// - end    帧 → 冻结（显示耗时）；纯工具轮（无输出流）不产生任何帧
// - error  帧 → 终态（错误文案入卡）
// 卡片上限 8 张：新会话到来时最老的「已结束」卡片先淘汰，直播中的不淘汰。
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { on as sseOn, off as sseOff, connectEvents } from '../../composables/useSSE'

interface StreamCard {
  sessionId: string
  model: string
  text: string
  phase: 'streaming' | 'end' | 'error'
  seq: number
  elapsedMs: number | null
  chars: number
  truncated: boolean
  updatedAt: number
  startAt: number
}

// 单卡文本渲染上限（字符）：超出部分不再追进 DOM（chars 仍真实累计，
// 截断注记可见）——delta 洪泛下浏览器主线程是稀缺资源。
const MAX_TEXT_CHARS = 40_000
// 卡片上限：新会话到来时淘汰最老的已结束卡片
const MAX_CARDS = 8

const cards = ref<Map<string, StreamCard>>(new Map())
const collapsed = ref(false)

const cardList = computed(() => Array.from(cards.value.values()))

const liveCount = computed(() =>
  cardList.value.filter(c => c.phase === 'streaming').length
)

function phaseLabel(p: StreamCard['phase']): string {
  switch (p) {
    case 'streaming': return '流式输出'
    case 'end': return '已完成'
    case 'error': return '出错'
  }
}

function formatElapsed(ms: number | null): string {
  if (ms == null) return ''
  if (ms < 1000) return `${ms}ms`
  return `${(ms / 1000).toFixed(1)}s`
}

function onStreamEvent(data: any) {
  if (!data || typeof data !== 'object') return
  const sessionId = typeof data.session_id === 'string' ? data.session_id : '(unknown)'
  const phase = String(data.phase ?? '')
  const text = typeof data.text === 'string' ? data.text : ''
  const model = typeof data.model === 'string' ? data.model : ''
  const seq = typeof data.seq === 'number' ? data.seq : 0
  const elapsed = typeof data.elapsed_ms === 'number' ? data.elapsed_ms : null

  let card = cards.value.get(sessionId)
  if (phase === 'start') {
    // 新一轮 LLM 调用：重置卡片（同会话上次流保留在别处无意义——观测面
    // 只关心当前流；start 帧即前端 reset 信号，与后端 sink seq 重置同源）
    card = {
      sessionId, model, text: '', phase: 'streaming', seq: 0,
      elapsedMs: null, chars: 0, truncated: false,
      updatedAt: Date.now(), startAt: Date.now(),
    }
  } else if (!card) {
    // 断线窗口内错过了 start（ephemeral 不可补拉）：诚实按中途流呈现
    card = {
      sessionId, model, text: '', phase: 'streaming', seq,
      elapsedMs: null, chars: 0, truncated: false,
      updatedAt: Date.now(), startAt: Date.now(),
    }
  }

  if (phase === 'delta') {
    if (seq > card.seq) card.seq = seq
    card.chars += text.length
    if (card.text.length < MAX_TEXT_CHARS) {
      card.text += text
      if (card.text.length >= MAX_TEXT_CHARS) {
        card.text = card.text.slice(0, MAX_TEXT_CHARS)
        card.truncated = true
      }
    }
  } else if (phase === 'end') {
    card.phase = 'end'
    card.elapsedMs = elapsed
    if (text) {
      card.chars += text.length
      if (card.text.length < MAX_TEXT_CHARS) card.text += text
    }
  } else if (phase === 'error') {
    card.phase = 'error'
    card.elapsedMs = elapsed
    if (text && card.text.length < MAX_TEXT_CHARS) card.text += (card.text ? '\n' : '') + text
  }
  card.updatedAt = Date.now()
  cards.value.set(sessionId, card)

  // 淘汰：超上限时移除最老的已结束卡片（直播中的保留）
  if (cards.value.size > MAX_CARDS) {
    const finished = cardList.value
      .filter(c => c.phase !== 'streaming')
      .sort((a, b) => a.updatedAt - b.updatedAt)
    const overflow = cards.value.size - MAX_CARDS
    for (let i = 0; i < Math.min(overflow, finished.length); i++) {
      cards.value.delete(finished[i].sessionId)
    }
  }
}

function clearCards() {
  cards.value.clear()
}

onMounted(() => {
  connectEvents()
  sseOn('provider_stream_event', onStreamEvent)
})

onUnmounted(() => {
  sseOff('provider_stream_event', onStreamEvent)
})
</script>

<template>
  <div v-if="cardList.length > 0" class="provider-stream-panel">
    <div class="psp-header" @click="collapsed = !collapsed">
      <span class="psp-title">⚡ Provider 流式输出</span>
      <span v-if="liveCount > 0" class="psp-live-badge">{{ liveCount }} 路直播中</span>
      <span class="psp-meta">{{ cardList.length }} 会话</span>
      <button class="psp-clear" title="清空全部卡片" @click.stop="clearCards">🗑</button>
      <span class="psp-toggle">{{ collapsed ? '▸' : '▾' }}</span>
    </div>
    <div v-if="!collapsed" class="psp-body">
      <div
        v-for="card in cardList"
        :key="card.sessionId"
        class="psp-card"
        :class="`psp-${card.phase}`"
      >
        <div class="psp-card-head">
          <span class="psp-session" :title="card.sessionId">{{ card.sessionId }}</span>
          <span class="psp-model">{{ card.model || '—' }}</span>
          <span class="psp-phase" :class="`phase-${card.phase}`">
            <template v-if="card.phase === 'streaming'">● {{ phaseLabel(card.phase) }}</template>
            <template v-else>{{ phaseLabel(card.phase) }}</template>
          </span>
          <span v-if="card.elapsedMs != null" class="psp-elapsed">{{ formatElapsed(card.elapsedMs) }}</span>
          <span class="psp-chars">{{ card.chars }} 字符</span>
        </div>
        <pre class="psp-text">{{ card.text }}<span v-if="card.truncated" class="psp-truncated">…[显示已截断，共 {{ card.chars }} 字符]</span><span v-if="card.phase === 'streaming'" class="psp-caret">▌</span></pre>
      </div>
    </div>
  </div>
</template>

<style scoped>
.provider-stream-panel {
  border-bottom: 1px solid var(--border-light);
  background: var(--bg-secondary);
  flex-shrink: 0;
  max-height: 40%;
  display: flex;
  flex-direction: column;
}

.psp-header {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-2) var(--space-4);
  cursor: pointer;
  user-select: none;
  font-size: var(--text-sm);
  color: var(--text-secondary);
}

.psp-header:hover {
  background: var(--bg-hover);
}

.psp-title {
  font-weight: 600;
  color: var(--text-primary);
}

.psp-live-badge {
  padding: 1px 8px;
  border-radius: var(--radius-sm);
  background: rgba(16, 185, 129, 0.15);
  color: #10b981;
  font-size: var(--text-xs);
  font-weight: 600;
}

.psp-meta {
  color: var(--text-muted);
  font-size: var(--text-xs);
}

.psp-clear {
  margin-left: auto;
  border: none;
  background: transparent;
  cursor: pointer;
  font-size: var(--text-sm);
  color: var(--text-muted);
}

.psp-clear:hover {
  color: var(--text-primary);
}

.psp-toggle {
  color: var(--text-muted);
}

.psp-body {
  overflow-y: auto;
  padding: 0 var(--space-4) var(--space-3);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.psp-card {
  border: 1px solid var(--border-light);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  overflow: hidden;
}

.psp-card.psp-error {
  border-color: rgba(239, 68, 68, 0.4);
}

.psp-card-head {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-1) var(--space-3);
  background: var(--bg-tertiary);
  font-size: var(--text-xs);
  color: var(--text-muted);
  flex-wrap: wrap;
}

.psp-session {
  font-family: 'Cascadia Code', 'Consolas', monospace;
  color: var(--accent);
  max-width: 260px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.psp-model {
  font-family: 'Cascadia Code', 'Consolas', monospace;
}

.psp-phase {
  font-weight: 600;
}

.psp-phase.phase-streaming { color: #10b981; }
.psp-phase.phase-end { color: var(--text-muted); }
.psp-phase.phase-error { color: #ef4444; }

.psp-elapsed {
  font-family: 'Cascadia Code', 'Consolas', monospace;
}

.psp-chars {
  margin-left: auto;
}

.psp-text {
  margin: 0;
  padding: var(--space-2) var(--space-3);
  font-family: 'Cascadia Code', 'Consolas', monospace;
  font-size: var(--text-xs);
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 200px;
  overflow-y: auto;
  color: var(--text-primary);
}

.psp-truncated {
  color: #eab308;
}

.psp-caret {
  color: #10b981;
  animation: psp-blink 1s step-end infinite;
}

@keyframes psp-blink {
  0%, 100% { opacity: 1; }
  50% { opacity: 0; }
}
</style>
