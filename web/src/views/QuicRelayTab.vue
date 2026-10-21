<script setup lang="ts">
// 中继通道（QUIC，2026-10-21）：通道页第 5 个 tab——iroh QUIC 中继管理面。
//
// 三卡布局（docs/PLAN/2026-10-21_quic-relay-server.md §3.8）：
// ① QUIC 总开关——roaming.enabled（复用 cluster.roaming.enable/disable，
//    与集群【跨网】TAB 双入口同源不漂移）+ 运行态徽标 + 本机 iroh:<node_id>；
// ② 中继服务器网关——relay.enabled / 端口 / external_host / tls 档 / token
//    （真实值永不上前端：status 只给 token_set bool，输入留空 = 不改动，
//    显式「清除」才发空串）+ 对外 relay URL 展示；保存 cluster.relay.config，
//    重启生效（requires_restart 徽标诚实呈现 config 闸与运行态差异）；
// ③ 客户端中继地址——roaming.relay_url / relay_token（relay_url 空 = n0
//    公共中继；relay_token 与自建中继服务端 token 配对）。
//
// 降级：roaming-relay feature 未编译 = relay.status available:false → 整页
// 诚实提示不可操作（命令面恒注册，payload 带原因）。
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface RoamingStatus {
  available: boolean
  enabled: boolean
  active: boolean
  requires_restart: boolean
  key_present: boolean
  node_id: string | null
}
interface RelaySettings {
  enabled: boolean
  http_port: number
  quic_port: number
  external_host: string
  tls: string
  token_set: boolean
  cert_path: string
  cert_state: string
  key_path: string
  key_state: string
  advertised_url: string
}
interface RelayStatus {
  available: boolean
  enabled: boolean
  active: boolean
  requires_restart: boolean
  http_addr: string | null
  quic_addr: string | null
  relay: RelaySettings
  client: { roaming_enabled: boolean; relay_url: string; relay_token_set: boolean }
  reason?: string
}

const roamingStatus = ref<RoamingStatus | null>(null)
const relayStatus = ref<RelayStatus | null>(null)
let pollTimer: ReturnType<typeof setInterval> | null = null

async function loadStatus() {
  try {
    const [r, s] = await Promise.all([
      request('cluster', 'roaming.status'),
      request('cluster', 'relay.status'),
    ])
    roamingStatus.value = (r || null) as RoamingStatus | null
    relayStatus.value = (s || null) as RelayStatus | null
    // 表单只播种一次（首轮成功载荷）——轮询不重置表单，防打断用户编辑；
    // 运行态徽标照常跟随 live 轮询。
    if (!formSeeded.value && relayStatus.value?.available) {
      seedForms()
      formSeeded.value = true
    }
  } catch {
    // 轮询失败静默（本机 web server 短暂重启时别刷错误提示）
  }
}

// ---- 卡① QUIC 总开关 ----
const roamingBadge = computed(() => {
  const s = roamingStatus.value
  if (!s?.available) return { text: '不可用', cls: 'badge-neutral' }
  if (s.active) return { text: '运行中', cls: 'badge-success' }
  if (s.enabled) return { text: '待重启', cls: 'badge-warning' }
  return { text: '未启用', cls: 'badge-neutral' }
})
const togglingRoaming = ref(false)

async function toggleRoaming(on: boolean) {
  togglingRoaming.value = true
  try {
    await request('cluster', on ? 'roaming.enable' : 'roaming.disable')
    toast.success(on ? '跨网漫游已启用（重启网关生效）' : '跨网漫游已关闭（重启网关生效）')
    await loadStatus()
  } catch (e: any) {
    toast.error('开关失败: ' + e)
  } finally {
    togglingRoaming.value = false
  }
}

async function copyText(text: string, label: string) {
  try {
    await navigator.clipboard.writeText(text)
    toast.success(label + '已复制')
  } catch {
    toast.error('复制失败（浏览器不支持剪贴板 API）')
  }
}

// ---- 卡② 中继服务器网关 ----
// 端口以输入框原值承载（type=number 的 v-model 会把非空输入转 number、
// 空值留 ''）——对比时 String() 归一，保存时 Number 化。表单只播种一次：
// 轮询不重置表单（防打断编辑），运行态徽标独立跟随 live 轮询。
const formSeeded = ref(false)
interface ServerForm {
  enabled: boolean
  // 端口以输入框原值承载（type=number 的 v-model 会把非空输入转 number、
  // 空值留 ''）——对比时 String() 归一，保存时 Number 化。
  http_port: number | string
  quic_port: number | string
  external_host: string
  tls: string
  cert_path: string
  key_path: string
  token: string
}
const serverForm = ref<ServerForm>({
  enabled: false,
  http_port: '3340',
  quic_port: '7842',
  external_host: '',
  tls: 'self-signed',
  cert_path: '',
  key_path: '',
  token: '',
})
const serverLoaded = ref<ServerForm>({ ...serverForm.value })
const savingServer = ref(false)

