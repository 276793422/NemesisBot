<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { storeToRefs } from 'pinia'
import { useWorkflowStore } from '../../stores/workflow'
import { useWSAPI } from '../../composables/useWSAPI'

const store = useWorkflowStore()
const { workflows, driverStatus, listLoading, listError, hasUndrivenTriggers } = storeToRefs(store)
const { request } = useWSAPI()

function refresh() {
  store.clearListCache()
  store.fetchList(true)
}

function startNew() {
  store.startNewWorkflow()
  store.setActiveTab('canvas')
}

function editWorkflow(name: string) {
  store.loadForEdit(name)
  store.setActiveTab('canvas')
}

function viewHistory(name: string) {
  store.setActiveTab('history')
  store.fetchRuns({ workflow_name: name })
}

function openWorkflowChat(chatIndex: string) {
  // Path-mode URL: /workflow/chat/<8hex>. The standalone page reads
  // window.location.pathname to recover the index — no hash routing.
  window.open('/workflow/chat/' + encodeURIComponent(chatIndex), '_blank')
}

// ---- Delete confirmation modal -------------------------------------------

const deleteModal = ref<{
  open: boolean
  name: string
  busy: boolean
  error: string
}>({
  open: false,
  name: '',
  busy: false,
  error: '',
})

function openDeleteModal(name: string) {
  deleteModal.value = {
    open: true,
    name,
    busy: false,
    error: '',
  }
}

function closeDeleteModal() {
  if (deleteModal.value.busy) return
  deleteModal.value.open = false
}

async function submitDelete() {
  const m = deleteModal.value
  if (m.busy) return
  m.busy = true
  m.error = ''
  try {
    const result = await store.deleteWorkflow(m.name)
    if (result.ok) {
      m.open = false
    } else {
      m.error = result.error
    }
  } catch (e: any) {
    m.error = e?.message || String(e)
  } finally {
    m.busy = false
  }
}

// ---- Password management modal -------------------------------------------

const passwordModal = ref<{
  open: boolean
  chatIndex: string
  workflowName: string
  hasPassword: boolean
  mode: 'set' | 'clear'
  password: string
  confirm: string
  busy: boolean
  error: string
}>({
  open: false,
  chatIndex: '',
  workflowName: '',
  hasPassword: false,
  mode: 'set',
  password: '',
  confirm: '',
  busy: false,
  error: '',
})

function openPasswordModal(wf: { name: string; chat_index: string; has_chat_password: boolean }) {
  passwordModal.value = {
    open: true,
    chatIndex: wf.chat_index,
    workflowName: wf.name,
    hasPassword: wf.has_chat_password,
    mode: wf.has_chat_password ? 'clear' : 'set',
    password: '',
    confirm: '',
    busy: false,
    error: '',
  }
}

function closePasswordModal() {
  if (passwordModal.value.busy) return
  passwordModal.value.open = false
}

async function submitPasswordModal() {
  const m = passwordModal.value
  if (m.busy) return
  m.error = ''
  if (m.mode === 'set') {
    if (!m.password) {
      m.error = '密码不能为空'
      return
    }
    if (m.password !== m.confirm) {
      m.error = '两次输入的密码不一致'
      return
    }
  }
  m.busy = true
  try {
    if (m.mode === 'set') {
      await request('workflow', 'set_chat_password', {
        index: m.chatIndex,
        password: m.password,
      })
    } else {
      await request('workflow', 'clear_chat_password', { index: m.chatIndex })
    }
    // Refresh list so the lock icon state reflects the new password state.
    await store.fetchList(true)
    m.open = false
  } catch (e: any) {
    m.error = e?.message || String(e)
  } finally {
    m.busy = false
  }
}

onMounted(() => {
  // Fetch list on first mount in case the parent view hasn't yet.
  if (workflows.value.length === 0 && !listLoading.value) {
    store.fetchList()
  }
})
</script>

