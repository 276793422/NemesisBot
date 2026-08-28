<script setup lang="ts">
import { ref, computed, onMounted, defineAsyncComponent } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

// G7 (D1)：用户态沙盒 UI 仅在 Linux 工具链构建中打包（build-linux.sh 导出
// VITE_USERLAND_SANDBOX=1）。env 缺省时 import.meta.env.X 被静态替换为
// undefined → 三元常量折叠 → 动态 import 分支被 tree-shake，产物不含该
// chunk（Windows 构建可用 assets grep 验证无 userland 符号，R3）。
const HAS_USERLAND_UI = import.meta.env.VITE_USERLAND_SANDBOX === '1'
const UserlandSandbox = HAS_USERLAND_UI
  ? defineAsyncComponent(() => import('../components/sandbox/UserlandSandbox.vue'))
  : null

const { request } = useWSAPI()
const toast = useToast()

// --- State ---
const loading = ref(true)
const activeTab = ref<'config' | 'status' | 'files'>('config')
// P5 平台自适应总览（platform / executor 四开关 live / backend_probe / ready）
const overview = ref<any>(null)
const status = ref<any>(null)
const env = ref<any>(null)
const pending = ref<any[]>([])
const busy = ref<string | null>(null)
const selected = ref<Set<string>>(new Set())

const platform = computed(() => overview.value?.platform ?? null)
const isWindows = computed(() => platform.value === 'windows')
// 非 Windows（linux/macos/other）一律走用户态布局（other 无后端，布局仍诚实渲染探测为空）
const isUserland = computed(() => platform.value !== null && platform.value !== 'windows')
const executorCfg = computed(() => overview.value?.executor ?? null)
const executorOn = computed(() => !!executorCfg.value?.enabled && !!executorCfg.value?.sandbox)
const strictOn = computed(() => !!executorCfg.value?.strict)
const allowNetworkCfg = computed(() => !!executorCfg.value?.allow_network)
const sandboxReady = computed(() => !!overview.value?.ready)
const backends = computed<any[]>(() => overview.value?.backend_probe?.backends ?? [])
const selectedBackend = computed(() => overview.value?.backend_probe?.selected ?? null)
const strictHint = computed(() => {
  if (isWindows.value) {
    return sandboxReady.value
      ? '当前引擎：✅ Sandboxie 就绪 — 严格模式可兑现'
      : '当前引擎：⚠️ Sandboxie 未就绪 — 严格开启后，要求沙盒的高危工具调用将被拒绝，直到引擎启动且重启 Agent'
  }
  if (selectedBackend.value) {
    return `当前后端：✅ ${selectedBackend.value} 可用 — 严格模式可兑现`
  }
  return '当前后端：⚠️ landlock / bwrap 均不可用 — 严格开启后，要求沙盒的高危工具调用将被拒绝'
})

// Windows-only（Sandboxie 专属语义）computed 保持原样
const ready = computed(() => !!status.value?.ready)
const allowNetwork = computed(() => !!status.value?.allow_network)
const sevenZipOk = computed(() => !!env.value?.seven_zip?.available)
const filesAcquired = computed(() => !!env.value?.sandboxie?.files_acquired)
const driverInstalled = computed(() => !!env.value?.sandboxie?.driver_installed)
const sbiesvcRunning = computed(() => !!env.value?.sandboxie?.sbiesvc_running)

async function refreshAll() {
  loading.value = true
  try {
    // overview 是平台真相源：先拿它，再按平台决定要不要拉 Sandboxie 专属状态
    const ov = await request('sandbox', 'overview').catch(() => null)
    overview.value = ov
    if (ov?.platform === 'windows') {
      const [st, pend] = await Promise.all([
        request('sandbox', 'status').catch(() => null),
        request('sandbox', 'pending').catch(() => []),
      ])
      status.value = st
      pending.value = Array.isArray(pend) ? pend : (pend?.files ?? [])
    }
  } finally {
    loading.value = false
  }
}

