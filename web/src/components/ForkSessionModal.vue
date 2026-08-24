<script setup lang="ts">
/**
 * P3-1 (2026-08-24 UI entry gap): 会话分叉弹窗 —— Z1 session fork 的完整版 UI。
 *
 * 打开时拉取轮次表（GET /api/chat/sessions/:id/turns，同 CLI `session show`
 * 的轮次计数：一轮 = 完整 user→…→assistant 交换；每行显示轮首提问 preview +
 * 轮末回复 end_preview —— 分叉保留完整轮次，选这行新会话就停在 end_preview 上），
 * 用户选分岔点后 POST fork（后端走 Z1 fork_session，2026-08-25 第三轮：轮次
 * 与分叉内容都以 chat_log jsonl 为唯一真相源——jsonl 前缀逐行原样复制，新
 * 会话 store 由同一批行重建，原会话不动）。成功后
 * emit('forked', 新会话 id)，由父组件刷新列表并切换。
 */
import { ref, watch } from 'vue'
import { useChatApi, type SessionTurnRow } from '../composables/useChatApi'
import { useToast } from '../composables/useToast'

const props = defineProps<{ sessionId: string; sessionTitle: string }>()
const emit = defineEmits<{ (e: 'close'): void; (e: 'forked', newSessionId: string): void }>()

const toast = useToast()
const api = useChatApi()

const loading = ref(false)
const forking = ref(false)
const turns = ref<SessionTurnRow[]>([])
const totalMessages = ref(0)
/** 选中的分岔轮次；null = 全量分叉（默认，对齐 CLI 省略 --at 的语义）。 */
const atTurn = ref<number | null>(null)

watch(
  () => props.sessionId,
  async (sid) => {
    if (!sid) return
    loading.value = true
    turns.value = []
    atTurn.value = null
    try {
      const resp = await api.turns(sid)
      turns.value = resp.turns
      totalMessages.value = resp.total_messages
    } catch (e: any) {
      toast.error(e?.message || '拉取轮次失败')
      emit('close')
    }
    loading.value = false
  },
  { immediate: true },
)

async function doFork() {
  if (forking.value) return
  forking.value = true
  try {
    const resp = await api.fork(props.sessionId, atTurn.value ?? undefined)
    toast.success(`已分叉出新会话（保留 ${resp.kept_messages} 条消息）`)
    emit('forked', resp.session_id)
  } catch (e: any) {
    toast.error(e?.message || '分叉失败')
  }
  forking.value = false
}
</script>

<template>
  <div class="modal-backdrop" @click.self="emit('close')">
    <div class="modal">
      <div class="modal-header">
        <h3>分叉会话</h3>
        <button class="close-btn" @click="emit('close')">×</button>
      </div>
      <div class="modal-body">
        <p class="hint">
          从「{{ sessionTitle }}」的某一轮分岔出新会话：新会话包含前 N 轮完整对话，
          原会话保持不变（之后的轮次继续在原会话进行）。选择分岔点：
        </p>
        <label class="full-option">
          <input type="radio" :value="null" v-model="atTurn" />
          <span>全量分叉（复制全部 {{ totalMessages }} 条消息，从头开始新分支）</span>
        </label>
        <div v-if="loading" class="loading">加载轮次中…</div>
        <div v-else class="turn-table">
          <label v-for="t in turns" :key="t.turn" class="turn-row">
            <input type="radio" :value="t.turn" v-model="atTurn" />
            <span class="turn-no">第 {{ t.turn }} 轮</span>
            <span class="turn-main">
              <span class="turn-preview">{{ t.preview }}</span>
              <span class="turn-end">↳ 分叉末条：{{ t.end_preview }}</span>
            </span>
            <span class="turn-meta">{{ t.kept_messages }} 条</span>
          </label>
        </div>
      </div>
      <div class="modal-footer">
        <span class="foot-hint">
          {{ atTurn == null ? '保留全部轮次' : `保留前 ${atTurn} 轮（从第 ${atTurn + 1} 轮起另开分支）` }}
        </span>
        <button class="btn" @click="emit('close')">取消</button>
        <button class="btn primary" :disabled="forking || loading" @click="doFork">
          {{ forking ? '分叉中…' : '分叉' }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}
.modal {
  width: 560px;
  max-width: 92vw;
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
}
.modal-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 16px;
  border-bottom: 1px solid var(--border);
}
.modal-header h3 {
  margin: 0;
  font-size: 15px;
}
.close-btn {
  background: none;
  border: none;
  font-size: 20px;
  color: var(--text-muted);
  cursor: pointer;
}
.modal-body {
  padding: 12px 16px;
  overflow-y: auto;
}
.hint {
  font-size: 12px;
  color: var(--text-muted);
  margin: 0 0 10px;
  line-height: 1.6;
}
.full-option {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px;
  border: 1px solid var(--border);
  border-radius: 6px;
  margin-bottom: 10px;
  font-size: 13px;
  cursor: pointer;
}
.loading {
  padding: 20px;
  text-align: center;
  color: var(--text-muted);
  font-size: 13px;
}
.turn-table {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.turn-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 8px;
  border-radius: 4px;
  font-size: 13px;
  cursor: pointer;
}
.turn-row:hover {
  background: var(--bg-primary);
}
.turn-no {
  min-width: 56px;
  color: var(--text-muted);
}
.turn-main {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}
.turn-preview {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
/* 该轮最后一条回复的首行 —— 分叉保留完整轮次，新会话将停在它上面。
 * end_preview 缺席（旧后端）时整行不渲染，不算错误。 */
.turn-end {
  font-size: 11px;
  color: var(--text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.turn-meta {
  min-width: 48px;
  text-align: right;
  color: var(--text-muted);
  font-size: 11px;
}
.modal-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
  padding: 12px 16px;
  border-top: 1px solid var(--border);
}
.foot-hint {
  flex: 1;
  font-size: 12px;
  color: var(--text-muted);
}
.btn {
  padding: 6px 16px;
  font-size: 13px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: transparent;
  color: var(--text-primary, inherit);
  cursor: pointer;
}
.btn.primary {
  border-color: var(--accent);
  color: var(--accent);
}
.btn.primary:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
