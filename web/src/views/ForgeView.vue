<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const enabled = ref(false)
const artifacts = ref<any[]>([])
const loading = ref(true)

function formatSize(bytes: number | undefined): string {
  if (!bytes) return '--'
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
  return (bytes / (1024 * 1024 * 1024)).toFixed(1) + ' GB'
}

async function loadStatus() {
  try {
    const data = await request('forge', 'status')
    enabled.value = data?.enabled || false
  } catch { /* ignore */ }
}

async function loadArtifacts() {
  try {
    const data = await request('forge', 'artifacts')
    artifacts.value = data?.artifacts || []
  } catch { /* ignore */ }
  loading.value = false
}

async function toggleForge() {
  try {
    await request('forge', 'config.save', { enabled: !enabled.value })
    enabled.value = !enabled.value
    toast.success(enabled.value ? '已启用' : '已禁用')
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

async function triggerReflect() {
  try {
    const data = await request('forge', 'reflect')
    if (data?.triggered) {
      toast.success(data?.message || '反思已触发')
    } else {
      toast.info(data?.message || '反思功能尚未集成')
    }
  } catch (e: any) {
    toast.error('触发失败: ' + e)
  }
}

onMounted(async () => {
  await Promise.all([loadStatus(), loadArtifacts()])
})
</script>

<template>
  <div class="page-forge">
    <div class="page-header">
      <h2>Forge 自学习</h2>
      <div class="page-header-actions">
        <button class="btn" @click="triggerReflect">触发反思</button>
      </div>
    </div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <!-- Status card -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>状态</h3>
            <div style="display: flex; align-items: center; gap: var(--space-3);">
              <span class="badge" :class="enabled ? 'badge-success' : 'badge-neutral'">{{ enabled ? '已启用' : '未启用' }}</span>
              <div class="toggle" :class="{ active: enabled }" @click="toggleForge"></div>
            </div>
          </div>
          <div class="card-body">
            <p style="color: var(--text-secondary); font-size: var(--text-sm);">
              Forge 自学习框架基于 Read → Execute → Reflect → Write 核心循环，支持 Collector、Reflector、Factory、Registry 等子系统。
            </p>
          </div>
        </div>

        <!-- Artifacts -->
        <div class="card">
          <div class="card-header"><h3>Artifacts</h3></div>
          <div class="card-body">
            <div v-if="artifacts.length === 0" class="empty-state" style="padding: var(--space-4);">
              <p>暂无学习产物</p>
            </div>
            <div v-else class="table-wrap">
              <table>
                <thead><tr><th>名称</th><th>类型</th><th>大小</th></tr></thead>
                <tbody>
                  <tr v-for="(a, idx) in artifacts" :key="idx">
                    <td>{{ a.name || '--' }}</td>
                    <td><span class="badge badge-info">{{ a.type || '--' }}</span></td>
                    <td>{{ a.type === 'file' ? formatSize(a.size) : '--' }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