async function checkEnv() {
  busy.value = 'check'
  try {
    env.value = await request('sandbox', 'check')
  } catch (e: any) {
    toast.error('环境检查失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

// --- P5：executor 开关逐字段变更（Linux 联动 / 全平台 strict / Linux 联网） ---
async function setExecutorCfg(fields: Record<string, boolean>, successMsg: string) {
  busy.value = 'set_config'
  try {
    const r = await request('sandbox', 'set_config', fields)
    toast.success(r?.restart_hint ? `${successMsg}（${r.restart_hint}）` : successMsg)
    await refreshAll()
  } catch (e: any) {
    toast.error('设置失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

function enableSandboxExec() {
  if (!window.confirm(
    '即将启用沙盒执行（用户态沙盒）：\n' +
    '• executor.enabled=true + executor.sandbox=true（联动）\n' +
    '• ⚠️ executor.enabled 需重启 Agent 生效；sandbox 对后续工具调用实时生效\n\n' +
    '确认启用？'
  )) return
  setExecutorCfg({ enabled: true, sandbox: true }, '沙盒执行已启用')
}

function disableSandboxExec() {
  if (!window.confirm(
    '即将停用沙盒执行：\n' +
    '• executor.enabled=false + executor.sandbox=false（联动）\n' +
    '• ⚠️ 停用后高危工具在 executor 子进程内执行，但不再有沙盒强制\n\n' +
    '确认停用？'
  )) return
  setExecutorCfg({ enabled: false, sandbox: false }, '沙盒执行已停用')
}

function toggleUserlandNetwork() {
  const next = !allowNetworkCfg.value
  setExecutorCfg(
    { allow_network: next },
    `沙盒内联网已${next ? '开启' : '关闭'}`
  )
}

function toggleStrict() {
  const next = !strictOn.value
  // 开启严格模式且当前后端不可用 = 立即开始拒绝高危工具调用——值得一次确认
  if (next && !sandboxReady.value) {
    if (!window.confirm(
      '当前沙盒后端不可用：\n' +
      `${strictHint.value}\n` +
      '开启严格模式后，要求沙盒的工具调用将被【拒绝执行】（而非降级）。\n\n' +
      '确认开启？'
    )) return
  }
  setExecutorCfg({ strict: next }, `严格模式已${next ? '开启（fail-closed）' : '关闭（fail-open）'}`)
}

async function install7z() {
  busy.value = 'install_7z'
  try {
    await request('sandbox', 'install_7z', undefined, 0)
    toast.success('7z 环境就绪')
    await checkEnv()
  } catch (e: any) {
    toast.error('7z 安装失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

async function installSandboxie() {
  busy.value = 'install_sandboxie'
  try {
    await request('sandbox', 'install_sandboxie', undefined, 0)
    toast.success('Sandboxie 文件已下载')
    await checkEnv()
  } catch (e: any) {
    toast.error('下载失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

async function startSandboxie() {
  if (!window.confirm(
    '即将启动 Sandboxie 引擎：\n' +
    '• 会弹出 UAC 提权框（安装内核驱动 + 服务）\n' +
    '• ⚠️ 启动后需要重启 Agent / Gateway 才能完全生效（执行体分离 + 真盒隔离）\n\n' +
    '确认启动？'
  )) return
  busy.value = 'start'
  try {
    await request('sandbox', 'start')
    toast.success('Sandboxie 引擎已启动 · config 已更新 (executor+sandbox=true)。⚠️ 请重启 Agent / Gateway 使其完全生效。')
    await refreshAll()
    await checkEnv()
  } catch (e: any) {
    toast.error('启动失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

async function stopSandboxie() {
  if (!window.confirm(
    '即将停止 Sandboxie 引擎：\n' +
    '• 会弹出 UAC 提权框（卸载内核驱动 + 服务）\n' +
    '• ⚠️ 停止后需要重启 Agent / Gateway 才能完全生效\n\n' +
    '确认停止？'
  )) return
  busy.value = 'stop'
  try {
    await request('sandbox', 'stop')
    toast.success('Sandboxie 引擎已停止 · config 已更新 (executor+sandbox=false)。⚠️ 请重启 Agent / Gateway 使其完全生效。')
    await refreshAll()
    await checkEnv()
  } catch (e: any) {
    toast.error('停止失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

// --- File selection + sync (commit) ---
function toggleFile(path: string) {
  const s = new Set(selected.value)
  if (s.has(path)) s.delete(path)
  else s.add(path)
  selected.value = s
}
function isSel(path: string) { return selected.value.has(path) }
function selectAll() { selected.value = new Set(pending.value.map((p: any) => p.real_path)) }
function selectNone() { selected.value = new Set() }

async function syncSelected() {
  const files = [...selected.value]
  if (files.length === 0) { toast.error('请先勾选要同步的文件'); return }
  busy.value = 'sync'
  try {
    const r = await request('sandbox', 'commit', { files })
    toast.success(`已同步 ${r?.committed ?? 0}/${r?.total ?? files.length} 个文件到主机`)
    await refreshAll()
  } catch (e: any) {
    toast.error('同步失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

async function syncAll() {
  busy.value = 'sync_all'
  try {
    const r = await request('sandbox', 'commit', { all: true })
    toast.success(`已同步全部 ${r?.committed ?? 0} 个文件到主机`)
    await refreshAll()
  } catch (e: any) {
    toast.error('同步失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

async function deleteSelected() {
  const files = [...selected.value]
  if (files.length === 0) { toast.error('请先勾选要从沙箱删除的文件'); return }
  // Deletion is irreversible: the box file is gone and can no longer be synced
  // to the host. Confirm before discarding.
  if (!window.confirm(`确定从沙箱中删除选中的 ${files.length} 个文件吗？\n删除后这些文件将无法再同步到主机（真盘）。`)) return
  busy.value = 'delete'
  try {
    const r = await request('sandbox', 'delete', { files })
    if (r?.errors?.length) {
      toast.error(`部分删除失败：${r.errors.length}/${r?.total ?? files.length}（已删 ${r?.deleted ?? 0}）`)
    } else {
      toast.success(`已从沙箱删除 ${r?.deleted ?? 0}/${r?.total ?? files.length} 个文件`)
    }
    selected.value = new Set()
    await refreshAll()
  } catch (e: any) {
    toast.error('删除失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

function formatSize(n: number): string {
  if (!n) return '0B'
  if (n < 1024) return `${n}B`
  if (n < 1024 * 1024) return `${Math.round(n / 1024)}K`
  return `${(n / 1048576).toFixed(1)}M`
}

async function openBox() {
  try {
    await request('sandbox', 'open_box')
  } catch (e: any) {
    toast.error('打开失败: ' + (e?.message ?? e))
  }
}

// Open an explorer window INSIDE the box (Start.exe /box:NemesisBox explorer.exe).
// Anything launched from it inherits the box via process-tree propagation. cwd is
// %USERPROFILE%. Only enabled when the engine is ready.
async function openExplorer() {
  busy.value = 'open_explorer'
  try {
    await request('sandbox', 'open_explorer')
    toast.success('已在沙盒内打开资源管理器（用户目录）')
  } catch (e: any) {
    toast.error('打开失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

// Toggle the box-level network switch (AllowNetworkAccess). Persists to config,
// rewrites Sandboxie.ini, reloads Sandboxie. Newly started box processes pick it up
// immediately; already-open ones need reopening (Sandboxie WFP caches BlockInternet
// per-process; reload doesn't refresh it).
async function toggleNetwork() {
  const next = !allowNetwork.value
  busy.value = 'set_network'
  try {
    await request('sandbox', 'set_network', { enabled: next })
    toast.success(`盒内联网已${next ? '开启' : '关闭'} · 新启动的程序立即生效，已打开的需重开`)
    await refreshAll()
  } catch (e: any) {
    toast.error('切换失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

onMounted(async () => {
  await refreshAll()
  if (isWindows.value) await checkEnv()
})
</script>

<template>
  <div class="page-sandbox">
    <div class="page-header">
      <h2>沙盒</h2>
      <span v-if="platform" class="platform-badge">{{ platform === 'windows' ? 'Windows · Sandboxie' : platform === 'linux' ? 'Linux · 用户态沙盒' : platform === 'macos' ? 'macOS · Seatbelt' : platform }}</span>
    </div>
    <div class="page-body">

      <!-- 平台信息加载失败（overview 是平台真相源） -->
      <div v-if="!platform" class="card">
        <div class="card-body" style="color: var(--text-secondary); font-size: var(--text-sm);">
          {{ loading ? '加载中…' : '无法获取沙盒平台总览（sandbox.overview）— 请确认网关以完整构建运行（含 sandbox feature）。' }}
          <button class="btn btn-sm" style="margin-left: var(--space-2);" @click="refreshAll" :disabled="!!busy || loading">重试</button>
        </div>
      </div>

      <!-- Busy banner -->
      <div v-if="busy" class="card" style="padding: var(--space-3) var(--space-4); background: var(--accent-bg, rgba(59,130,246,0.08)); border-color: var(--accent); margin-bottom: var(--space-3);">
        <div style="display: flex; align-items: center; gap: var(--space-3);">
          <div class="spinner spinner-sm"></div>
          <span style="font-size: var(--text-sm); color: var(--accent);">
            {{ busy === 'install_7z' ? '正在准备 7z 环境...'
             : busy === 'install_sandboxie' ? '正在下载 Sandboxie 文件（下载 + 解压，无 UAC）...'
             : busy === 'start' ? '正在启动 Sandboxie 引擎（装驱动 + 服务，会弹 UAC）...'
             : busy === 'stop' ? '正在停止 Sandboxie 引擎（卸驱动 + 服务，会弹 UAC）...'
             : busy === 'sync' || busy === 'sync_all' ? '正在同步文件到主机...'
             : busy === 'delete' ? '正在从沙箱删除文件...'
             : busy === 'open_explorer' ? '正在沙盒内打开资源管理器...'
             : busy === 'set_network' ? '正在切换盒内联网状态...'
             : busy === 'set_config' ? '正在更新执行体配置...'
             : '正在检查环境...' }}
          </span>
        </div>
      </div>

      <!-- ════════ Windows 布局：3 Tab Sandboxie 专属语义（保持原样）+ 严格模式卡 ════════ -->
      <template v-if="isWindows">
      <!-- Tabs -->
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'config' }" @click="activeTab = 'config'">沙箱配置</button>
        <button class="tab" :class="{ active: activeTab === 'status' }" @click="activeTab = 'status'">沙箱状态</button>
        <button class="tab" :class="{ active: activeTab === 'files' }" @click="activeTab = 'files'">沙箱文件</button>
        <button class="btn btn-sm" style="margin-left: auto;" @click="refreshAll" :disabled="!!busy">刷新</button>
      </div>

      <!-- ════════ 沙箱配置 ════════ -->
      <div v-if="activeTab === 'config'">
        <div class="card">
          <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
            <h3 style="margin: 0;">环境管理</h3>
            <button class="btn btn-sm" @click="checkEnv" :disabled="!!busy">检查环境</button>
          </div>
          <div class="card-body">

            <!-- 7z environment -->
            <div style="margin-bottom: var(--space-4);">
              <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-2);">
                <span style="font-weight: 500;">7z 环境</span>
                <button class="btn btn-sm" @click="install7z" :disabled="!!busy || sevenZipOk">
                  {{ sevenZipOk ? '已就绪' : '安装' }}
                </button>
              </div>
              <div style="padding-left: var(--space-4); font-size: var(--text-sm); color: var(--text-secondary);">
                <span :style="{ color: sevenZipOk ? 'var(--success)' : 'var(--text-secondary)' }">{{ sevenZipOk ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">{{ sevenZipOk ? `可用（${env?.seven_zip?.source ?? 'system'}）` : '未找到 — 用于解压 Sandboxie 安装包' }}</span>
              </div>
            </div>

            <!-- Sandboxie 文件 (acquire; no UAC) -->
            <div style="margin-bottom: var(--space-4);">
              <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-2);">
                <span style="font-weight: 500;">Sandboxie 文件</span>
                <button class="btn btn-sm" @click="installSandboxie" :disabled="!!busy || !sevenZipOk || filesAcquired">
                  {{ filesAcquired ? '已下载' : '下载' }}
                </button>
              </div>
              <div style="padding-left: var(--space-4); font-size: var(--text-sm); color: var(--text-secondary);">
                <span :style="{ color: filesAcquired ? 'var(--success)' : 'var(--text-secondary)' }">{{ filesAcquired ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">{{ filesAcquired ? '运行时文件已就绪' : '未下载 — 下载并解压 Sandboxie 安装包' }}</span>
              </div>
            </div>

            <!-- Sandboxie 引擎 (activate/deactivate; UAC) — after files acquired -->
            <div v-if="filesAcquired" style="border-top: 1px solid var(--border); padding-top: var(--space-4);">
              <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-2);">
                <span style="font-weight: 500;">Sandboxie 引擎</span>
                <div style="display: flex; gap: var(--space-2);">
                  <button v-if="!sbiesvcRunning" class="btn btn-sm btn-primary" @click="startSandboxie" :disabled="!!busy">启动</button>
                  <button v-else class="btn btn-sm btn-danger" @click="stopSandboxie" :disabled="!!busy">停止</button>
                </div>
              </div>
              <div style="padding-left: var(--space-4); font-size: var(--text-sm); display: flex; flex-direction: column; gap: var(--space-1);">
                <div>
                  <span :style="{ color: driverInstalled ? 'var(--success)' : 'var(--text-secondary)' }">{{ driverInstalled ? '●' : '○' }}</span>
                  <span style="margin-left: var(--space-2);">驱动 + 服务{{ driverInstalled ? '（已安装）' : '（未安装 — 点"启动"激活）' }}</span>
                </div>
                <div>
                  <span :style="{ color: sbiesvcRunning ? 'var(--success)' : 'var(--text-secondary)' }">{{ sbiesvcRunning ? '●' : '○' }}</span>
                  <span style="margin-left: var(--space-2);">SbieSvc{{ sbiesvcRunning ? '（运行中）' : '（未运行）' }}</span>
                </div>
              </div>
            </div>

            <!-- Box network switch (AllowNetworkAccess) — sibling under the engine block. -->
            <div v-if="filesAcquired" style="border-top: 1px solid var(--border); padding-top: var(--space-4); margin-top: var(--space-4);">
              <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-2);">
                <span style="font-weight: 500;">盒内联网</span>
                <button
                  class="btn btn-sm"
                  :class="{ 'btn-primary': allowNetwork }"
                  @click="toggleNetwork"
                  :disabled="!!busy"
                >
                  {{ allowNetwork ? '已开启' : '已关闭' }}
                </button>
              </div>
              <div style="padding-left: var(--space-4); font-size: var(--text-sm); color: var(--text-secondary);">
                <span :style="{ color: allowNetwork ? 'var(--success)' : 'var(--text-secondary)' }">{{ allowNetwork ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">{{ allowNetwork ? '盒内程序允许联网' : '盒内程序禁止联网' }}</span>
              </div>
              <div style="padding-left: var(--space-4); margin-top: var(--space-1); font-size: var(--text-xs); color: var(--text-secondary);">
                切换对新启动的盒内程序立即生效；已打开的需重开。
              </div>
            </div>

            <!-- P5-2 严格模式（fail-closed）— Windows 按钮旁写清 Sandboxie 引擎实际状态 -->
            <div style="border-top: 1px solid var(--border); padding-top: var(--space-4); margin-top: var(--space-4);">
              <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-2);">
                <span style="font-weight: 500;">严格模式（fail-closed）</span>
                <button
                  class="btn btn-sm"
                  :class="{ 'btn-primary': strictOn }"
                  @click="toggleStrict"
                  :disabled="!!busy"
                >
                  {{ strictOn ? '已开启' : '已关闭' }}
                </button>
              </div>
              <div style="padding-left: var(--space-4); font-size: var(--text-sm); color: var(--text-secondary);">
                <span :style="{ color: strictOn ? 'var(--warning, orange)' : 'var(--text-secondary)' }">●</span>
                <span style="margin-left: var(--space-2);">
                  {{ strictOn ? '要求沙盒的调用：Sandboxie 不可用即拒绝执行（不静默降级）' : '要求沙盒的调用：Sandboxie 不可用时降级为无盒执行（warn，现状默认）' }}
                </span>
              </div>
              <div style="padding-left: var(--space-4); margin-top: var(--space-1); font-size: var(--text-xs); color: var(--text-secondary);">
                {{ strictHint }}
              </div>
            </div>

            <div v-if="!sevenZipOk && !filesAcquired" style="margin-top: var(--space-3); font-size: var(--text-xs); color: var(--text-secondary);">
              提示：先准备 7z 环境，再下载 Sandboxie 文件（无 UAC）。文件就绪后点"启动"激活引擎（装驱动，弹 UAC）。然后在 config.json 设 <code>executor.enabled=true, sandbox=true</code> 重启 gateway 即可启用沙盒执行。
            </div>
          </div>
        </div>
      </div>

      <!-- ════════ 沙箱状态 ════════ -->
      <div v-if="activeTab === 'status'">
        <div class="card">
          <div class="card-header"><h3 style="margin: 0;">沙箱状态</h3></div>
          <div class="card-body">
            <div v-if="loading" style="color: var(--text-secondary);">加载中…</div>
            <div v-else style="display: flex; flex-direction: column; gap: var(--space-2); font-size: var(--text-sm);">
              <div>
                <span :style="{ color: status?.sbiesvc === 'Running' ? 'var(--success)' : 'var(--text-secondary)' }">{{ status?.sbiesvc === 'Running' ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">SbieSvc（服务）：{{ status?.sbiesvc ?? '未知' }}</span>
              </div>
              <div>
                <span :style="{ color: status?.sbiedrv === 'Running' ? 'var(--success)' : 'var(--text-secondary)' }">{{ status?.sbiedrv === 'Running' ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">SbieDrv（驱动）：{{ status?.sbiedrv ?? '未知' }}</span>
              </div>
              <div>
                <span :style="{ color: status?.start_exe_present ? 'var(--success)' : 'var(--text-secondary)' }">{{ status?.start_exe_present ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">Start.exe：{{ status?.start_exe_present ? '存在' : '缺失' }}</span>
              </div>
              <div>
                <span :style="{ color: status?.ready ? 'var(--success)' : 'var(--text-secondary)' }">{{ status?.ready ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">沙盒就绪：{{ status?.ready ? '是' : '否' }}</span>
              </div>
              <div style="margin-top: var(--space-3); padding-top: var(--space-3); border-top: 1px solid var(--border); display: flex; justify-content: space-between; align-items: center;">
                <div style="font-size: var(--text-sm); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">
                  <span style="color: var(--text-secondary);">沙箱缓存路径：</span>
                  <code>{{ status?.box_root || '(未知)' }}</code>
                </div>
                <div style="display: flex; gap: var(--space-2);">
                  <button class="btn btn-sm" @click="openBox" :disabled="!status?.box_root">打开沙箱</button>
                  <button class="btn btn-sm btn-primary" @click="openExplorer" :disabled="!!busy || !ready">打开盒内资源管理器</button>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- ════════ 沙箱文件 ════════ -->
      <div v-if="activeTab === 'files'">
        <div class="card">
          <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
            <h3 style="margin: 0;">沙箱内文件（待同步）</h3>
            <div style="display: flex; gap: var(--space-2); align-items: center;">
              <span style="font-size: var(--text-sm); color: var(--text-secondary);">{{ pending.length }} 个 · 已选 {{ selected.size }}</span>
              <button class="btn btn-sm" @click="selectAll" :disabled="!pending.length">全选</button>
              <button class="btn btn-sm" @click="selectNone" :disabled="!selected.size">清空</button>
              <button class="btn btn-sm btn-primary" @click="syncSelected" :disabled="!!busy || !selected.size">同步选中到主机</button>
              <button class="btn btn-sm btn-danger" @click="deleteSelected" :disabled="!!busy || !selected.size">删除选中</button>
              <button class="btn btn-sm" @click="syncAll" :disabled="!!busy || !pending.length">同步全部</button>
            </div>
          </div>
          <div class="card-body">
            <div v-if="pending.length === 0" style="color: var(--text-secondary); font-size: var(--text-sm);">暂无文件。沙箱执行工具写入工作区的文件会列在这里，可勾选后同步到主机（真盘）。</div>
            <div v-else style="display: flex; flex-direction: column; gap: var(--space-1); font-size: var(--text-sm); font-family: var(--font-mono);">
              <label v-for="p in pending" :key="p.real_path" style="display: flex; align-items: center; gap: var(--space-2); padding: var(--space-1) 0; cursor: pointer;">
                <input type="checkbox" :checked="isSel(p.real_path)" @change="toggleFile(p.real_path)" />
                <span style="flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">{{ p.real_path }}</span>
                <span style="color: var(--text-secondary); flex-shrink: 0;">{{ formatSize(p.size) }}</span>
              </label>
            </div>
          </div>
        </div>
      </div>
      </template>

      <!-- ════════ Linux / macOS 布局：用户态沙盒（G7 D1：构建期门控组件） ════════ -->
      <template v-if="isUserland">
        <!-- 构建含用户态 UI（Linux 工具链构建）：完整面板，开关请求逻辑留在本视图 -->
        <UserlandSandbox
          v-if="HAS_USERLAND_UI && UserlandSandbox"
          :busy="busy"
          :executor-on="executorOn"
          :strict-on="strictOn"
          :allow-network="allowNetworkCfg"
          :sandbox-ready="sandboxReady"
          :backends="backends"
          :selected-backend="selectedBackend"
          :strict-hint="strictHint"
          @refresh="refreshAll"
          @enable-exec="enableSandboxExec"
          @disable-exec="disableSandboxExec"
          @toggle-network="toggleUserlandNetwork"
          @toggle-strict="toggleStrict"
        />
        <!-- 构建不含用户态 UI（Windows/Android 工具链构建）：诚实降级提示；
             运行时平台真相源仍是 overview.platform，这里只说明 UI 缺失原因 -->
        <div v-else class="card" data-test="userland-degrade">
          <div class="card-body" style="color: var(--text-secondary); font-size: var(--text-sm);">
            此构建未包含用户态沙盒 UI（构建于 Windows/Android 工具链）。沙盒配置仍可通过
            <code>config.json</code> 的 <code>executor</code> 段管理；如需面板，请用
            <code>scripts/build-linux.sh</code> 重新构建（自动开启用户态沙盒 UI）。
          </div>
        </div>
      </template>

    </div>
  </div>
</template>

<style scoped>
code { background: var(--bg-secondary, rgba(0,0,0,0.05)); padding: 1px 4px; border-radius: 3px; font-size: var(--text-xs); }
.platform-badge {
  font-size: var(--text-xs);
  color: var(--text-secondary);
  border: 1px solid var(--border);
  border-radius: 999px;
  padding: 2px 10px;
  margin-left: var(--space-3);
}
</style>
