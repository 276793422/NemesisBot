<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const activeTab = ref('boot')
const bootContent = ref('')
const heartbeatContent = ref('')
const cronJobs = ref<any[]>([])
const loading = ref(true)
const editing = ref(false)
const editContent = ref('')
const showAddCron = ref(false)
const cronForm = ref({ name: '', cron: '', channel: '', prompt: '', enabled: true })

async function loadBoot() {
  try {
    const data = await request('tasks', 'boot.get')
    bootContent.value = data?.content || ''
  } catch { /* ignore */ }
}

async function loadHeartbeat() {
  try {
    const data = await request('tasks', 'heartbeat.get')
    heartbeatContent.value = data?.content || ''
  } catch { /* ignore */ }
}

async function loadCronJobs() {
  try {
    const data = await request('tasks', 'cron.list')
    cronJobs.value = data?.jobs || []
  } catch { /* ignore */ }
}

function startEdit() {
  if (activeTab.value === 'boot') editContent.value = bootContent.value
  else editContent.value = heartbeatContent.value
  editing.value = true
}

async function saveContent() {
  const cmd = activeTab.value === 'boot' ? 'boot.save' : 'heartbeat.save'
  try {
    await request('tasks', cmd, { content: editContent.value })
    toast.success('已保存')
    if (activeTab.value === 'boot') bootContent.value = editContent.value
    else heartbeatContent.value = editContent.value
    editing.value = false
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

async function addCronJob() {
  if (!cronForm.value.name || !cronForm.value.cron) { toast.warn('请填写名称和 Cron 表达式'); return }
  try {
    await request('tasks', 'cron.add', cronForm.value)
    toast.success('已添加')
    showAddCron.value = false
    cronForm.value = { name: '', cron: '', channel: '', prompt: '', enabled: true }
    await loadCronJobs()
  } catch (e: any) {
    toast.error('添加失败: ' + e)
  }
}

async function deleteCronJob(id: string) {
  if (!confirm('确定删除此定时任务？')) return
  try {
    await request('tasks', 'cron.delete', { id })
    toast.success('已删除')
    await loadCronJobs()
  } catch (e: any) {
    toast.error('删除失败: ' + e)
  }
}

function switchTab(tab: string) {
  activeTab.value = tab
  editing.value = false
}

onMounted(async () => {
  await Promise.all([loadBoot(), loadHeartbeat(), loadCronJobs()])
  loading.value = false
})
</script>

<template>
  <div class="page-tasks">
    <div class="page-header"><h2>任务管理</h2></div>
    <div class="page-body">
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'boot' }" @click="switchTab('boot')">启动任务</button>
        <button class="tab" :class="{ active: activeTab === 'heartbeat' }" @click="switchTab('heartbeat')">心跳任务</button>
        <button class="tab" :class="{ active: activeTab === 'cron' }" @click="switchTab('cron')">定时任务</button>
      </div>

      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <!-- Boot/Heartbeat editor -->
      <div v-if="!loading && (activeTab === 'boot' || activeTab === 'heartbeat')">
        <div class="card">
          <div class="card-header">
            <h3>{{ activeTab === 'boot' ? 'BOOT.md' : 'HEARTBEAT.md' }}</h3>
            <div style="display: flex; gap: var(--space-2);">
              <template v-if="!editing">
                <button class="btn btn-sm" @click="startEdit">编辑</button>
              </template>
              <template v-else>
                <button class="btn btn-sm" @click="editing = false">取消</button>
                <button class="btn btn-sm btn-primary" @click="saveContent">保存</button>
              </template>
            </div>
          </div>
          <div class="card-body">
            <p style="color: var(--text-muted); font-size: var(--text-sm); margin-bottom: var(--space-3);">
              {{ activeTab === 'boot' ? 'Agent 每次启动时执行的检查清单' : 'Agent 心跳轮询时执行的任务' }}
            </p>
            <div v-if="editing">
              <textarea class="form-textarea" style="min-height: 400px; font-family: var(--font-mono); font-size: var(--text-sm);" v-model="editContent"></textarea>
            </div>
            <div v-else class="markdown-body">
              <pre style="white-space: pre-wrap;">{{ (activeTab === 'boot' ? bootContent : heartbeatContent) || '（空文件）' }}</pre>
            </div>
          </div>
        </div>
      </div>

      <!-- Cron jobs -->
      <div v-if="!loading && activeTab === 'cron'">
        <div style="display: flex; justify-content: flex-end; margin-bottom: var(--space-4);">
          <button class="btn btn-primary" @click="showAddCron = !showAddCron">{{ showAddCron ? '取消' : '+ 添加任务' }}</button>
        </div>

        <div v-if="showAddCron" class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>添加定时任务</h3></div>
          <div class="card-body">
            <div style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-3);">
              <div class="form-group">
                <label class="form-label">名称 *</label>
                <input class="form-input" v-model="cronForm.name" placeholder="任务名称">
              </div>
              <div class="form-group">
                <label class="form-label">Cron 表达式 *</label>
                <input class="form-input" v-model="cronForm.cron" placeholder="0 9 * * *">
              </div>
              <div class="form-group">
                <label class="form-label">通道</label>
                <input class="form-input" v-model="cronForm.channel" placeholder="web">
              </div>
              <div class="form-group">
                <label class="form-label">启用</label>
                <div class="toggle" :class="{ active: cronForm.enabled }" @click="cronForm.enabled = !cronForm.enabled"></div>
              </div>
            </div>
            <div class="form-group" style="margin-top: var(--space-3);">
              <label class="form-label">提示词</label>
              <textarea class="form-textarea" v-model="cronForm.prompt" placeholder="任务提示词"></textarea>
            </div>
            <button class="btn btn-primary" style="margin-top: var(--space-3);" @click="addCronJob">添加</button>
          </div>
        </div>

        <div v-if="cronJobs.length === 0" class="empty-state">
          <h3>暂无定时任务</h3>
          <p>点击"添加任务"创建 Cron 定时任务</p>
        </div>

        <div v-if="cronJobs.length > 0" class="table-wrap">
          <table>
            <thead><tr><th>名称</th><th>Cron</th><th>通道</th><th>状态</th><th>操作</th></tr></thead>
            <tbody>
              <tr v-for="job in cronJobs" :key="job.id || job.name">
                <td style="font-weight: 500;">{{ job.name }}</td>
                <td><code>{{ job.cron }}</code></td>
                <td>{{ job.channel || '--' }}</td>
                <td><span class="badge" :class="job.enabled ? 'badge-success' : 'badge-neutral'">{{ job.enabled ? '启用' : '禁用' }}</span></td>
                <td><button class="btn btn-sm btn-danger" @click="deleteCronJob(job.id || job.name)">删除</button></td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>
  </div>
</template>
