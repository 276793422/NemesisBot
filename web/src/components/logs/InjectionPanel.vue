<script setup lang="ts">
import { ref, watch } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'

// G2（U9 ②）：注入与重放可见性面板。
// - 注入台账聚合（round → 注入来源/位置/尺寸，无原文）
// - 每轮「校验重放」：session store + 台账重建 vs request_logs 原始请求逐字节比对

const props = defineProps<{ session: string }>()

const { request } = useWSAPI()

const expanded = ref(false)
const loading = ref(false)
const summary = ref<any>(null)
// 会话级缓存：切换回来时不重复拉取（台账是 append-only，会话详情生命周期内够用）。
const cache = new Map<string, any>()

async function load() {
  if (cache.has(props.session)) {
    summary.value = cache.get(props.session)
    return
  }
  const sess = props.session
  loading.value = true
  try {
    const data = await request('logs', 'injection_summary', { session: sess })
    if (sess !== props.session) return // 会话已切换：在途响应丢弃，不污染当前面板
    summary.value = data
    cache.set(sess, data)
  } catch {
    if (sess !== props.session) return
    summary.value = null
  } finally {
    if (sess === props.session) loading.value = false
  }
}

watch(expanded, v => { if (v) void load() })
watch(
  () => props.session,
  () => {
    summary.value = null
    verifyResults.value = {}
    verifying.value = {}
    if (expanded.value) void load()
  },
)

const verifying = ref<Record<string, boolean>>({})
const verifyResults = ref<Record<string, any>>({})

/// round 每回合从 1 重来 —— 结果/在途状态都按 `trace_id:round` 组合键存，
/// 只按 round 存会让多回合会话的同号轮互相覆盖。
function vkey(traceId: string, round: number): string {
  return `${traceId}:${round}`
}

async function verify(round: number, traceId: string) {
  const sess = props.session
  const k = vkey(traceId, round)
  verifying.value = { ...verifying.value, [k]: true }
  let r: any
  try {
    r = await request('logs', 'replay_verify', { session: sess, round, trace_id: traceId })
  } catch (e) {
    r = { ok: false, verdict: 'error', note: String(e) }
  }
  // 会话已切换：在途结论丢弃（A 会话的校验结论不得挂到 B 会话同名轮次上）。
  if (sess !== props.session) return
  verifyResults.value = { ...verifyResults.value, [k]: r }
  verifying.value = { ...verifying.value, [k]: false }
}

const SOURCE_LABELS: Record<string, string> = {
  context_digest: '上下文摘要',
  grace_nudge: '收尾提醒',
  degenerate_nudge: '纠偏提醒',
  repetition_nudge: '重复提醒',
  voice_append: '语音后缀',
  llm_hook: 'LLM Hook',
}

function sourceLabel(s: string): string {
  return SOURCE_LABELS[s] || s
}

function verdictBadge(v: any): { icon: string; text: string; cls: string; title: string } {
  switch (v?.verdict) {
    case 'byte_exact':
      return { icon: '✅', text: '逐字节一致', cls: 'ok', title: v.note || '' }
    case 'degraded_subsequence':
      return v.ok
        ? { icon: '⚠️', text: '降级：角色子序列匹配', cls: 'warn', title: v.note || '' }
        : { icon: '⚠️', text: '降级：子序列也不匹配', cls: 'bad', title: v.note || '' }
    case 'unavailable':
      return {
        icon: '✂️',
        text: `历史已裁剪（需 ${v.needed} 条，仅存 ${v.available} 条）`,
        cls: 'warn',
        title: v.note || '',
      }
    case 'mismatch':
      return {
        icon: '❌',
        text: `首差异 #${v.first_diff?.index} ${v.first_diff?.kind || ''}`,
        cls: 'bad',
        title: v.first_diff?.detail || v.note || '',
      }
    case 'no_recording':
      return { icon: '🚫', text: '无原始请求录制', cls: 'muted', title: v.note || '' }
    case 'no_ledger':
      return { icon: '🚫', text: '无注入台账', cls: 'muted', title: v.note || '' }
    default:
      return { icon: '💥', text: '校验请求失败', cls: 'bad', title: v?.note || '' }
  }
}
</script>