<template>
  <div class="wf-list">
    <div class="wf-list-toolbar">
      <button class="btn btn-primary" @click="startNew">
        + 新建工作流
      </button>
      <button class="btn" @click="refresh">⟳ 刷新</button>
      <div v-if="hasUndrivenTriggers" class="warn-banner">
        ⚠ 部分触发器未驱动（详情见下方触发器驱动状态）
      </div>
    </div>

    <div v-if="listLoading" class="wf-empty">⟳ 加载工作流列表...</div>
    <div v-else-if="listError" class="wf-empty wf-error">⚠ {{ listError }}</div>
    <div v-else-if="workflows.length === 0" class="wf-empty">
      暂无工作流。点击「新建工作流」开始创建。
    </div>

    <table v-else class="wf-table">
      <thead>
        <tr>
          <th>名称</th>
          <th>描述</th>
          <th>版本</th>
          <th>节点</th>
          <th>触发器</th>
          <th>下次触发</th>
          <th class="col-actions">操作</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="wf in workflows" :key="wf.name">
          <td class="col-name">{{ wf.name }}</td>
          <td class="col-desc">{{ wf.description || '—' }}</td>
          <td>{{ wf.version }}</td>
          <td>{{ wf.node_count }}</td>
          <td>
            <div class="trigger-badges">
              <span
                v-for="(t, idx) in wf.triggers"
                :key="idx"
                class="badge"
                :class="t.driven ? 'badge-ok' : 'badge-warn'"
                :title="t.driven ? '已驱动' : (t.reason || '未驱动')"
              >
                {{ t.trigger_type }}
              </span>
            </div>
          </td>
          <td class="col-next">
            <span v-for="(t, idx) in wf.triggers" :key="idx">
              <span v-if="t.next_fire_at">{{ t.next_fire_at }}<br /></span>
            </span>
            <span v-if="!wf.triggers.some(t => t.next_fire_at)">—</span>
          </td>
          <td class="col-actions">
            <button class="btn btn-small" @click="editWorkflow(wf.name)">
              编辑
            </button>
            <button class="btn btn-small" @click="viewHistory(wf.name)">
              历史
            </button>
            <button
              v-if="wf.chat_index"
              class="btn btn-small"
              :title="wf.has_chat_password ? '修改/清除聊天密码' : '设置聊天密码'"
              @click="openPasswordModal(wf)"
            >
              <span :class="wf.has_chat_password ? 'lock-on' : 'lock-off'">🔒</span>
            </button>
            <button
              v-if="wf.chat_index"
              class="btn btn-small"
              title="在新标签页中测试聊天此工作流"
              @click="openWorkflowChat(wf.chat_index)"
            >
              💬
            </button>
            <button
              class="btn btn-small btn-danger"
              title="删除此工作流"
              @click="openDeleteModal(wf.name)"
            >
              ×
            </button>
          </td>
        </tr>
      </tbody>
    </table>

    <div v-if="Object.keys(driverStatus).length > 0" class="driver-status">
      <div class="driver-status-title">触发器驱动状态（来自后端）：</div>
      <div
        v-for="(s, key) in driverStatus"
        :key="key"
        class="driver-status-row"
        :class="s.driven ? 'ok' : 'warn'"
      >
        <span class="ds-name">{{ s.trigger_type }}</span>
        <span class="ds-state">{{ s.driven ? '已驱动' : '未驱动' }}</span>
        <span v-if="s.reason" class="ds-reason">{{ s.reason }}</span>
      </div>
    </div>

    <!-- Password management modal -->
    <div v-if="passwordModal.open" class="pwd-modal-overlay" @click.self="closePasswordModal">
      <div class="pwd-modal">
        <h2>{{ passwordModal.mode === 'set' ? '设置聊天密码' : '清除聊天密码' }}</h2>
        <p class="pwd-modal-subtitle">{{ passwordModal.workflowName }}</p>

        <template v-if="passwordModal.mode === 'set'">
          <label class="pwd-label">新密码</label>
          <input
            class="form-input"
            type="password"
            autocomplete="new-password"
            v-model="passwordModal.password"
            :disabled="passwordModal.busy"
          />
          <label class="pwd-label">确认密码</label>
          <input
            class="form-input"
            type="password"
            autocomplete="new-password"
            v-model="passwordModal.confirm"
            @keydown.enter="submitPasswordModal"
            :disabled="passwordModal.busy"
          />
        </template>
        <template v-else>
          <p class="pwd-warn">
            当前已设置密码。清除后任何人通过 URL 都可访问此工作流的聊天页。
          </p>
        </template>

        <p v-if="passwordModal.error" class="pwd-error">{{ passwordModal.error }}</p>

        <div class="pwd-actions">
          <button class="btn" @click="closePasswordModal" :disabled="passwordModal.busy">
            取消
          </button>
          <button
            v-if="passwordModal.hasPassword && passwordModal.mode === 'set'"
            class="btn btn-secondary"
            @click="passwordModal.mode = 'clear'"
            :disabled="passwordModal.busy"
          >
            切换为清除
          </button>
          <button
            v-else-if="passwordModal.hasPassword"
            class="btn btn-secondary"
            @click="passwordModal.mode = 'set'"
            :disabled="passwordModal.busy"
          >
            切换为重设
          </button>
          <button
            class="btn btn-primary"
            @click="submitPasswordModal"
            :disabled="passwordModal.busy"
          >
            {{ passwordModal.busy ? '处理中...' : (passwordModal.mode === 'set' ? '保存' : '清除') }}
          </button>
        </div>
      </div>
    </div>

    <!-- Delete confirmation modal -->
    <div v-if="deleteModal.open" class="pwd-modal-overlay" @click.self="closeDeleteModal">
      <div class="pwd-modal">
        <h2>删除工作流</h2>
        <p class="pwd-modal-subtitle">
          确认删除工作流 <code class="del-name">{{ deleteModal.name }}</code> 吗？
        </p>
        <p class="pwd-warn">
          此操作不可撤销。删除后磁盘上的工作流文件和历史记录都会被清除。
        </p>
        <p v-if="deleteModal.error" class="pwd-error">{{ deleteModal.error }}</p>
        <div class="pwd-actions">
          <button class="btn" @click="closeDeleteModal" :disabled="deleteModal.busy">
            取消
          </button>
          <button
            class="btn btn-danger-solid"
            @click="submitDelete"
            :disabled="deleteModal.busy"
          >
            {{ deleteModal.busy ? '处理中...' : '删除' }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.wf-list {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: var(--space-3);
  gap: var(--space-3);
  overflow: auto;
}

.wf-list-toolbar {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.warn-banner {
  margin-left: auto;
  font-size: var(--text-xs);
  color: var(--warning, #f39c12);
  background: rgba(243, 156, 18, 0.1);
  padding: var(--space-1) var(--space-2);
  border-radius: var(--radius-sm);
}

.wf-empty {
  padding: var(--space-6);
  text-align: center;
  color: var(--text-muted);
}

.wf-error {
  color: var(--danger, #e74c3c);
}

.wf-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-sm);
}

.wf-table th,
.wf-table td {
  padding: var(--space-2) var(--space-3);
  text-align: left;
  border-bottom: 1px solid var(--border);
}

.wf-table th {
  font-weight: 600;
  color: var(--text-secondary);
  background: var(--bg-secondary);
}

.col-name {
  font-weight: 500;
}

.col-desc {
  color: var(--text-secondary);
  max-width: 280px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.col-next {
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}

.col-actions {
  white-space: nowrap;
  display: flex;
  gap: var(--space-1);
  justify-content: flex-end;
}

.trigger-badges {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-1);
}

.badge {
  display: inline-block;
  padding: 2px var(--space-2);
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  font-weight: 500;
}

.badge-ok {
  background: rgba(46, 204, 113, 0.15);
  color: var(--success, #2ecc71);
}

.badge-warn {
  background: rgba(243, 156, 18, 0.15);
  color: var(--warning, #f39c12);
}

.driver-status {
  margin-top: var(--space-3);
  padding: var(--space-3);
  background: var(--bg-secondary);
  border-radius: var(--radius-md);
  font-size: var(--text-xs);
}

.driver-status-title {
  font-weight: 600;
  margin-bottom: var(--space-2);
  color: var(--text-secondary);
}

.driver-status-row {
  display: flex;
  gap: var(--space-2);
  padding: var(--space-1) 0;
}

.driver-status-row .ds-name {
  min-width: 90px;
  font-weight: 500;
}

.driver-status-row.ok .ds-state {
  color: var(--success, #2ecc71);
}

.driver-status-row.warn .ds-state {
  color: var(--warning, #f39c12);
}

.ds-reason {
  color: var(--text-muted);
  flex: 1;
}

.btn-small {
  padding: 2px var(--space-2);
  font-size: var(--text-xs);
}

.lock-on {
  filter: hue-rotate(0deg);
}

.lock-off {
  opacity: 0.55;
}

.pwd-modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

.pwd-modal {
  background: var(--bg-surface, #fff);
  border-radius: 8px;
  padding: var(--space-5);
  min-width: 380px;
  max-width: 460px;
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.3);
}

.pwd-modal h2 {
  margin: 0 0 var(--space-1) 0;
  font-size: var(--text-lg, 18px);
}

.pwd-modal-subtitle {
  color: var(--text-secondary);
  margin: 0 0 var(--space-3) 0;
  font-size: var(--text-sm);
}

.pwd-label {
  display: block;
  font-size: var(--text-xs);
  color: var(--text-secondary);
  margin: var(--space-2) 0 var(--space-1) 0;
}

.pwd-warn {
  color: var(--warning, #f39c12);
  font-size: var(--text-sm);
  padding: var(--space-2);
  background: rgba(243, 156, 18, 0.08);
  border-radius: var(--radius-sm);
}

.pwd-error {
  color: var(--danger, #e74c3c);
  font-size: var(--text-sm);
  margin: var(--space-2) 0;
}

.pwd-actions {
  display: flex;
  gap: var(--space-2);
  justify-content: flex-end;
  margin-top: var(--space-4);
}

.form-input {
  display: block;
  width: 100%;
  padding: 8px 10px;
  border: 1px solid var(--border, #ddd);
  border-radius: var(--radius-sm);
  font-size: var(--text-sm);
  background: var(--bg-input, #fff);
  color: var(--text, #222);
  box-sizing: border-box;
}

.btn-secondary {
  background: var(--bg-secondary);
  color: var(--text);
}

.btn-danger {
  color: var(--danger, #e74c3c);
  border: 1px solid transparent;
}

.btn-danger:hover:not(:disabled) {
  border-color: var(--danger, #e74c3c);
  background: rgba(231, 76, 60, 0.08);
}

.btn-danger-solid {
  background: var(--danger, #e74c3c);
  color: #fff;
  border: 1px solid var(--danger, #e74c3c);
}

.btn-danger-solid:hover:not(:disabled) {
  opacity: 0.9;
}

.del-name {
  font-family: 'Consolas', monospace;
  background: var(--bg-secondary);
  padding: 1px var(--space-1);
  border-radius: var(--radius-sm);
  font-size: var(--text-sm);
}
</style>
