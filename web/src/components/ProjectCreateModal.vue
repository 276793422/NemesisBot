<script setup lang="ts">
/**
 * L6++（2026-09-08）F1：新建项目弹窗 —— 名称 + 绝对路径。
 *
 * 校验分层：前端只拦「非空 + 绝对路径形态」（行内提示，不发请求）；
 * 存在性 / canonical / 与主 workspace 或其他项目重叠 / 上限全由后端权威
 * 判定——拒绝时 toast 错误原文回显、modal 不关（无残留条目）。
 * 表单先例：board/ProjectPanel.vue 创建弹窗 + ForkSessionModal 骨架。
 *
 * D 批（2026-11-19）围栏多根：新增 `add-dir` 模式——给既有项目添加
 * 附加放行目录（＋项目按钮语义分叉的「会话属项目」臂）。同弹窗内列出
 * 已有 extra_dirs 支持摘除（摘列表不动磁盘）。生效时机诚实声明：围栏
 * Arc 在工具注册期烧入，写盘后**不热生效**——重启网关或重建项目后生效
 * （后端 note 同步声明）。
 */
import { ref } from 'vue'
import { useSessionStore } from '../stores/session'
import { useToast } from '../composables/useToast'
import type { ProjectInfo } from '../composables/useChatApi'

const props = defineProps<{
  /** `create`（默认）= 新建项目；`add-dir` = 给既有项目添加附加放行目录。 */
  mode?: 'create' | 'add-dir'
  /** add-dir 模式的目标项目（extra_dirs 展示与摘除的数据源）。 */
  project?: ProjectInfo
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'created', project: ProjectInfo): void
  (e: 'updated', project: ProjectInfo): void
}>()

const sessionStore = useSessionStore()
const toast = useToast()

const isAddDir = props.mode === 'add-dir'

const name = ref('')
const path = ref('')
const inlineError = ref('')
const busy = ref(false)
/** 摘除进行中的目录（防重复点击）。 */
const removing = ref('')

/** 绝对路径形态：Windows 盘符（C:/ 或 C:\）或 POSIX 根（/）。仅形态校验，
 *  不判存在性（后端权威）。 */
function isAbsolutePath(p: string): boolean {
  return /^[A-Za-z]:[\\/]/.test(p) || p.startsWith('/')
}

async function submit() {
  inlineError.value = ''
  if (isAddDir) {
    await submitAddDir()
    return
  }
  await submitCreate()
}

async function submitCreate() {
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

async function submitAddDir() {
  const target = props.project
  if (!target) {
    inlineError.value = '缺少目标项目'
    return
  }
  const p = path.value.trim()
  if (!p) {
    inlineError.value = '请填写目录路径'
    return
  }
  if (!isAbsolutePath(p)) {
    inlineError.value = '目录必须是绝对路径（如 D:\\assets\\lib 或 /srv/assets）'
    return
  }
  if (busy.value) return
  busy.value = true
  try {
    const project = await sessionStore.addProjectDir(target.id, p)
    path.value = ''
    // 后端 note = 诚实生效时机声明，原文透传（不代拟「已生效」）。
    toast.success(`目录已添加到「${project.name}」。${NOTE_EFFECTIVE}`)
    emit('updated', project)
  } catch (e: any) {
    const msg = typeof e === 'string' ? e : e?.message || '添加目录失败'
    inlineError.value = msg
    toast.error(msg)
  }
  busy.value = false
}

/** 摘除附加放行目录：摘列表不动磁盘（注册不拥有）；生效时机同添加。 */
async function removeDir(dir: string) {
  const target = props.project
  if (!target || removing.value) return
  if (!confirm(`移除放行目录「${dir}」？仅解除放行（摘列表），不删除目录内的任何文件。`)) return
  removing.value = dir
  try {
    const project = await sessionStore.removeProjectDir(target.id, dir)
    toast.success(`目录已从「${project.name}」解除放行。${NOTE_EFFECTIVE}`)
    emit('updated', project)
  } catch (e: any) {
    toast.error(typeof e === 'string' ? e : e?.message || '移除目录失败')
  }
  removing.value = ''
}

/** 生效时机诚实声明（add/remove 共用；与后端 note 口径一致）。 */
const NOTE_EFFECTIVE = '在跑的项目围栏不热生效——重启网关或重建项目后生效'
</script>

<template>
  <div class="modal-backdrop" @click.self="emit('close')">
    <div class="modal">
      <div class="modal-header">
        <h3 v-if="isAddDir">添加目录 — {{ project?.name }}</h3>
        <h3 v-else>新建项目</h3>
        <button class="close-btn" @click="emit('close')">×</button>
      </div>
      <div class="modal-body">
        <template v-if="isAddDir">
          <p class="hint">
            附加放行目录：项目组会话的读写围栏在项目目录之外额外放行该目录
            （资产库/共享盘等）。目录内容不出现在文件树、不进 checkpoint 快照
            （rewind 不覆盖它）；对在跑的项目围栏不热生效——重启网关或重建
            项目后生效。
          </p>
          <div v-if="(project?.extra_dirs ?? []).length" class="extra-list">
            <div v-for="d in project?.extra_dirs ?? []" :key="d" class="extra-row">
              <span class="extra-path" :title="d">{{ d }}</span>
              <button
                class="extra-remove"
                :disabled="removing === d"
                title="移除放行（不删除文件）"
                @click="removeDir(d)"
              >✕</button>
            </div>
          </div>
          <div class="form-group">
            <label class="form-label">绝对路径 *</label>
            <input class="form-input" v-model="path" placeholder="如 D:\assets\lib 或 /srv/assets" @keyup.enter="submit" />
          </div>
        </template>
        <template v-else>
          <p class="hint">项目 = 一个本地目录的会话分组。该项目组的会话在此目录内工作（读写围栏锚定此目录）。</p>
          <div class="form-group">
            <label class="form-label">名称 *</label>
            <input class="form-input" v-model="name" placeholder="如：计费服务重构" @keyup.enter="submit" />
          </div>
          <div class="form-group">
            <label class="form-label">绝对路径 *</label>
            <input class="form-input" v-model="path" placeholder="如 C:\works\billing 或 /home/me/billing" @keyup.enter="submit" />
          </div>
        </template>
        <div v-if="inlineError" class="inline-error">{{ inlineError }}</div>
      </div>
      <div class="modal-footer">
        <button class="btn" @click="emit('close')">取消</button>
        <button class="btn primary" :disabled="busy" @click="submit">
          {{ busy ? (isAddDir ? '添加中…' : '创建中…') : (isAddDir ? '添加' : '创建') }}
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
/* D 批：已放行目录列表（add-dir 模式）。 */
.extra-list {
  margin: 0 0 10px;
  border: 1px solid var(--border);
  border-radius: 4px;
  overflow: hidden;
}
.extra-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 8px;
  font-size: 12px;
}
.extra-row + .extra-row {
  border-top: 1px solid var(--border);
}
.extra-path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl;
  text-align: left;
  unicode-bidi: plaintext;
}
.extra-remove {
  flex-shrink: 0;
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 12px;
  padding: 2px 4px;
}
.extra-remove:hover:not(:disabled) {
  color: #dc3545;
}
.extra-remove:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
