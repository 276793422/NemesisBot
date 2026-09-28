<script setup lang="ts">
/**
 * 角色目录与客户端委派（2026-09-28）：委派弹窗 —— 把任务派给指定角色的
 * 子代理（WSAPI `chat.spawn`，loop 侧与模型 spawn 同一 SpawnFn detached
 * 通道：工具档位/角色模板/安全管线全同源）。
 *
 * 角色下拉数据源 = WSAPI `roles.list`（目录全量 17 项 + 当前 tier + 可见
 * 性）。可见性裁决与 spawn dispatch 闸同一后端函数（visible_roles）——
 * 前端看到的集合就是模型能用的集合；不可见项展示但置灰（诚实目录：
 * 用户知道有什么、为什么不可用），提交前前端只拦「非空 + 必须选可见
 * 角色」，分档/隐藏的权威判定在后端。
 */
import { ref, watch } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

interface RoleEntry {
  slug: string
  description: string
  min_tier: string
  visible: boolean
  hidden: boolean
}

const props = defineProps<{ sessionId: string }>()
const emit = defineEmits<{ (e: 'close'): void; (e: 'done', payload: { role: string; result: string }): void }>()

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const busy = ref(false)
const inlineError = ref('')
const tier = ref('')
const visibleCount = ref(0)
const roles = ref<RoleEntry[]>([])
const task = ref('')
const role = ref('')
const toolsProfile = ref<'readonly' | 'full'>('readonly')

watch(
  () => props.sessionId,
  async (sid) => {
    if (!sid) return
    loading.value = true
    try {
      const resp = await request('roles', 'list')
      tier.value = resp?.tier ?? ''
      visibleCount.value = resp?.visible_count ?? 0
      roles.value = resp?.roles ?? []
    } catch (e: any) {
      const msg = typeof e === 'string' ? e : e?.message || '拉取角色目录失败'
      toast.error(msg)
      emit('close')
    }
    loading.value = false
  },
  { immediate: true },
)

async function submit() {
  inlineError.value = ''
  const t = task.value.trim()
  if (!t) {
    inlineError.value = '请填写任务描述'
    return
  }
  if (busy.value) return
  busy.value = true
  try {
    const resp = await request('chat', 'spawn', {
      session_id: props.sessionId,
      task: t,
      role: role.value,
      tools_profile: toolsProfile.value,
    })
    const result: string = resp?.result ?? ''
    toast.success(`委派完成（${resp?.role || '自动'}）：${result.slice(0, 80)}`)
    emit('done', { role: resp?.role ?? role.value, result })
    emit('close')
  } catch (e: any) {
    // 后端权威拒绝（未知角色/档位外/隐藏/槽未装配…）→ 原文回显，modal 不关。
    const msg = typeof e === 'string' ? e : e?.message || '委派失败'
    inlineError.value = msg
    toast.error(msg)
  }
  busy.value = false
}
</script>

<template>
  <div class="modal-backdrop" @click.self="emit('close')">
    <div class="modal">
      <div class="modal-header">
        <h3>委派任务给子代理</h3>
        <button class="close-btn" @click="emit('close')">×</button>
      </div>
      <div class="modal-body">
        <p class="hint">
          子代理以所选角色独立完成任务后把最终回复交回本会话
          <template v-if="tier">（当前模型档位：{{ tier }}，可用角色 {{ visibleCount }} 个）</template>。
        </p>
        <div v-if="loading" class="loading">加载角色目录中…</div>
        <template v-else>
          <div class="form-group">
            <label class="form-label">角色</label>
            <select class="form-input" v-model="role">
              <option value="">自动（按模型档位推导）</option>
              <option v-for="r in roles" :key="r.slug" :value="r.slug" :disabled="!r.visible">
                {{ r.slug }} — {{ r.description }}{{ r.visible ? '' : r.hidden ? '（已隐藏）' : `（需 ${r.min_tier} 档）` }}
              </option>
            </select>
          </div>
          <div class="form-group">
            <label class="form-label">任务描述 *</label>
            <textarea class="form-input task-input" v-model="task" rows="4"
              placeholder="如：审查当前工作区的未提交改动，指出正确性与安全问题"></textarea>
          </div>
          <div class="form-group">
            <label class="form-label">工具档位</label>
            <select class="form-input" v-model="toolsProfile">
              <option value="readonly">只读（默认，安全）</option>
              <option value="full">全量工具</option>
            </select>
          </div>
          <div v-if="inlineError" class="inline-error">{{ inlineError }}</div>
        </template>
      </div>
      <div class="modal-footer">
        <button class="btn" @click="emit('close')">取消</button>
        <button class="btn primary" :disabled="busy || loading" @click="submit">
          {{ busy ? '委派中…' : '委派' }}
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
  width: 520px;
  max-width: 92vw;
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
}
.hint {
  font-size: 12px;
  color: var(--text-muted);
  margin: 0 0 10px;
  line-height: 1.6;
}
.loading {
  padding: 20px;
  text-align: center;
  color: var(--text-muted);
  font-size: 13px;
}
.form-group {
  margin-bottom: 10px;
}
.form-label {
  display: block;
  font-size: 12px;
  color: var(--text-muted);
  margin-bottom: 4px;
}
.form-input {
  width: 100%;
  box-sizing: border-box;
  padding: 6px 8px;
  font-size: 13px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--bg-primary);
  color: inherit;
}
.task-input {
  resize: vertical;
  font-family: inherit;
}
.inline-error {
  font-size: 12px;
  color: #dc3545;
  margin-top: 4px;
  word-break: break-all;
}
.modal-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
  padding: 12px 16px;
  border-top: 1px solid var(--border);
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
