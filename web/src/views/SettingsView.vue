<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { httpGet } from '../composables/useWebSocket'

const config = ref<Record<string, Record<string, any>>>({})
const loading = ref(true)
const error = ref('')

const sensitiveKeys = ['key', 'token', 'secret', 'password', 'auth', 'credential']

function isSensitive(key: string): boolean {
  const lower = key.toLowerCase()
  return sensitiveKeys.some(s => lower.includes(s))
}

function formatValue(value: any): string {
  if (value === null || value === undefined) return '--'
  if (typeof value === 'object') return JSON.stringify(value, null, 2)
  return String(value)
}

function maskValue(key: string, value: any): string {
  if (isSensitive(key) && typeof value === 'string' && value.length > 0) {
    return value.substring(0, 4) + '****'
  }
  return formatValue(value)
}

onMounted(async () => {
  try {
    config.value = await httpGet<Record<string, Record<string, any>>>('/api/config')
  } catch (err) {
    console.error('[Settings] Failed to load:', err)
    error.value = '加载配置失败'
  }
  loading.value = false
})
</script>

<template>
  <div class="page-settings">
    <div class="page-header">
      <h2>设置</h2>
    </div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading && error" style="text-align: center; padding: var(--space-8); color: var(--error);">{{ error }}</div>

      <div v-if="!loading && !error">
        <div v-for="(sectionData, section) in config" :key="section" class="settings-section">
          <h3>{{ section }}</h3>
          <div class="settings-grid">
            <template v-for="(value, key) in sectionData" :key="key">
              <template v-if="typeof value !== 'object'">
                <div class="settings-key">{{ key }}</div>
                <div class="settings-value">{{ maskValue(key as string, value) }}</div>
              </template>
            </template>
          </div>
        </div>

        <div v-if="!loading && Object.keys(config).length === 0" class="empty-state">
          <p>暂无配置信息</p>
        </div>
      </div>
    </div>
  </div>
</template>