function seedForms() {
  const s = relayStatus.value!
  serverForm.value = {
    enabled: s.relay.enabled,
    http_port: String(s.relay.http_port),
    quic_port: String(s.relay.quic_port),
    external_host: s.relay.external_host,
    tls: s.relay.tls,
    cert_path: s.relay.cert_path,
    key_path: s.relay.key_path,
    token: '',
  }
  serverLoaded.value = { ...serverForm.value }
  clientForm.value = {
    relay_url: s.client.relay_url,
    relay_token: '',
  }
  clientLoaded.value = { ...clientForm.value }
}

function buildServerPatch(): Record<string, unknown> | null {
  const f = serverForm.value
  const l = serverLoaded.value
  const patch: Record<string, unknown> = {}
  if (f.enabled !== l.enabled) patch.enabled = f.enabled
  if (String(f.http_port) !== String(l.http_port)) {
    const n = Number(f.http_port)
    if (!Number.isInteger(n) || n < 1 || n > 65535) {
      throw new Error('HTTP 端口必须是 1-65535 的整数')
    }
    patch.http_port = n
  }
  if (String(f.quic_port) !== String(l.quic_port)) {
    const n = Number(f.quic_port)
    if (!Number.isInteger(n) || n < 1 || n > 65535) {
      throw new Error('QUIC 端口必须是 1-65535 的整数')
    }
    patch.quic_port = n
  }
  if (f.external_host.trim() !== l.external_host) patch.external_host = f.external_host.trim()
  if (f.tls !== l.tls) patch.tls = f.tls
  if (f.tls === 'manual') {
    if (f.cert_path.trim() !== l.cert_path) patch.cert_path = f.cert_path.trim()
    if (f.key_path.trim() !== l.key_path) patch.key_path = f.key_path.trim()
  }
  // token：留空 = 不改（真实值永不上前端，无从对比）；显式清除走清除按钮。
  if (f.token !== '') patch.token = f.token
  return Object.keys(patch).length > 0 ? patch : null
}

async function saveServer() {
  savingServer.value = true
  try {
    const patch = buildServerPatch()
    if (!patch) {
      toast.info('无变更')
      return
    }
    await request('cluster', 'relay.config', patch)
    toast.success('已保存（重启网关后生效）')
    await loadStatus()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  } finally {
    savingServer.value = false
  }
}

async function clearServerToken() {
  savingServer.value = true
  try {
    await request('cluster', 'relay.config', { token: '' })
    toast.success('访问令牌已清除（回落 AllowAll，重启网关后生效）')
    serverForm.value.token = ''
    await loadStatus()
  } catch (e: any) {
    toast.error('清除失败: ' + e)
  } finally {
    savingServer.value = false
  }
}

const serverLiveBadge = computed(() => {
  const s = relayStatus.value
  if (!s?.available) return null
  if (s.active) return { text: '中继运行中', cls: 'badge-success' }
  if (s.enabled) return { text: '待重启', cls: 'badge-warning' }
  return { text: '未启用', cls: 'badge-neutral' }
})

// ---- 卡③ 客户端中继地址 ----
interface ClientForm {
  relay_url: string
  relay_token: string
}
const clientForm = ref<ClientForm>({ relay_url: '', relay_token: '' })
const clientLoaded = ref<ClientForm>({ ...clientForm.value })
const savingClient = ref(false)

function buildClientPatch(): Record<string, unknown> | null {
  const f = clientForm.value
  const l = clientLoaded.value
  const patch: Record<string, unknown> = {}
  if (f.relay_url.trim() !== l.relay_url) patch.relay_url = f.relay_url.trim()
  if (f.relay_token !== '') patch.relay_token = f.relay_token
  return Object.keys(patch).length > 0 ? patch : null
}

async function saveClient() {
  savingClient.value = true
  try {
    const patch = buildClientPatch()
    if (!patch) {
      toast.info('无变更')
      return
    }
    await request('cluster', 'relay.config', patch)
    toast.success('已保存（重启网关后生效）')
    await loadStatus()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  } finally {
    savingClient.value = false
  }
}

