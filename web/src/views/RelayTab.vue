<script setup lang="ts">
// 中继通道（goal 批次三）：通道页【中继通道】tab。
//
// 双区布局（goal 钉死）：
// - 服务端区：本机作为中继服务端（正常启动默认开）。开关为**运行时态**
//   （POST /api/relay/enabled，不写 config.json——重启恢复默认开）；
//   桥入设备实时列表与状态页 /relay 数据同源（同读 RelayServer 设备表）；
//   状态页入口链接（ws token 门在状态页侧，此处不重复）。
// - 客户端区：本机作为桥设备出连远端中继。relay_url/token/access_token
//   三配置持久化（config.set_field → bridge.client.*）；连接状态与手动
//   重连（POST /api/relay/client/reconnect）。
//
// 鉴权语义：三个 HTTP 端点走 dashboard 信任边界（与 /api/status 同）；
// 状态页的 admin cookie 门是「知道 ws token」的带外通道，两者受众不同。
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { httpGet } from '../composables/useWebSocket'
import { apiUrl } from '../lib/appBase'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

// ---- overview（服务端态 + 客户端态，5s 轮询） ----
interface RelayDevice {
  node_id: string
  name: string
  version: string
  connected_at: number
  bytes_up: number
  bytes_down: number
  online: boolean
}
interface RelayClientStatus {
  enabled: boolean
  state: 'connecting' | 'connected' | 'rejected' | 'disconnected'
  relay_url: string
  node_id: string
  last_error: string | null
  updated_at: number
}
interface RelayOverview {
  server: { enabled: boolean; full_mode: boolean; devices: RelayDevice[] } | null
  client: RelayClientStatus | null
}

const overview = ref<RelayOverview>({ server: null, client: null })
let pollTimer: ReturnType<typeof setInterval> | null = null

async function loadOverview() {
  try {
    overview.value = await httpGet<RelayOverview>('/api/relay/overview')
  } catch {
    // 轮询失败静默（本机 web server 短暂重启时别刷错误提示）
  }
}

// ---- 服务端开关（运行时态，不写配置） ----
const toggling = ref(false)

async function toggleServer(on: boolean) {
  toggling.value = true
  try {
    const res = await fetch(apiUrl('/api/relay/enabled'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ on }),
    })
    if (!res.ok) throw new Error('HTTP ' + res.status)
    toast.success(on ? '中继服务端已开启' : '中继服务端已关闭')
    await loadOverview()
  } catch (e: any) {
    toast.error('开关失败: ' + e)
  } finally {
    toggling.value = false
  }
}

// ---- 客户端配置（config.get 读 / config.set_field 写，只提交变更字段） ----
const cfgRelayUrl = ref('')
const cfgToken = ref('')
const cfgAccessToken = ref('')
// 令牌/密码明文显示开关（默认星号遮蔽，点「显示」查看原文——原文本就
// 存本机 config.json，此处只控制前端渲染）。
const showToken = ref(false)
const showAccess = ref(false)
// 加载时的原始值（token/access_token 是遮蔽值）——保存时对比，未变不写，
// 避免「遮蔽值覆盖真实值」（ChannelsView 已知坑，此处直接规避）。
const _loaded = ref<{ relay_url: string; token: string; access_token: string }>({
  relay_url: '',
  token: '',
  access_token: '',
})
const savingCfg = ref(false)

async function loadClientConfig() {
  try {
    const data = await request('config', 'get')
    const bc = data?.bridge?.client
    cfgRelayUrl.value = bc?.relay_url || ''
    cfgToken.value = bc?.token || ''
    cfgAccessToken.value = bc?.access_token || ''
    _loaded.value = {
      relay_url: cfgRelayUrl.value,
      token: cfgToken.value,
      access_token: cfgAccessToken.value,
    }
  } catch (e: any) {
    toast.error('加载客户端配置失败: ' + e)
  }
}

