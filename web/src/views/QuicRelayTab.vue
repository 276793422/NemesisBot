<script setup lang="ts">
// QUIC 中继页（2026-10-23 与集群解耦改版）：通道页 tab——iroh QUIC 中继管理面。
//
// 两卡 + 名册表布局：
// ① 中继服务器网关——头部「中继服务」滑动开关 = quic.relay.enable/disable
//    实时开关（config 闸 + 进程内拉起/停掉一体，免重启网关）；端口 /
//    external_host / tls 档 / token（真实值永不上前端：status 只给 token_set
//    bool，输入留空 = 不改动，显式「清除」才发空串）+ 对外 relay URL 展示；
//    参数走 quic.relay.config 保存——中继在启动时读 config，重新开关即以
//    新配置生效；requires_restart 徽标 = config 闸与运行态差异（手改配置
//    文件/拉起失败的诚实呈现）；
// ② 客户端中继地址——client.relay_url / relay_token（relay_url 空 = n0
//    公共中继；relay_token 与自建中继服务端 token 配对）；
// ③ 中结名册——服务端全量名册（quic.registry.members，中继机视角）：接入
//    即报到、周期心跳的设备全在这里。跨网开关（集群是否消费中继组网）在
//    【集群 → 配置】跨网漫游卡——集群是 QUIC 的消费方，不是宿主。
//
// 降级：quic.relay.status available:false → 整页诚实提示不可操作
//（命令面恒注册，payload 带原因；M4 起 feature 已删恒编译——该分支仅防御性保留）。
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

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

const relayStatus = ref<RelayStatus | null>(null)
let pollTimer: ReturnType<typeof setInterval> | null = null

async function loadStatus() {
  try {
    const s = await request('quic', 'relay.status')
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

async function copyText(text: string, label: string) {
  try {
    await navigator.clipboard.writeText(text)
    toast.success(label + '已复制')
  } catch {
    toast.error('复制失败（浏览器不支持剪贴板 API）')
  }
}

// ---- 卡① 中继服务器网关 ----
// 端口以输入框原值承载（type=number 的 v-model 会把非空输入转 number、
// 空值留 ''）——对比时 String() 归一，保存时 Number 化。表单只播种一次：
// 轮询不重置表单（防打断编辑），运行态徽标与「中继服务」开关独立跟随 live
// 轮询。开关是实时面（relay.enable/disable），表单是参数面（relay.config）。
const formSeeded = ref(false)
interface ServerForm {
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
    await request('quic', 'relay.config', patch)
    toast.success('已保存——中继在启动时读配置，重新开关「中继服务」即以新配置生效')
    await loadStatus()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  } finally {
    savingServer.value = false
  }
}

// ---- 卡①头部「中继服务」实时开关 ----
// quic.relay.enable/disable：config 闸落盘 + 进程内拉起/停掉一体，免重启网关。
// 失败也回读真实状态——开关 UI 以服务端回执为准（复选框自动回弹）。
const togglingRelay = ref(false)

async function toggleRelay(on: boolean) {
  togglingRelay.value = true
  try {
    await request('quic', on ? 'relay.enable' : 'relay.disable')
    toast.success(on ? '中继已启动（实时生效）' : '中继已停止（实时生效）')
  } catch (e: any) {
    toast.error('开关失败: ' + e)
  } finally {
    togglingRelay.value = false
    await loadStatus()
  }
}

async function clearServerToken() {
  savingServer.value = true
  try {
    await request('quic', 'relay.config', { token: '' })
    toast.success('访问令牌已清除——下次开启「中继服务」开关将拒绝启动（无鉴权跳板不允许，fail-closed）')
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
  // 实时开关接线后 enabled≠active 只剩两种来路：手改配置文件未跟上 / 开关
  // 拉起失败（token 空 fail-closed 等）——不是「待重启」，是「待启动」。
  if (s.enabled) return { text: '待启动', cls: 'badge-warning' }
  return { text: '未启用', cls: 'badge-neutral' }
})
// 卡①提示行：中继令牌缺失（fail-closed 拒绝启动）——「待启动」的修复动作
// = 设置令牌后开一次「中继服务」开关（免重启）。
const relayHint = computed(() => {
  const s = relayStatus.value
  if (!s?.available || s.active || !s.enabled) return ''
  if (!s.relay.token_set) {
    return '中继已启用但未设访问令牌——开启「中继服务」开关时将拒绝启动（无鉴权跳板不允许，fail-closed）。请设置「访问令牌」后再开开关。'
  }
  return ''
})

// ---- 卡② 客户端中继地址 ----
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
    await request('quic', 'relay.config', patch)
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
    await request('quic', 'relay.config', { relay_token: '' })
    toast.success('客户端中继令牌已清除')
    clientForm.value.relay_token = ''
    await loadStatus()
  } catch (e: any) {
    toast.error('清除失败: ' + e)
  } finally {
    savingClient.value = false
  }
}

