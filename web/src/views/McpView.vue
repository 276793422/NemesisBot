<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface McpServer { name: string; command?: string; args?: string[]; env?: string[]; timeout?: number }

const activeTab = ref('servers')
const servers = ref<McpServer[]>([])
const enabled = ref(false)
const loading = ref(true)
const showAdd = ref(false)
const addForm = ref({ name: '', command: '', args: '', env: '' })

async function loadStatus() {
  try {
    const data = await request('mcp', 'status')
    enabled.value = data?.enabled || false
  } catch { /* ignore */ }
}

async function loadServers() {
  try {
    const data = await request('mcp', 'servers')
    servers.value = data?.servers || []
  } catch (e: any) {
    toast.error('加载失败: ' + e)
  }
  loading.value = false
}

async function addServer() {
  if (!addForm.value.name || !addForm.value.command) { toast.warn('请填写名称和命令'); return }
  try {
    const payload: any = { name: addForm.value.name, command: addForm.value.command }
    if (addForm.value.args) payload.args = addForm.value.args.split(' ')
    if (addForm.value.env) payload.env = addForm.value.env.split(' ')
    await request('mcp', 'server.add', payload)
    toast.success('已添加')
    showAdd.value = false
    addForm.value = { name: '', command: '', args: '', env: '' }
    await loadServers()
  } catch (e: any) {
    toast.error('添加失败: ' + e)
  }
}

async function deleteServer(name: string) {
  if (!confirm(`确定删除服务器 "${name}" 吗？`)) return
  try {
    await request('mcp', 'server.delete', { name })
    toast.success('已删除')
    await loadServers()
  } catch (e: any) {
    toast.error('删除失败: ' + e)
  }
}

onMounted(async () => {
  await Promise.all([loadStatus(), loadServers()])
})
</script>

<template>
  <div class="page-mcp">
    <div class="page-header">
      <h2>MCP 管理</h2>
      <div class="page-header-actions">
        <span class="badge" :class="enabled ? 'badge-success' : 'badge-neutral'">{{ enabled ? '已启用' : '未启用' }}</span>
      </div>
    </div>
    <div class="page-body">
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'servers' }" @click="activeTab = 'servers'">服务器</button>
        <button class="tab" :class="{ active: activeTab === 'add' }" @click="activeTab = 'add'">添加</button>
      </div>

      <!-- Servers list -->
      <div v-if="activeTab === 'servers'">
        <div v-if="loading" style="text-align: center; padding: var(--space-8);">
          <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
        </div>
        <div v-if="!loading && servers.length === 0" class="empty-state">
          <h3>暂无 MCP 服务器</h3>
          <p>点击"添加"Tab 配置第一个 MCP 服务器</p>
        </div>
        <div v-if="!loading && servers.length > 0" class="table-wrap">
          <table>
            <thead>
              <tr><th>名称</th><th>命令</th><th>参数</th><th>超时</th><th>操作</th></tr>
            </thead>
            <tbody>
              <tr v-for="s in servers" :key="s.name">
                <td style="font-weight: 500;">{{ s.name }}</td>
                <td><code>{{ s.command || '--' }}</code></td>
                <td>{{ (s.args || []).join(' ') || '--' }}</td>
                <td>{{ s.timeout || 0 }}s</td>
                <td>
                  <button class="btn btn-sm btn-danger" @click="deleteServer(s.name)">删除</button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>

      <!-- Add server -->
      <div v-if="activeTab === 'add'">
        <div class="card" style="max-width: 500px;">
          <div class="card-header"><h3>添加 MCP 服务器</h3></div>
          <div class="card-body">
            <div class="form-group">
              <label class="form-label">名称 *</label>
              <input class="form-input" v-model="addForm.name" placeholder="例如: filesystem">
            </div>
            <div class="form-group">
              <label class="form-label">命令 *</label>
              <input class="form-input" v-model="addForm.command" placeholder="例如: npx">
            </div>
            <div class="form-group">
              <label class="form-label">参数（空格分隔）</label>
              <input class="form-input" v-model="addForm.args" placeholder="例如: -y @modelcontextprotocol/server-filesystem /path">
            </div>
            <div class="form-group">
              <label class="form-label">环境变量（空格分隔）</label>
              <input class="form-input" v-model="addForm.env" placeholder="例如: KEY=value">
            </div>
            <button class="btn btn-primary" @click="addServer">添加</button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