async function saveClientConfig() {
  savingCfg.value = true
  try {
    const fields: Array<['relay_url' | 'token' | 'access_token', string, string]> = [
      ['relay_url', 'bridge.client.relay_url', cfgRelayUrl.value.trim()],
      ['token', 'bridge.client.token', cfgToken.value],
      ['access_token', 'bridge.client.access_token', cfgAccessToken.value],
    ]
    for (const [key, path, value] of fields) {
      // 遮蔽值原样 = 用户未改 → 跳过（真实值不被覆盖）
      if (_loaded.value[key] === value) continue
      await request('config', 'set_field', { path, value })
    }
    toast.success('已保存（重启后生效）')
    await loadClientConfig()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  } finally {
    savingCfg.value = false
  }
}

// ---- 手动重连 ----
const reconnecting = ref(false)

async function manualReconnect() {
  reconnecting.value = true
  try {
    const res = await fetch(apiUrl('/api/relay/client/reconnect'), { method: 'POST' })
    if (!res.ok) throw new Error('HTTP ' + res.status)
    toast.success('已通知重连')
    // 状态翻转需要一拍，立即刷一次
    setTimeout(loadOverview, 500)
  } catch (e: any) {
    toast.error('重连失败: ' + e)
  } finally {
    setTimeout(() => { reconnecting.value = false }, 800)
  }
}

// ---- 展示辅助 ----
const clientStateLabels: Record<string, string> = {
  connecting: '连接中',
  connected: '已连接',
  rejected: '被拒绝',
  disconnected: '未连接',
}
const clientStateBadge: Record<string, string> = {
  connecting: 'badge-warning',
  connected: 'badge-success',
  rejected: 'badge-error',
  disconnected: 'badge-neutral',
}
const clientState = computed(() => overview.value.client?.state || null)

function formatBytes(n: number): string {
  if (n < 1024) return n + ' B'
  if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KB'
  return (n / 1024 / 1024).toFixed(1) + ' MB'
}
function formatTime(unixSecs: number): string {
  if (!unixSecs) return '-'
  return new Date(unixSecs * 1000).toLocaleTimeString()
}

onMounted(async () => {
  await Promise.all([loadOverview(), loadClientConfig()])
  pollTimer = setInterval(loadOverview, 5000)
})
onUnmounted(() => {
  if (pollTimer) clearInterval(pollTimer)
})
</script>

