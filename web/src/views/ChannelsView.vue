<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import VoiceTab from './VoiceTab.vue'
import SimpleFieldForm from '../components/SimpleFieldForm.vue'
import { CHANNEL_FIELD_META } from '../lib/friendlyFields'

defineProps<{ embedded?: boolean }>()

const { request } = useWSAPI()
const toast = useToast()

interface ChannelInfo { name: string; enabled?: boolean; config?: any }

const channels = ref<ChannelInfo[]>([])
const loading = ref(true)
const selectedChannel = ref<string | null>(null)
const channelDetail = ref<Record<string, any>>({})
const formModel = ref<Record<string, any>>({})
const editing = ref(false)
const activeTab = ref<'local' | 'cloud' | 'voice'>('local')
const showRaw = ref(false)
const editConfig = ref('')

const channelLabels: Record<string, string> = {
  web: 'Web', websocket: 'WebSocket', telegram: 'Telegram', discord: 'Discord',
  whatsapp: 'WhatsApp', feishu: '飞书', slack: 'Slack', line: 'LINE',
  onebot: 'OneBot', qq: 'QQ', dingtalk: '钉钉', maixcam: 'MaixCam', external: 'External',
  bluesky: 'Bluesky', email: 'Email', irc: 'IRC', matrix: 'Matrix',
  mastodon: 'Mastodon', signal: 'Signal',
}

const localNames = new Set(['web', 'websocket'])

const localChannels = computed(() => channels.value.filter(ch => localNames.has(ch.name)))
const cloudChannels = computed(() => channels.value.filter(ch => !localNames.has(ch.name)))
const isVoiceTab = computed(() => activeTab.value === 'voice')

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
    formModel.value = { ...(data?.config || {}) }
    editConfig.value = JSON.stringify(data?.config || {}, null, 2)
    selectedChannel.value = name
    editing.value = false
    showRaw.value = false
  } catch (e: any) {
    toast.error('加载详情失败: ' + e)
  }
}

async function updateChannel() {
  if (!selectedChannel.value) return
  try {
    let config: any
    if (showRaw.value) {
      config = JSON.parse(editConfig.value)
    } else {
      // Merge form primitives back into full config (preserve nested objects)
      config = { ...channelDetail.value, ...formModel.value }
    }
    await request('channels', 'update', { name: selectedChannel.value, config })
    toast.success('已保存')
    editing.value = false
    showRaw.value = false
    await loadChannels()
    await loadChannelDetail(selectedChannel.value)
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

async function toggleEnabled(ch: ChannelInfo) {
  try {
    const data = await request('channels', 'get', { name: ch.name })
    const config = { ...(data?.config || {}), enabled: !ch.enabled }
    await request('channels', 'update', { name: ch.name, config })
    toast.success(config.enabled ? '已启用' : '已关闭')
    await loadChannels()
    if (selectedChannel.value === ch.name) await loadChannelDetail(ch.name)
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

onMounted(loadChannels)
</script>

<template>
  <div :class="embedded ? 'channels-embed' : 'page-channels'">
    <div v-if="!embedded" class="page-header"><h2>通道管理</h2></div>
    <div :class="embedded ? '' : 'page-body'">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <div style="display: flex; border-bottom: 1px solid var(--border); margin-bottom: var(--space-3);">
          <button class="ch-tab" :class="{ active: activeTab === 'local' }" @click="activeTab = 'local'">本地通道</button>
          <button class="ch-tab" :class="{ active: activeTab === 'cloud' }" @click="activeTab = 'cloud'">云端通道</button>
          <button class="ch-tab" :class="{ active: activeTab === 'voice' }" @click="activeTab = 'voice'">语音通道</button>
        </div>

        <div v-if="isVoiceTab">
          <VoiceTab />
        </div>

        <div v-else style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-4); min-height: 400px;">
          <div>
            <div v-if="activeTab === 'local'" style="display: grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap: var(--space-3);">
              <div v-for="ch in localChannels" :key="ch.name" class="card" style="cursor: pointer;"
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

            <div v-if="activeTab === 'cloud'" style="display: grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap: var(--space-3);">
              <div v-for="ch in cloudChannels" :key="ch.name" class="card" style="cursor: pointer;"
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
          </div>

          <div class="card">
            <div class="card-header">
              <h3>{{ selectedChannel ? (channelLabels[selectedChannel] || selectedChannel) : '选择通道' }}</h3>
              <div v-if="selectedChannel && !editing" style="display: flex; gap: var(--space-2);">
                <button class="btn btn-sm" @click="toggleEnabled({ name: selectedChannel, enabled: channelDetail.enabled })">
                  {{ channelDetail.enabled ? '关闭' : '启用' }}
                </button>
                <button class="btn btn-sm btn-primary" @click="editing = true; formModel = { ...channelDetail }">配置</button>
              </div>
            </div>
            <div class="card-body">
              <div v-if="!selectedChannel" class="empty-state" style="padding: var(--space-6);">
                <p>从左侧选择一个通道。常见只需填 Token 并启用，无需编辑 JSON。</p>
              </div>
              <div v-else-if="editing">
                <p class="form-hint" style="margin-bottom: var(--space-3);">按字段填写即可；密钥类请粘贴新值，留空或保持遮蔽则不要改。</p>
                <SimpleFieldForm v-model="formModel" :meta-table="CHANNEL_FIELD_META" />
                <div style="margin-top: var(--space-4);">
                  <button type="button" class="btn btn-sm" @click="showRaw = !showRaw">{{ showRaw ? '隐藏 JSON' : '高级：原始 JSON' }}</button>
                </div>
                <textarea v-if="showRaw" class="form-textarea" style="min-height: 200px; margin-top: var(--space-2); font-family: var(--font-mono); font-size: var(--text-xs);" v-model="editConfig"></textarea>
                <div style="margin-top: var(--space-3); display: flex; justify-content: flex-end; gap: var(--space-2);">
                  <button class="btn" @click="editing = false; loadChannelDetail(selectedChannel!)">取消</button>
                  <button class="btn btn-primary" @click="updateChannel">保存</button>
                </div>
              </div>
              <div v-else>
                <div class="settings-grid">
                  <template v-for="(value, key) in channelDetail" :key="key">
                    <template v-if="typeof value !== 'object'">
                      <span class="settings-key">{{ CHANNEL_FIELD_META[key as string]?.label || key }}</span>
                      <span class="settings-value">{{ typeof value === 'boolean' ? (value ? '是' : '否') : String(value) }}</span>
                    </template>
                  </template>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.ch-tab {
  padding: var(--space-2) var(--space-4);
  border: none;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  border-bottom: 2px solid transparent;
  font: inherit;
}
.ch-tab.active {
  color: var(--accent);
  border-bottom-color: var(--accent);
}
</style>
