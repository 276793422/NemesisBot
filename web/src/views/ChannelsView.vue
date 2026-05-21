<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface ChannelInfo { name: string; enabled?: boolean; config?: any }

const channels = ref<ChannelInfo[]>([])
const loading = ref(true)
const selectedChannel = ref<string | null>(null)
const channelDetail = ref<any>({})
const editConfig = ref('')
const editing = ref(false)

const channelLabels: Record<string, string> = {
  web: 'Web', websocket: 'WebSocket', telegram: 'Telegram', discord: 'Discord',
  whatsapp: 'WhatsApp', feishu: '飞书', slack: 'Slack', line: 'LINE',
  onebot: 'OneBot', qq: 'QQ', dingtalk: '钉钉', maixcam: 'MaixCam', external: 'External',
}

async function loadChannels() {
  try {
    const data = await request('channels', 'list')
    channels.value = data?.channels || []
  } catch (e: any) {
    toast.error('加载失败: ' + e)
  }
  loading.value = false
}

async function loadChannelDetail(name: string) {
  try {
    const data = await request('channels', 'get', { name })
    channelDetail.value = data?.config || {}
    editConfig.value = JSON.stringify(data?.config || {}, null, 2)
    selectedChannel.value = name
    editing.value = false
  } catch (e: any) {
    toast.error('加载详情失败: ' + e)
  }
}

async function updateChannel() {
  if (!selectedChannel.value) return
  try {
    const config = JSON.parse(editConfig.value)
    await request('channels', 'update', { name: selectedChannel.value, config })
    toast.success('已保存')
    editing.value = false
    await loadChannels()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

onMounted(loadChannels)
</script>

<template>
  <div class="page-channels">
    <div class="page-header"><h2>通道管理</h2></div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading" style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-4); min-height: 400px;">
        <!-- Channel grid -->
        <div style="display: grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap: var(--space-3); align-content: start;">
          <div v-for="ch in channels" :key="ch.name" class="card" style="cursor: pointer;"
            :style="{ borderColor: selectedChannel === ch.name ? 'var(--accent)' : '' }"
            @click="loadChannelDetail(ch.name)">
            <div style="padding: var(--space-4); text-align: center;">
              <div style="font-weight: 600; margin-bottom: var(--space-2);">{{ channelLabels[ch.name] || ch.name }}</div>
              <span class="badge" :class="ch.enabled ? 'badge-success' : 'badge-neutral'">
                {{ ch.enabled ? '已启用' : '未启用' }}
              </span>
            </div>
          </div>
        </div>

        <!-- Channel detail -->
        <div class="card">
          <div class="card-header">
            <h3>{{ selectedChannel ? (channelLabels[selectedChannel] || selectedChannel) : '选择通道' }}</h3>
            <div v-if="selectedChannel && !editing" style="display: flex; gap: var(--space-2);">
              <button class="btn btn-sm" @click="editing = true">编辑</button>
            </div>
          </div>
          <div class="card-body">
            <div v-if="!selectedChannel" class="empty-state" style="padding: var(--space-6);">
              <p>从左侧选择一个通道查看配置</p>
            </div>
            <div v-else-if="editing">
              <div style="padding: var(--space-3); margin-bottom: var(--space-3); background: var(--warning-bg, #fef3cd); border: 1px solid var(--warning, #e5a00d); border-radius: var(--radius-md); font-size: var(--text-sm); color: var(--text-secondary);">
                注意：敏感字段（如 API Key、Token）已被遮蔽显示（含 **** ）。如需修改，请将遮蔽值替换为真实值；如保持遮蔽值不变，保存后该字段将被覆盖为遮蔽值。
              </div>
              <textarea class="form-textarea" style="min-height: 350px; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="editConfig"></textarea>
              <div style="margin-top: var(--space-3); display: flex; justify-content: flex-end; gap: var(--space-2);">
                <button class="btn" @click="editing = false; loadChannelDetail(selectedChannel!)">取消</button>
                <button class="btn btn-primary" @click="updateChannel">保存</button>
              </div>
            </div>
            <div v-else>
              <div class="settings-grid">
                <template v-for="(value, key) in channelDetail" :key="key">
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
    </div>
  </div>
</template>
