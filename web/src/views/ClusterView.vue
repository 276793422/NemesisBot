<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const activeTab = ref('status')
const status = ref<any>({})
const config = ref<any>({})
const peersContent = ref('')
const loading = ref(true)
const editing = ref(false)
const editConfig = ref('')

async function loadStatus() {
  try {
    const data = await request('cluster', 'status')
    status.value = data || {}
  } catch { /* ignore */ }
}

async function loadConfig() {
  try {
    const data = await request('cluster', 'config.get')
    config.value = data || {}
    editConfig.value = JSON.stringify(data, null, 2)
  } catch { /* ignore */ }
}

async function loadPeers() {
  try {
    const data = await request('cluster', 'peers')
    peersContent.value = data?.peers || ''
  } catch { /* ignore */ }
}

async function saveConfig() {
  try {
    const parsed = JSON.parse(editConfig.value)
    await request('cluster', 'config.save', parsed)
    toast.success('已保存')
    editing.value = false
    await loadConfig()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

onMounted(async () => {
  await Promise.all([loadStatus(), loadConfig(), loadPeers()])
  loading.value = false
})
</script>

<template>
  <div class="page-cluster">
    <div class="page-header"><h2>集群管理</h2></div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <div class="tabs">
          <button class="tab" :class="{ active: activeTab === 'status' }" @click="activeTab = 'status'">状态</button>
          <button class="tab" :class="{ active: activeTab === 'config' }" @click="activeTab = 'config'">配置</button>
          <button class="tab" :class="{ active: activeTab === 'peers' }" @click="activeTab = 'peers'">节点</button>
        </div>

        <!-- Status -->
        <div v-if="activeTab === 'status'">
          <div class="stats-grid">
            <div class="stat-card">
              <div class="stat-label">启用状态</div>
              <div class="stat-value"><span class="badge" :class="status.config?.enabled ? 'badge-success' : 'badge-neutral'">{{ status.config?.enabled ? '已启用' : '未启用' }}</span></div>
            </div>
            <div class="stat-card">
              <div class="stat-label">节点数</div>
              <div class="stat-value">{{ status.peers_count || 0 }}</div>
            </div>
            <div class="stat-card">
              <div class="stat-label">角色</div>
              <div class="stat-value">{{ status.role || '--' }}</div>
            </div>
          </div>
        </div>

        <!-- Config -->
        <div v-if="activeTab === 'config'">
          <div class="card">
            <div class="card-header">
              <h3>集群配置</h3>
              <div style="display: flex; gap: var(--space-2);">
                <template v-if="!editing">
                  <button class="btn btn-sm" @click="editing = true">编辑</button>
                </template>
                <template v-else>
                  <button class="btn btn-sm" @click="editing = false">取消</button>
                  <button class="btn btn-sm btn-primary" @click="saveConfig">保存</button>
                </template>
              </div>
            </div>
            <div class="card-body">
              <div v-if="editing">
                <textarea class="form-textarea" style="min-height: 60vh; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="editConfig"></textarea>
              </div>
              <div v-else>
                <div class="settings-grid">
                  <template v-for="(value, key) in config" :key="key">
                    <template v-if="typeof value !== 'object'">
                      <span class="settings-key">{{ key }}</span>
                      <span class="settings-value">{{ String(value) }}</span>
                    </template>
                  </template>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Peers -->
        <div v-if="activeTab === 'peers'">
          <div v-if="!peersContent" class="empty-state">
            <h3>暂无节点</h3>
            <p>使用 CLI 命令添加集群节点</p>
          </div>
          <div v-if="peersContent" class="card">
            <div class="card-header"><h3>peers.toml</h3></div>
            <div class="card-body">
              <pre style="white-space: pre-wrap; font-family: var(--font-mono); font-size: var(--text-sm);">{{ peersContent }}</pre>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