// ---- 卡③ 中结名册（服务端全量名册；中继机视角） ----
interface RegistryMember {
  node_id: string
  name: string
  role: string
  iroh_id: string
  build: string
  features: string[]
  last_seen_ms: number
}
const registry = ref<{ count: number; members: RegistryMember[] } | null>(null)

async function loadRegistry() {
  try {
    const r = await request('quic', 'registry.members')
    if (r) registry.value = { count: r.count ?? 0, members: r.members ?? [] }
  } catch {
    // 管理面未就绪 / 未装配 = 保持上次值（降级面板已覆盖该形态，防御性保留）
  }
}

function shortIroh(hex: string): string {
  return hex.length > 12 ? hex.slice(0, 12) + '…' : hex
}

function fmtSeen(ms: number): string {
  if (!ms) return '--'
  const d = new Date(ms)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

// ---- 降级 ----
const featureOn = computed(() => relayStatus.value?.available === true)
const degradeReason = computed(
  () => relayStatus.value?.reason || 'QUIC 中继管理面未就绪（命令不可用）',
)

onMounted(async () => {
  await loadStatus()
  await loadRegistry()
  pollTimer = setInterval(() => {
    loadStatus()
    loadRegistry()
  }, 5000)
})
onUnmounted(() => {
  if (pollTimer) clearInterval(pollTimer)
})
</script>

<template>
  <div style="display: flex; flex-direction: column; gap: var(--space-4);">

    <!-- ===== 降级面板（管理面未就绪；M4 起恒编译，防御性保留） ===== -->
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
            请确认网关为完整构建（QUIC 管理面恒编译，降级面板仅防御性保留）。
          </p>
        </div>
      </div>
    </div>

    <template v-else>
      <!-- ===== 卡① 中继服务器网关 ===== -->
      <div class="card">
        <div class="card-header" style="display: flex; justify-content: space-between; align-items: center; gap: var(--space-2);">
          <h3 style="margin: 0;">中继服务器网关（自建内网穿透服务）</h3>
          <div style="display: flex; align-items: center; gap: var(--space-2); flex-shrink: 0;">
            <span v-if="serverLiveBadge" class="badge" :class="serverLiveBadge.cls">{{ serverLiveBadge.text }}</span>
            <label class="toggle-switch"
              :title="relayStatus?.relay.enabled ? '关闭即实时停止中继（免重启网关）' : '开启即实时拉起中继（免重启网关）'">
              <input type="checkbox" :checked="relayStatus?.relay.enabled === true"
                :disabled="togglingRelay"
                @change="toggleRelay(($event.target as HTMLInputElement).checked)" />
              <span class="toggle-slider"></span>
              <span class="toggle-label">中继服务</span>
            </label>
          </div>
        </div>
        <div class="card-body">
          <p v-if="relayHint"
            style="margin: 0 0 var(--space-3); padding: var(--space-2) var(--space-3); background: var(--warning-bg, rgba(255, 193, 7, 0.12)); border: 1px solid var(--warning-border, rgba(255, 193, 7, 0.45)); border-radius: var(--radius-md); font-size: var(--text-xs); color: var(--text);">
            ⚠ {{ relayHint }}
          </p>
          <div class="settings-grid" style="grid-template-columns: 140px 1fr;">
            <span class="settings-key">HTTP 端口</span>
            <div>
              <input class="form-input" type="number" min="1" max="65535" v-model="serverForm.http_port" placeholder="3340" style="max-width: 160px;" />
              <div class="port-hint">中继 HTTP 通道（客户端「中继地址」指向它；防火墙放行 <span style="font-family: var(--font-mono);">TCP</span>）</div>
            </div>

            <span class="settings-key">QUIC 端口</span>
            <div>
              <input class="form-input" type="number" min="1" max="65535" v-model="serverForm.quic_port" placeholder="7842" style="max-width: 160px;" />
              <div class="port-hint">QUIC 传输通道（打洞数据面；防火墙放行 <span style="font-family: var(--font-mono);">UDP</span>）</div>
            </div>

            <span class="settings-key">成员名册</span>
            <div class="port-hint" style="align-self: center;">
              自动发现面（rendezvous）随中继自动拉起，默认 <span style="font-family: var(--font-mono);">3341/TCP</span>——设备接入即报到、
              周期拉名册，同令牌设备自动互见，无需手工写 peers.toml（防火墙放行 <span style="font-family: var(--font-mono);">TCP 3341</span>）。
            </div>

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
                :placeholder="relayStatus?.relay.token_set ? '已设置——输入新值可替换（留空不改）' : '客户端接入本中继所需（未设 = 中继拒绝启动，fail-closed）'"
                spellcheck="false" style="font-family: var(--font-mono); font-size: var(--text-sm); max-width: 320px;" />
              <button v-if="relayStatus?.relay.token_set" class="btn btn-sm" type="button" :disabled="savingServer"
                title="清除令牌（清除后中继将拒绝启动——无鉴权裸奔不允许）" @click="clearServerToken">清除</button>
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
              保存写入 config.quic.json（relay 段）。「中继服务」开关实时生效（免重启网关）；
              端口/地址等参数在启动时读取——重新开关即以新配置生效。也可用独立命令
              <span style="font-family: var(--font-mono);">nemesisbot quic relay</span> 单独跑中继进程。
            </div>
          </div>
        </div>
      </div>

      <!-- ===== 卡② 客户端中继地址 ===== -->
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
            中继地址留空时走 n0 公共中继（默认）；填自建中继 URL 后，本机叠加层经该中继与对端打洞互通，
            并自动接入该中继的成员名册（30s 周期同步，同令牌设备互相可见）。
            连通状态见【集群 → 拓扑/节点】——节点经中继握手后呈现在线。
            跨网漫游开关（是否以 QUIC 组网）在【集群 → 配置】跨网漫游卡。
          </div>
        </div>
      </div>

      <!-- ===== 卡③ 中结名册（服务端全量名册；中继机视角） ===== -->
      <div class="card">
        <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
          <h3 style="margin: 0;">中结名册（接入设备）</h3>
          <span class="badge badge-neutral" style="font-size: var(--text-xs);">{{ registry?.count ?? 0 }} 个成员</span>
        </div>
        <div class="card-body">
          <div class="port-hint" style="margin-bottom: var(--space-2);">
            服务端全量名册（token 命名空间内全部报到成员，含本机）。设备接入即报到、30s 周期续期；
            同令牌设备经名册自动互见（集群视图按 features 过滤，见【集群 → 配置】名册同步）。
          </div>
          <div v-if="!registry || !registry.members.length" class="empty-state" style="padding: var(--space-4);">
            <p style="margin: 0; font-size: var(--text-sm); color: var(--text-secondary);">
              暂无成员——中继未运行，或尚无设备接入（接入后 30s 内出现在此）。
            </p>
          </div>
          <div v-else class="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>节点 ID</th>
                  <th>名称</th>
                  <th>角色</th>
                  <th>iroh 身份</th>
                  <th>构建</th>
                  <th>features</th>
                  <th>最近报到</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="m in registry.members" :key="m.node_id + m.iroh_id">
                  <td style="font-family: var(--font-mono); font-size: var(--text-xs);">{{ m.node_id }}</td>
                  <td>{{ m.name || '--' }}</td>
                  <td>{{ m.role || '--' }}</td>
                  <td style="font-family: var(--font-mono); font-size: var(--text-xs);"
                    :title="'iroh:' + m.iroh_id">
                    <span style="display: inline-flex; align-items: center; gap: var(--space-1);">
                      {{ shortIroh(m.iroh_id) }}
                      <button class="btn btn-sm" type="button" style="padding: 0 6px;"
                        title="复制完整 iroh 地址" @click="copyText('iroh:' + m.iroh_id, 'iroh 地址')">⧉</button>
                    </span>
                  </td>
                  <td style="font-family: var(--font-mono); font-size: var(--text-xs);">{{ m.build || '--' }}</td>
                  <td>{{ (m.features || []).join(', ') || '--' }}</td>
                  <td style="font-family: var(--font-mono); font-size: var(--text-xs);">{{ fmtSeen(m.last_seen_ms) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>
      </div>
    </template>

  </div>
</template>

<style scoped>
/* 滑动开关（与 VoiceTab/CodingView 等同款） */
.toggle-switch {
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  cursor: pointer;
  position: relative;
}
.toggle-switch input {
  position: absolute;
  opacity: 0;
  width: 0;
  height: 0;
}
.toggle-slider {
  width: 36px;
  height: 20px;
  background: var(--border, #d1d5db);
  border-radius: 10px;
  position: relative;
  transition: background 0.2s;
  flex-shrink: 0;
}
.toggle-slider::after {
  content: '';
  position: absolute;
  width: 16px;
  height: 16px;
  background: white;
  border-radius: 50%;
  top: 2px;
  left: 2px;
  transition: transform 0.2s;
  box-shadow: 0 1px 3px rgba(0,0,0,0.15);
}
.toggle-switch input:checked + .toggle-slider {
  background: var(--accent, #3b82f6);
}
.toggle-switch input:checked + .toggle-slider::after {
  transform: translateX(16px);
}
.toggle-label {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  user-select: none;
}
.toggle-switch input:disabled + .toggle-slider {
  opacity: 0.4;
  cursor: not-allowed;
}
.toggle-switch input:disabled ~ .toggle-label {
  opacity: 0.5;
}

/* 端口/名册说明行 */
.port-hint {
  margin-top: var(--space-1);
  font-size: var(--text-xs);
  color: var(--text-secondary);
  line-height: 1.5;
}
</style>