<template>
  <div style="display: flex; flex-direction: column; gap: var(--space-4);">

    <!-- ===== 服务端区 ===== -->
    <div class="card">
      <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
        <h3 style="margin: 0;">中继服务端</h3>
        <div style="display: flex; align-items: center; gap: var(--space-2);">
          <a v-if="overview.server" href="/relay" target="_blank" class="btn btn-sm" style="text-decoration: none;">状态页</a>
          <template v-if="overview.server">
            <label style="display: flex; align-items: center; gap: var(--space-1); font-size: var(--text-sm); cursor: pointer;">
              <input type="checkbox" :checked="overview.server.enabled" :disabled="toggling"
                @change="toggleServer(($event.target as HTMLInputElement).checked)" />
              运行中
            </label>
          </template>
        </div>
      </div>
      <div class="card-body">
        <div v-if="!overview.server" class="empty-state" style="padding: var(--space-4);">
          <p style="margin: 0; font-size: var(--text-sm);">本机未开启中继服务端（config.json → bridge.server.token 配置接入门令牌后重启生效）。</p>
        </div>
        <template v-else>
          <div v-if="overview.server.devices.length === 0" class="empty-state" style="padding: var(--space-4);">
            <p style="margin: 0; font-size: var(--text-sm);">{{ overview.server.enabled ? '暂无桥入设备' : '中继已关闭——开启后设备可接入' }}</p>
          </div>
          <table v-else style="width: 100%; border-collapse: collapse; font-size: var(--text-sm);">
            <thead>
              <tr style="text-align: left; color: var(--text-secondary); border-bottom: 1px solid var(--border);">
                <th style="padding: var(--space-2);">设备</th>
                <th style="padding: var(--space-2);">节点 ID</th>
                <th style="padding: var(--space-2);">状态</th>
                <th style="padding: var(--space-2);">接入时间</th>
                <th style="padding: var(--space-2);">上/下行</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="d in overview.server.devices" :key="d.node_id" style="border-bottom: 1px solid var(--border);">
                <td style="padding: var(--space-2);">{{ d.name }} <span style="color: var(--text-secondary); font-size: var(--text-xs);">{{ d.version }}</span></td>
                <td style="padding: var(--space-2); font-family: var(--font-mono); font-size: var(--text-xs);">{{ d.node_id }}</td>
                <td style="padding: var(--space-2);"><span class="badge" :class="d.online ? 'badge-success' : 'badge-neutral'">{{ d.online ? '在线' : '离线' }}</span></td>
                <td style="padding: var(--space-2);">{{ formatTime(d.connected_at) }}</td>
                <td style="padding: var(--space-2);">{{ formatBytes(d.bytes_up) }} / {{ formatBytes(d.bytes_down) }}</td>
              </tr>
            </tbody>
          </table>
        </template>
      </div>
    </div>

    <!-- ===== 客户端区 ===== -->
    <div class="card">
      <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
        <h3 style="margin: 0;">桥客户端（本机接入远端中继）</h3>
        <div v-if="overview.client" style="display: flex; align-items: center; gap: var(--space-2);">
          <span class="badge" :class="clientStateBadge[clientState || 'disconnected']">
            {{ clientStateLabels[clientState || 'disconnected'] }}
          </span>
          <button class="btn btn-sm" :disabled="reconnecting" @click="manualReconnect">手动重连</button>
        </div>
      </div>
      <div class="card-body">
        <div v-if="overview.client?.last_error"
          style="margin-bottom: var(--space-3); padding: var(--space-2) var(--space-3); background: var(--danger-bg, rgba(220,53,69,0.08)); border: 1px solid var(--danger, #dc3545); border-radius: var(--radius-md); font-size: var(--text-sm);">
          {{ overview.client.last_error }}
        </div>

        <div class="settings-grid" style="grid-template-columns: 140px 1fr;">
          <span class="settings-key">中继地址</span>
          <input class="form-input" v-model="cfgRelayUrl" placeholder="ws://vps.example.com:60600" />

          <span class="settings-key">接入门令牌</span>
          <div style="display: flex; gap: var(--space-2); align-items: center;">
            <input class="form-input" :type="showToken ? 'text' : 'password'" v-model="cfgToken" placeholder="与远端 bridge.server.token 同值" autocomplete="new-password" />
            <button class="btn btn-sm" type="button" :title="showToken ? '隐藏令牌' : '显示令牌'" @click="showToken = !showToken">{{ showToken ? '隐藏' : '显示' }}</button>
          </div>

          <span class="settings-key">面板访问密码</span>
          <div style="display: flex; gap: var(--space-2); align-items: center;">
            <input class="form-input" :type="showAccess ? 'text' : 'password'" v-model="cfgAccessToken" placeholder="远程打开本机面板所需（空 = 拒绝远程访问）" autocomplete="new-password" />
            <button class="btn btn-sm" type="button" :title="showAccess ? '隐藏密码' : '显示密码'" @click="showAccess = !showAccess">{{ showAccess ? '隐藏' : '显示' }}</button>
          </div>
        </div>

        <div style="margin-top: var(--space-3); display: flex; justify-content: flex-end;">
          <button class="btn btn-primary" :disabled="savingCfg" @click="saveClientConfig">保存配置</button>
        </div>

        <div v-if="overview.client" style="margin-top: var(--space-3); padding-top: var(--space-3); border-top: 1px solid var(--border); font-size: var(--text-xs); color: var(--text-secondary);">
          节点 ID：<span style="font-family: var(--font-mono);">{{ overview.client.node_id }}</span>
          · 最近更新：{{ formatTime(overview.client.updated_at) }}
          · 配置修改重启后生效
        </div>
        <div v-else class="empty-state" style="padding: var(--space-4);">
          <p style="margin: 0; font-size: var(--text-sm);">
            客户端未启用——填写配置后，在 config.json 设置 bridge.client.enabled = true 并重启。
          </p>
        </div>
      </div>
    </div>

  </div>
</template>
