<script setup lang="ts">
/**
 * L6++（2026-09-08）F1：新建项目弹窗 —— 名称 + 绝对路径。
 *
 * 校验分层：前端只拦「非空 + 绝对路径形态」（行内提示，不发请求）；
 * 存在性 / canonical / 与主 workspace 或其他项目重叠 / 上限全由后端权威
 * 判定——拒绝时 toast 错误原文回显、modal 不关（无残留条目）。
 * 表单先例：board/ProjectPanel.vue 创建弹窗 + ForkSessionModal 骨架。
 */
import { ref } from 'vue'
import { useSessionStore } from '../stores/session'
import { useToast } from '../composables/useToast'
import type { ProjectInfo } from '../composables/useChatApi'

const emit = defineEmits<{ (e: 'close'): void; (e: 'created', project: ProjectInfo): void }>()

const sessionStore = useSessionStore()
const toast = useToast()

const name = ref('')
const path = ref('')
const inlineError = ref('')
const busy = ref(false)

/** 绝对路径形态：Windows 盘符（C:/ 或 C:\）或 POSIX 根（/）。仅形态校验，
 *  不判存在性（后端权威）。 */
function isAbsolutePath(p: string): boolean {
  return /^[A-Za-z]:[\\/]/.test(p) || p.startsWith('/')
}

async function submit() {
  inlineError.value = ''
  const n = name.value.trim()
  const p = path.value.trim()
  if (!n) {
    inlineError.value = '请填写项目名称'
    return
  }
  if (!p) {
    inlineError.value = '请填写项目路径'
    return
  }
  if (!isAbsolutePath(p)) {
    inlineError.value = '路径必须是绝对路径（如 C:\\works\\proj 或 /home/me/proj）'
    return
  }
  if (busy.value) return
  busy.value = true
  try {
    const project = await sessionStore.createProject(n, p)
    toast.success(`项目「${project.name}」已创建`)
    emit('created', project)
    emit('close')
  } catch (e: any) {
    // 后端权威拒绝（重叠/上限/目录不存在…）→ 原文回显，modal 不关。
    const msg = typeof e === 'string' ? e : e?.message || '创建项目失败'
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
        <h3>新建项目</h3>
        <button class="close-btn" @click="emit('close')">×</button>
      </div>
      <div class="modal-body">
        <p class="hint">项目 = 一个本地目录的会话分组。该项目组的会话在此目录内工作（读写围栏锚定此目录）。</p>
        <div class="form-group">
          <label class="form-label">名称 *</label>
          <input class="form-input" v-model="name" placeholder="如：计费服务重构" @keyup.enter="submit" />
        </div>
        <div class="form-group">
          <label class="form-label">绝对路径 *</label>
          <input class="form-input" v-model="path" placeholder="如 C:\works\billing 或 /home/me/billing" @keyup.enter="submit" />
        </div>
        <div v-if="inlineError" class="inline-error">{{ inlineError }}</div>
      </div>
      <div class="modal-footer">
        <button class="btn" @click="emit('close')">取消</button>
        <button class="btn primary" :disabled="busy" @click="submit">
          {{ busy ? '创建中…' : '创建' }}
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
  width: 480px;
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
