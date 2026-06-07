<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import TaskRow from './TaskRow.vue'

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const tasks = ref<any[]>([])
const filterStatus = ref('')
const stats = ref({ queued: 0, running: 0, completed: 0, failed: 0 })
const showSubmitModal = ref(false)
const submitForm = ref({ content: '', target_node_id: '' })
const submitSubmitting = ref(false)
const nodeList = ref<any[]>([])
const currentOffset = ref(0)
const hasMore = ref(true)

let refreshTimer: ReturnType<typeof setInterval> | null = null

const filtered = computed(() => {
  if (!filterStatus.value) return tasks.value
  return tasks.value.filter((t: any) => t.status === filterStatus.value)
})

async function loadTasks(append = false) {
  try {
    const data = await request('cluster', 'tasks.list', {
      status_filter: filterStatus.value || undefined,
      offset: append ? currentOffset.value : 0,
      limit: 20,
    })
    if (data?.tasks) {
      if (append) {
        tasks.value = [...tasks.value, ...data.tasks]
      } else {
        tasks.value = data.tasks
        currentOffset.value = 0
      }
      currentOffset.value = tasks.value.length
      hasMore.value = data.tasks.length >= 20
    }
    if (data?.stats) stats.value = data.stats
  } catch { /* backend not ready */ }
}

async function loadMoreTasks() {
  await loadTasks(true)
}

async function loadNodeList() {
  try {
    const data = await request('cluster', 'nodes.list')
    if (data?.nodes) nodeList.value = data.nodes
  } catch { /* ignore */ }
}

async function cancelTask(id: string) {
  try {
    await request('cluster', 'tasks.cancel', { task_id: id })
    toast.success('任务已取消')
    await loadTasks()
  } catch (e: any) {
    toast.error('取消失败: ' + (e || '未知错误'))
  }
}

async function submitTask() {
  if (!submitForm.value.content.trim()) {
    toast.warn('请输入任务内容')
    return
  }
  submitSubmitting.value = true
  try {
    await request('cluster', 'tasks.submit', {
      content: submitForm.value.content.trim(),
      target_node_id: submitForm.value.target_node_id || undefined,
    })
    toast.success('任务已提交')
    showSubmitModal.value = false
    submitForm.value = { content: '', target_node_id: '' }
    await loadTasks()
  } catch (e: any) {
    toast.error('提交失败: ' + (e || '未知错误'))
  } finally {
    submitSubmitting.value = false
  }
}

function openSubmitModal() {
  loadNodeList()
  showSubmitModal.value = true
}

onMounted(async () => {
  await loadTasks()
  loading.value = false
  refreshTimer = setInterval(loadTasks, 5000)
})

onUnmounted(() => {
  if (refreshTimer) clearInterval(refreshTimer)
})
</script>

<template>
  <div v-if="loading" style="text-align:center;padding:var(--space-8)">
    <div class="spinner spinner-lg" style="margin:0 auto" />
  </div>

  <div v-if="!loading">
    <div class="stats-grid" style="margin-bottom:var(--space-3)">
      <div class="stat-card">
        <div class="stat-label">排队</div>
        <div class="stat-value"><span class="badge badge-warning">{{ stats.queued }}</span></div>
      </div>
      <div class="stat-card">
        <div class="stat-label">运行</div>
        <div class="stat-value"><span class="badge badge-info">{{ stats.running }}</span></div>
      </div>
      <div class="stat-card">
        <div class="stat-label">完成</div>
        <div class="stat-value"><span class="badge badge-success">{{ stats.completed }}</span></div>
      </div>
      <div class="stat-card">
        <div class="stat-label">失败</div>
        <div class="stat-value"><span class="badge badge-error">{{ stats.failed }}</span></div>
      </div>
    </div>

    <div style="display:flex;gap:var(--space-2);margin-bottom:var(--space-3);align-items:center">
      <select class="form-select" v-model="filterStatus" style="width:auto" @change="loadTasks()">
        <option value="">全部状态</option>
        <option value="queued">排队</option>
        <option value="running">运行</option>
        <option value="completed">完成</option>
        <option value="failed">失败</option>
      </select>
      <div style="flex:1" />
      <button class="btn btn-sm btn-primary" @click="openSubmitModal">提交任务</button>
      <button class="btn btn-sm" @click="loadTasks()">刷新</button>
    </div>

    <div v-if="!filtered.length" class="empty-state">
      <h3>暂无任务</h3>
      <p>集群任务会自动显示在这里</p>
    </div>

    <TaskRow
      v-for="task in filtered"
      :key="task.id"
      :task="task"
      @cancel="cancelTask"
    />

    <div v-if="hasMore && filtered.length > 0" style="text-align:center;padding:var(--space-3)">
      <button class="btn btn-sm" @click="loadMoreTasks">加载更多</button>
    </div>

    <!-- Submit Task Modal -->
    <div v-if="showSubmitModal" class="modal-overlay" @click.self="showSubmitModal = false">
      <div class="modal">
        <div class="modal-header"><h3>提交任务</h3></div>
        <div class="modal-body">
          <div class="form-group">
            <label>任务内容 <span style="color:var(--error)">*</span></label>
            <textarea class="form-input" v-model="submitForm.content" rows="4" placeholder="输入任务描述..." style="resize:vertical" />
          </div>
          <div class="form-group">
            <label>目标节点</label>
            <select class="form-select" v-model="submitForm.target_node_id">
              <option value="">自动分配</option>
              <option v-for="n in nodeList" :key="n.id" :value="n.id">{{ n.name || n.id }}</option>
            </select>
          </div>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="showSubmitModal = false">取消</button>
          <button class="btn btn-primary" :disabled="submitSubmitting" @click="submitTask">{{ submitSubmitting ? '提交中...' : '确认提交' }}</button>
        </div>
      </div>
    </div>
  </div>
</template>