<template>
  <div class="injection-panel">
    <button class="panel-toggle" @click="expanded = !expanded">
      <span class="toggle-icon">{{ expanded ? '▾' : '▸' }}</span>
      🧩 注入与重放
      <span v-if="summary?.available" class="toggle-count">
        {{ summary.total_rounds }} 轮 · {{ summary.total_injections }} 次注入
      </span>
    </button>

    <div v-if="expanded" class="panel-body">
      <div v-if="loading" class="panel-hint">加载中…</div>

      <template v-else-if="summary?.available">
        <div v-for="r in summary.rounds" :key="r.trace_id + ':' + r.round" class="round-row">
          <div class="round-head">
            <span class="round-no">第 {{ r.round }} 轮</span>
            <span class="round-meta">{{ r.messages_count }} 条消息 · 历史 {{ r.history_len }}</span>
            <span v-if="r.summary_used" class="round-flag" title="本轮请求使用了摘要折叠（covers_up_to={{ r.summary_covers_up_to }}）">📄 摘要</span>
            <span v-if="r.voice_append" class="round-flag" title="本轮有语音播报后缀注入">🔊 语音</span>
            <button class="btn btn-sm verify-btn" :disabled="verifying[vkey(r.trace_id, r.round)]" @click="verify(r.round, r.trace_id)">
              {{ verifying[vkey(r.trace_id, r.round)] ? '校验中…' : '校验重放' }}
            </button>
          </div>

          <div v-if="r.injections.length" class="inj-list">
            <span v-for="(inj, i) in r.injections" :key="i" class="inj-tag" :title="`位置 #${inj.index} · ${inj.role} · ${inj.chars} 字符`">
              {{ sourceLabel(inj.source) }} <span class="inj-pos">#{{ inj.index }}</span>
            </span>
          </div>
          <div v-else class="inj-none">无注入</div>

          <div v-if="verifyResults[vkey(r.trace_id, r.round)]" class="verify-result" :class="verdictBadge(verifyResults[vkey(r.trace_id, r.round)]).cls" :title="verdictBadge(verifyResults[vkey(r.trace_id, r.round)]).title">
            {{ verdictBadge(verifyResults[vkey(r.trace_id, r.round)]).icon }} {{ verdictBadge(verifyResults[vkey(r.trace_id, r.round)]).text }}
          </div>
        </div>
      </template>

      <div v-else class="panel-hint">该会话无注入台账（旧会话或豁免轮次）</div>
    </div>
  </div>
</template>

<style scoped>
.injection-panel {
  border-bottom: 1px solid var(--border-light);
  background: var(--bg-secondary);
}

.panel-toggle {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  width: 100%;
  padding: var(--space-2) var(--space-4);
  background: transparent;
  border: none;
  cursor: pointer;
  color: var(--text-secondary);
  font-size: var(--text-sm);
  text-align: left;
}

.panel-toggle:hover { color: var(--text-primary); background: var(--bg-hover); }

.toggle-icon { font-size: var(--text-xs); width: 12px; }

.toggle-count {
  margin-left: auto;
  color: var(--text-muted);
  font-size: var(--text-xs);
}

.panel-body {
  padding: var(--space-2) var(--space-4) var(--space-3);
  max-height: 240px;
  overflow-y: auto;
}

.panel-hint {
  color: var(--text-muted);
  font-size: var(--text-xs);
  padding: var(--space-1) 0;
}

.round-row {
  padding: var(--space-2) 0;
  border-bottom: 1px dashed var(--border-light);
}
.round-row:last-child { border-bottom: none; }

.round-head {
  display: flex;
  align-items: center;
  gap: var(--space-3);
}

.round-no {
  font-size: var(--text-sm);
  color: var(--text-primary);
  font-weight: 600;
}

.round-meta {
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.round-flag {
  font-size: var(--text-xs);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  background: var(--accent-muted);
  color: var(--accent);
}

.verify-btn { margin-left: auto; }

.inj-list {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-1);
  margin-top: var(--space-1);
}

.inj-tag {
  font-size: var(--text-xs);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  background: rgba(59, 130, 246, 0.15);
  color: #3b82f6;
}

.inj-pos { opacity: 0.7; font-family: monospace; }

.inj-none {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-top: var(--space-1);
}

.verify-result {
  margin-top: var(--space-1);
  font-size: var(--text-xs);
  padding: 2px 8px;
  border-radius: var(--radius-sm);
  display: inline-block;
}

.verify-result.ok { background: rgba(34, 197, 94, 0.15); color: #16a34a; }
.verify-result.warn { background: rgba(245, 158, 11, 0.15); color: #d97706; }
.verify-result.bad { background: rgba(239, 68, 68, 0.15); color: #ef4444; }
.verify-result.muted { background: var(--bg-hover); color: var(--text-muted); }
</style>