async function clearClientToken() {
  savingClient.value = true
  try {
    await request('cluster', 'relay.config', { relay_token: '' })
    toast.success('客户端中继令牌已清除')
    clientForm.value.relay_token = ''
    await loadStatus()
  } catch (e: any) {
    toast.error('清除失败: ' + e)
  } finally {
    savingClient.value = false
  }
}

// ---- 降级 ----
const featureOn = computed(() => relayStatus.value?.available === true)
const degradeReason = computed(
  () => relayStatus.value?.reason || 'QUIC 中继管理面未就绪（命令不可用）',
)

onMounted(async () => {
  await loadStatus()
  pollTimer = setInterval(loadStatus, 5000)
})
onUnmounted(() => {
  if (pollTimer) clearInterval(pollTimer)
})
</script>

<template>
  <div style="display: flex; flex-direction: column; gap: var(--space-4);">

    <!-- ===== 降级面板（roaming-relay feature 未编译） ===== -->
    <div v-if="!featureOn" class="card">
      <div class="card-header"><h3 style="margin: 0;">QUIC 中继（内网穿透）</h3></div>
      <div class="card-body">
        <div class="empty-state" style="padding: var(--space-6); text-align: center;">
          <p style="margin: 0 0 var(--space-2); font-size: var(--text-sm); color: var(--text-secondary);">
            基于 iroh QUIC 的 UDP 打洞 + 自建中继回落，跨网段设备零端口转发互连。
          </p>
          <p style="margin: 0; font-size: var(--text-sm);">
            {{ degradeReason }}
          </p>
          <p style="margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--text-secondary);">
            需以 <span style="font-family: var(--font-mono);">roaming-relay</span> feature 重新编译（构建配置器「QUIC 中继服务器」勾选项）。
          </p>
        </div>
      </div>
    </div>

    <template v-else>
      <!-- ===== 卡① QUIC 总开关 ===== -->
      <div class="card">
        <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
          <h3 style="margin: 0;">QUIC 中继（内网穿透）· 总开关</h3>
          <span class="badge" :class="roamingBadge.cls">{{ roamingBadge.text }}</span>
        </div>
        <div class="card-body">
          <label style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm); cursor: pointer;">
            <input type="checkbox" :checked="roamingStatus?.enabled === true" :disabled="togglingRoaming"
              @change="toggleRoaming(($event.target as HTMLInputElement).checked)" />
            启用跨网漫游（iroh QUIC 叠加层）
          </label>
          <p style="margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--text-secondary);">
            启用后本机节点获得
            <span style="font-family: var(--font-mono);">iroh:&lt;node_id&gt;</span>
            地址，跨网段节点可直连或经中继互通。与【集群 → 跨网】TAB 读同一配置。
          </p>
          <div v-if="roamingStatus?.node_id"
            style="margin-top: var(--space-3); padding-top: var(--space-3); border-top: 1px solid var(--border); display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-xs);">
            <span style="color: var(--text-secondary);">本机节点地址：</span>
            <span style="font-family: var(--font-mono); word-break: break-all;">iroh:{{ roamingStatus.node_id }}</span>
            <button class="btn btn-sm" type="button" @click="copyText('iroh:' + roamingStatus.node_id, '节点地址')">复制</button>
          </div>
        </div>
      </div>

      <!-- ===== 卡② 中继服务器网关 ===== -->
      <div class="card">
        <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
          <h3 style="margin: 0;">中继服务器网关（自建内网穿透服务）</h3>
          <span v-if="serverLiveBadge" class="badge" :class="serverLiveBadge.cls">{{ serverLiveBadge.text }}</span>
        </div>
        <div class="card-body">
          <div class="settings-grid" style="grid-template-columns: 140px 1fr;">
            <span class="settings-key">随网关启动</span>
            <label style="display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm); cursor: pointer;">
              <input type="checkbox" v-model="serverForm.enabled" />
              gateway 进程内启动自建中继
            </label>

            <span class="settings-key">HTTP 端口</span>
            <input class="form-input" type="number" min="1" max="65535" v-model="serverForm.http_port" placeholder="3340" style="max-width: 160px;" />

            <span class="settings-key">QUIC 端口</span>
            <input class="form-input" type="number" min="1" max="65535" v-model="serverForm.quic_port" placeholder="7842" style="max-width: 160px;" />

            <span class="settings-key">对外主机</span>
            <input class="form-input" v-model="serverForm.external_host" placeholder="vps.example.com（空 = 自动探测本机 IP）" spellcheck="false" />

            <span class="settings-key">TLS 档</span>
            <select class="form-input" v-model="serverForm.tls" style="max-width: 240px;">
              <option value="self-signed">self-signed（首启自签证书，推荐）</option>
              <option value="manual">manual（自备证书）</option>
              <option value="none">none（纯 HTTP transport，受限兜底）</option>
            </select>

            <template v-if="serverForm.tls === 'manual'">
              <span class="settings-key">证书路径</span>
              <input class="form-input" v-model="serverForm.cert_path" placeholder="/etc/ssl/certs/relay.pem（pem）" spellcheck="false" />
              <span class="settings-key">私钥路径</span>
              <input class="form-input" v-model="serverForm.key_path" placeholder="/etc/ssl/private/relay-key.pem（pem）" spellcheck="false" />
            </template>

            <span class="settings-key">访问令牌</span>
            <div style="display: flex; gap: var(--space-2); align-items: center;">
              <input class="form-input" type="password" v-model="serverForm.token" autocomplete="new-password"
                :placeholder="relayStatus?.relay.token_set ? '已设置——输入新值可替换（留空不改）' : '客户端接入本中继所需（空 = 无鉴权）'"
                spellcheck="false" style="font-family: var(--font-mono); font-size: var(--text-sm); max-width: 320px;" />
              <button v-if="relayStatus?.relay.token_set" class="btn btn-sm" type="button" :disabled="savingServer"
                title="清除令牌（客户端将无法凭令牌接入——回落无鉴权）" @click="clearServerToken">清除</button>
            </div>
          </div>

          <div style="margin-top: var(--space-3); display: flex; justify-content: flex-end;">
            <button class="btn btn-primary" :disabled="savingServer" @click="saveServer">保存配置</button>
          </div>

          <div style="margin-top: var(--space-3); padding-top: var(--space-3); border-top: 1px solid var(--border); font-size: var(--text-xs); color: var(--text-secondary);">
            <div>对外 relay URL（客户端「中继地址」填这个）：
              <span style="font-family: var(--font-mono);">{{ relayStatus?.relay.advertised_url || '-' }}</span>
              <button v-if="relayStatus?.relay.advertised_url" class="btn btn-sm" type="button"
                @click="copyText(relayStatus!.relay.advertised_url, 'relay URL')">复制</button>
            </div>
            <div v-if="relayStatus?.active" style="margin-top: var(--space-1);">
              监听：HTTP <span style="font-family: var(--font-mono);">{{ relayStatus.http_addr || '-' }}</span>
              · QUIC <span style="font-family: var(--font-mono);">{{ relayStatus.quic_addr || '-' }}</span>
            </div>
            <div style="margin-top: var(--space-1);">
              保存写入 config.cluster.json（roaming.relay 段），重启网关后生效；也可用独立命令
              <span style="font-family: var(--font-mono);">nemesisbot quic relay</span> 单独跑中继进程。
            </div>
          </div>
        </div>
      </div>

      <!-- ===== 卡③ 客户端中继地址 ===== -->
      <div class="card">
        <div class="card-header">
          <h3 style="margin: 0;">客户端中继地址（本机出连中继）</h3>
        </div>
        <div class="card-body">
          <div class="settings-grid" style="grid-template-columns: 140px 1fr;">
            <span class="settings-key">中继地址</span>
            <input class="form-input" v-model="clientForm.relay_url" spellcheck="false"
              placeholder="http://vps.example.com:3340（空 = n0 公共中继）" />

            <span class="settings-key">中继令牌</span>
            <div style="display: flex; gap: var(--space-2); align-items: center;">
              <input class="form-input" type="password" v-model="clientForm.relay_token" autocomplete="new-password"
                :placeholder="relayStatus?.client.relay_token_set ? '已设置——输入新值可替换（留空不改）' : '与自建中继服务端访问令牌配对（n0 公共中继不需要）'"
                spellcheck="false" style="font-family: var(--font-mono); font-size: var(--text-sm); max-width: 320px;" />
              <button v-if="relayStatus?.client.relay_token_set" class="btn btn-sm" type="button" :disabled="savingClient"
                title="清除客户端中继令牌" @click="clearClientToken">清除</button>
            </div>
          </div>

          <div style="margin-top: var(--space-3); display: flex; justify-content: flex-end;">
            <button class="btn btn-primary" :disabled="savingClient" @click="saveClient">保存配置</button>
          </div>

          <div style="margin-top: var(--space-3); padding-top: var(--space-3); border-top: 1px solid var(--border); font-size: var(--text-xs); color: var(--text-secondary);">
            中继地址留空时走 n0 公共中继（默认）；填自建中继 URL 后，本机叠加层经该中继与对端打洞互通。
            连通状态见【集群 → 拓扑/节点】——节点经中继握手后呈现在线。
          </div>
        </div>
      </div>
    </template>

  </div>
</template>
