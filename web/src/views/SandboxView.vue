<script setup lang="ts">
import { ref, computed, watch, onMounted, defineAsyncComponent } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import SecurityLayerHeader from '../components/security/SecurityLayerHeader.vue'
import MxcPanel from '../components/sandbox/MxcPanel.vue'

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
// 顶层 TAB（2026-11-14 MXC 集成，D8）：Windows = SandboxIE（内含 沙箱配置/
// 沙箱状态/沙箱文件 三个子 TAB）/ MXC；Linux/macOS 无 SandboxIE → 平铺
// MXC（D10，不渲染 TAB 行）。
const activeTopTab = ref<'sandboxie' | 'mxc'>('sandboxie')
const activeTab = ref<'config' | 'status' | 'files'>('config')
// P5 平台自适应总览（platform / executor 四开关 live / backend_probe / ready）
// 2026-09-08 去重：backend_probe 已内联原 status/check 命令的探测数据
// （service 状态 / 7z / box_root），挂载只发 overview + pending——
// `sandbox.status` / `sandbox.check` 保留给手动重查与既有消费方。
const overview = ref<any>(null)
const probe = computed(() => overview.value?.backend_probe ?? null)
const pending = ref<any[]>([])
const busy = ref<string | null>(null)
const selected = ref<Set<string>>(new Set())
// P21（2026-09-25 能力扩展 WS1）：沙盒拒绝台账（sandbox.denials.list，最新在前）
const denials = ref<any[]>([])
const denialsLedger = ref('')

const platform = computed(() => overview.value?.platform ?? null)
const isWindows = computed(() => platform.value === 'windows')
// 非 Windows（linux/macos/other）一律走用户态布局（other 无后端，布局仍诚实渲染探测为空）
const isUserland = computed(() => platform.value !== null && platform.value !== 'windows')
const executorCfg = computed(() => overview.value?.executor ?? null)
const executorOn = computed(() => !!executorCfg.value?.enabled && !!executorCfg.value?.sandbox)
const strictOn = computed(() => !!executorCfg.value?.strict)
const allowNetworkCfg = computed(() => !!executorCfg.value?.allow_network)
const sandboxReady = computed(() => !!overview.value?.ready)

// 十层页面化（2026-11-10）：⑨ 沙盒隔离层共享页头（层号/职责/状态徽标）。
// 状态 = executor 两开关（Layer 1 子进程剥离 / Layer 2 套盒）+ 引擎就绪；
// 平台后缀进职责条（Windows=Sandboxie / Linux=landlock/bwrap / macOS=Seatbelt）。
const platformLabel = computed(() =>
  platform.value === 'windows'
    ? 'Windows · Sandboxie'
    : platform.value === 'linux'
      ? 'Linux · 用户态沙盒（landlock/bwrap）'
      : platform.value === 'macos'
        ? 'macOS · Seatbelt'
        : String(platform.value ?? ''),
)
const layerStatus = computed<{ label: string; tone: 'on' | 'off' | 'na' | 'warn' }>(() => {
  if (!platform.value) return { label: '不可用', tone: 'na' }
  if (!executorOn.value) return { label: '已关闭', tone: 'off' }
  // backend="mxc"：就绪语义 = MXC 探测通过（ProbeOutput 权威）
  if (currentBackend.value === 'mxc') {
    if (!mxcCompiled.value) return { label: '未编译', tone: 'na' }
    return mxcReady.value ? { label: '运行中（MXC 护栏）', tone: 'on' } : { label: 'MXC 未就绪', tone: 'warn' }
  }
  return sandboxReady.value ? { label: '运行中', tone: 'on' } : { label: '未就绪', tone: 'warn' }
})
const backends = computed<any[]>(() => overview.value?.backend_probe?.backends ?? [])
const selectedBackend = computed(() => overview.value?.backend_probe?.selected ?? null)
// D4 状态面（DACL 定向档 2026-09-27）：overview.dacl（Windows + acl feature
// 才非 null；engaged/availability/workspace_sid/root_ace 只读观测——后端
// 状态查询零副作用，不打标）。
const daclCfg = computed<any>(() => overview.value?.dacl ?? null)
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

// Windows-only（Sandboxie 专属语义）computed——全部从 overview.backend_probe 派生
const allowNetwork = computed(() => allowNetworkCfg.value)
const sevenZipOk = computed(() => !!probe.value?.seven_zip?.available)
const filesAcquired = computed(() => !!probe.value?.start_exe_present)
const driverInstalled = computed(() => !!probe.value?.driver_installed)
const sbiesvcRunning = computed(() => !!probe.value?.sbiesvc_running)

async function refreshAll() {
  loading.value = true
  try {
    // overview 是平台真相源 + Windows 探测数据（原 status/check 内容）单一来源
    const ov = await request('sandbox', 'overview').catch(() => null)
    overview.value = ov
    if (ov?.platform === 'windows') {
      const pend = await request('sandbox', 'pending').catch(() => [])
      pending.value = Array.isArray(pend) ? pend : (pend?.files ?? [])
    }
    // P21：拒绝台账独立加载（overview 失败也不阻断——台账是只读附属面）
    await loadDenials()
  } finally {
    loading.value = false
  }
}

// P21：拒绝台账查询（limit 上限 500 由后端钳制；空/异常响应诚实归空列表）
async function loadDenials() {
  const r = await request('sandbox', 'denials.list', { limit: 50 }).catch(() => null)
  denials.value = Array.isArray(r?.denials) ? r.denials : []
  denialsLedger.value = typeof r?.ledger === 'string' ? r.ledger : ''
}

// RFC3339 → 本地时间展示；解析失败原样回显（台账 ts 由后端 chrono 生成，恒可解析）
function formatTs(ts: string): string {
  const d = new Date(ts)
  return isNaN(d.getTime()) ? ts : d.toLocaleString()
}

// --- P5：executor 开关逐字段变更（Linux 联动 / 全平台 strict / Linux 联网） ---
async function setExecutorCfg(fields: Record<string, boolean | number>, successMsg: string) {
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

// --- DACL 定向档三键（配置页批次 5；executor.acl.{dacl, strict, max_files}）---
// ⚠ 与上方「严格模式」（全局 executor.strict）是**两个键**：acl.strict 只在
// ACL 档内部生效（ACL spawn 失败时 fail-closed），不触碰无 ACL 档的降级语义。
// 三键全部 live 读（hook 每次调用时读 config）；dacl 开启后的受限令牌铸造是
// 一次性的（含失败缓存）——铸造失败后关闭再开不会重铸，需重启 Agent。
// Windows + acl feature 装配（overview.dacl 非 null）才显示卡片。
const aclOn = computed(() => !!executorCfg.value?.acl?.dacl)
const aclStrictOn = computed(() => !!executorCfg.value?.acl?.strict)
const aclMaxFiles = computed(() => executorCfg.value?.acl?.max_files ?? 20000)
const aclMaxFilesInput = ref('')
watch(aclMaxFiles, (v: number) => { aclMaxFilesInput.value = String(v) }, { immediate: true })

function toggleAclDacl() {
  const next = !aclOn.value
  if (next && !window.confirm(
    '即将启用 DACL 定向档（半档隔离，实验性）：\n' +
    '• executor.acl.dacl=true——executor 子进程换 write-restricted 受限令牌 + workspace 根 ACL 收窄\n' +
    '• 强制完整性标签 No-Write-Up：对提权位置（如系统目录）拒绝写入\n' +
    '• 是 Sandboxie 全隔离之下的**半档**，与全局严格模式（executor.strict）是两个键\n\n' +
    '确认启用？'
  )) return
  setExecutorCfg({ acl_dacl: next }, `DACL 定向档已${next ? '开启' : '关闭'}`)
}

function toggleAclStrict() {
  const next = !aclStrictOn.value
  setExecutorCfg(
    { acl_strict: next },
    `ACL 档 fail-closed 已${next ? '开启' : '关闭'}${next && !aclOn.value ? '（提示：acl.dacl 未开，本键暂无实际效果）' : ''}`,
  )
}

function saveAclMaxFiles() {
  const n = parseInt(aclMaxFilesInput.value, 10)
  if (Number.isNaN(n) || n < 1 || n > 4294967295) {
    aclMaxFilesInput.value = String(aclMaxFiles.value)
    toast.error('文件数上限须为 1-4294967295 整数')
    return
  }
  setExecutorCfg({ acl_max_files: n }, `ACL 授予树文件数上限已设为 ${n}`)
}

// ════════ MXC 护栏后端（2026-11-14，D5/D6/D8/D11-D13）══════════════════
// 文案单一真相源在后端（overview.mxc.warning 回传），前端不硬编码。
// 状态卡/详情/策略区的交互全部在 MxcPanel.vue 内聚（本视图只保留
// 选择器/警告横幅/探测强刷 所需的最小状态——审查发现死副本已删）。
const mxc = computed<any>(() => overview.value?.mxc ?? null)
const mxcCompiled = computed(() => !!mxc.value?.compiled)
const mxcProbe = computed<any>(() => mxc.value?.probe ?? null)
const mxcReady = computed(() => mxcProbe.value?.status === 'ready')
const mxcWarning = computed(() => mxc.value?.warning ?? '')
const currentBackend = computed(() => executorCfg.value?.backend ?? 'auto')
// 下拉草稿值（「确认后写盘」语义——不是即时生效，D6）
const backendDraft = ref('')
watch(currentBackend, (v: string) => { backendDraft.value = v }, { immediate: true })
const restartBanner = ref(false)

/// 后端选择确认 + 写盘（D5/D6：选 MXC 必须过警告确认框；重启生效——
/// 重启前可反复改，反悔窗口最大化）。切走 MXC 也确认一次（对称诚意）。
function selectBackend() {
  const next = backendDraft.value
  if (next === currentBackend.value) return
  const label = (v: string) =>
    v === 'mxc' ? 'MXC（护栏）' : v === 'sandboxie' ? 'SandboxIE' : v === 'acl' ? 'ACL 轻量档' : '原生（内置）'
  if (next === 'mxc') {
    if (!window.confirm(
      `${mxcWarning.value}\n\n` +
      '• MXC 是护栏（ACL 最小授权）：授权路径内直接写真盘，无 Sandboxie 写虚拟化、无 pending/commit 反悔层\n' +
      '• 引爆场景（明知有问题还要跑、事后审查）请用 SandboxIE\n' +
      '• 需沙箱总闸（executor.sandbox）开启才会生效\n' +
      '• ⚠️ 需重启 Bot 生效；重启前可改回\n\n' +
      `确认从「${label(currentBackend.value)}」切换到「${label(next)}」？`
    )) {
      backendDraft.value = currentBackend.value
      return
    }
  } else {
    if (!window.confirm(
      `即将把沙盒后端从「${label(currentBackend.value)}」切换到「${label(next)}」：\n` +
      '• ⚠️ 需重启 Bot 生效；重启前可改回\n\n确认切换？'
    )) {
      backendDraft.value = currentBackend.value
      return
    }
  }
  busy.value = 'set_backend'
  request('sandbox', 'set_config', { backend: next })
    .then(() => {
      restartBanner.value = true
      toast.success('后端已写入配置 · 重启 Bot 后生效')
      return refreshAll()
    })
    .catch((e: any) => {
      toast.error('切换失败: ' + (e?.message ?? e))
      backendDraft.value = currentBackend.value
    })
    .finally(() => { busy.value = null })
}

/// MXC 强制重探（D13「重新检测」——绕缓存的 fresh probe）。
/// ⚠ 合并而非整体替换：mxc_probe 回执不含 config 段，整体替换会把已保存
/// 的网络策略从 UI 上抹掉（审查发现 2026-11-14——一键静默降级围栏）。
async function probeMxc() {
  busy.value = 'mxc_probe'
  try {
    const r = await request('sandbox', 'mxc_probe')
    if (overview.value) overview.value.mxc = { ...overview.value.mxc, ...r }
  } catch (e: any) {
    toast.error('探测失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

async function install7z() {
  busy.value = 'install_7z'
  try {
    await request('sandbox', 'install_7z', undefined, 0)
    toast.success('7z 环境就绪')
    await refreshAll()
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
    await refreshAll()
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
})
</script>

<template>
  <div class="page-sandbox">
    <SecurityLayerHeader
      num="⑨"
      title="沙盒隔离（沙盒层防护）"
      :duty="`第 9 层最终防御红线（沙盒层防护）：LLM 高危操作剥离进独立子进程（executor.enabled，Layer 1），可再套沙盒（executor.sandbox，Layer 2）——默认原生方案（Windows=Sandboxie 盒：写虚拟化、pending/commit 可反悔；Linux/macOS=内置 landlock/bwrap/Seatbelt），可选 MXC 护栏（ACL 最小授权 + 网络出口管控，无写虚拟化反悔层）。当前平台：${platformLabel}。`"
      :status-label="layerStatus.label"
      :status-tone="layerStatus.tone"
    />
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
             : busy === 'set_backend' ? '正在切换沙盒后端...'
             : busy === 'mxc_probe' ? '正在重新探测 MXC...'
             : '正在处理...' }}
          </span>
        </div>
      </div>

      <!-- ════════ P21：沙盒拒绝台账（executor 沙盒拦截事件，最新在前） ════════ -->
      <div class="card" data-test="denials-card">
        <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
          <h3 style="margin: 0;">拒绝台账 <span style="font-size: var(--text-sm); font-weight: 400; color: var(--text-secondary);">最近 {{ denials.length }} 条</span></h3>
          <button class="btn btn-sm" data-test="denials-refresh" @click="loadDenials" :disabled="loading">刷新台账</button>
        </div>
        <div class="card-body">
          <div v-if="denials.length === 0" style="color: var(--text-secondary); font-size: var(--text-sm);">
            暂无沙盒拒绝记录。executor 沙盒（landlock / bwrap / Sandboxie）拦截工具操作时会记到
            <code>workspace/logs/sandbox_denials.jsonl</code>，并同时回灌给模型引导自纠。
          </div>
          <div v-else style="display: flex; flex-direction: column; font-size: var(--text-sm);">
            <div
              v-for="(d, i) in denials"
              :key="i"
              data-test="denials-row"
              style="display: flex; align-items: baseline; gap: var(--space-2); padding: var(--space-1) 0; border-bottom: 1px solid var(--border);"
            >
              <span style="flex-shrink: 0; color: var(--text-secondary); font-family: var(--font-mono); font-size: var(--text-xs);">{{ formatTs(d.ts) }}</span>
              <span class="denial-backend">{{ d.backend }}</span>
              <code style="flex-shrink: 0;">{{ d.op }}</code>
              <span style="flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); font-size: var(--text-xs);" :title="d.target">{{ d.target }}</span>
              <span style="flex-shrink: 0; max-width: 35%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-secondary); font-size: var(--text-xs);" :title="d.reason">{{ d.reason }}</span>
              <span v-if="!d.model_visible" class="denial-hidden">未回灌</span>
            </div>
            <div v-if="denialsLedger" style="margin-top: var(--space-2); font-size: var(--text-xs); color: var(--text-secondary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">
              台账文件：<code>{{ denialsLedger }}</code>
            </div>
          </div>
        </div>
      </div>

      <!-- ════════ 后端选择器（2026-11-14 D6/D8）：下拉 + 确认 + 重启生效 ════════ -->
      <div class="card" data-test="backend-selector">
        <div class="card-body" style="display: flex; align-items: center; gap: var(--space-3); flex-wrap: wrap;">
          <span style="font-weight: 500;">沙盒后端</span>
          <select class="form-input" style="max-width: 280px;" data-test="backend-select" v-model="backendDraft" @change="selectBackend" :disabled="!!busy">
            <template v-if="isWindows">
              <option value="auto">SandboxIE（默认 · 自动）</option>
              <option value="sandboxie">SandboxIE（显式钉死）</option>
              <option value="acl">ACL 轻量档（实验性半档）</option>
              <option value="mxc">MXC（护栏 · 非沙盒）</option>
            </template>
            <template v-else>
              <option value="auto">内置（默认 · landlock/bwrap/Seatbelt）</option>
              <option value="mxc">MXC（护栏 · 非沙盒）</option>
            </template>
          </select>
          <span style="font-size: var(--text-xs); color: var(--text-secondary);">
            当前生效：<code data-test="backend-current">{{ currentBackend }}</code> · 切换需确认并重启 Bot（重启前可改回）·
            MXC 需沙箱总闸（executor.sandbox）开启才生效
          </span>
        </div>
        <div v-if="restartBanner" class="card-body" style="padding-top: 0; font-size: var(--text-sm); color: var(--warning, #d97706);" data-test="restart-banner">
          ⚠️ 已保存，重启 Bot 后生效。重启前再次修改不会叠加生效（以后保存的为准）。
        </div>
      </div>

      <!-- ════════ Windows 布局：3 Tab Sandboxie 专属语义（保持原样）+ 严格模式卡 ════════ -->
      <template v-if="isWindows">
      <!-- 顶层 TAB（D8）：SandboxIE（含 沙箱配置/沙箱状态/沙箱文件 三个子 TAB）| MXC -->
      <div class="tabs">
        <button class="tab" :class="{ active: activeTopTab === 'sandboxie' }" data-test="tab-sandboxie" @click="activeTopTab = 'sandboxie'">SandboxIE</button>
        <button class="tab" :class="{ active: activeTopTab === 'mxc' }" data-test="tab-mxc" @click="activeTopTab = 'mxc'">MXC</button>
        <button class="btn btn-sm" style="margin-left: auto;" @click="refreshAll" :disabled="!!busy">刷新</button>
      </div>

      <!-- MXC 常驻警告（D5：无论在哪个顶层 TAB 都可见——挂在 TAB 行下；
           浏览 MXC TAB 时无论当前后端都可见——MXC 页面必须有警告，而
           MxcPanel 内横幅在此挂点 hide-warning 防两层同文案叠放。
           mxcWarning 非空才渲染——feature 裁剪构建 overview.mxc 无 warning
           字段（compiled:false），空文案会渲染成无字橙色卡） -->
      <div v-if="mxcWarning && (currentBackend === 'mxc' || activeTopTab === 'mxc')" class="card" style="padding: var(--space-3) var(--space-4); border-color: var(--warning, #d97706); background: rgba(217,119,6,0.06);" data-test="mxc-active-warning">
        <span style="font-size: var(--text-sm); color: var(--warning, #d97706);">⚠️ {{ mxcWarning }}</span>
      </div>

      <template v-if="activeTopTab === 'sandboxie'">
      <!-- 子 TAB（SandboxIE 原三页收层，内容原样） -->
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'config' }" @click="activeTab = 'config'">沙箱配置</button>
        <button class="tab" :class="{ active: activeTab === 'status' }" @click="activeTab = 'status'">沙箱状态</button>
        <button class="tab" :class="{ active: activeTab === 'files' }" @click="activeTab = 'files'">沙箱文件</button>
      </div>

      <!-- ════════ 沙箱配置 ════════ -->
      <div v-if="activeTab === 'config'">
        <div class="card">
          <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
            <h3 style="margin: 0;">环境管理</h3>
            <button class="btn btn-sm" @click="refreshAll" :disabled="!!busy">检查环境</button>
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
                <span style="margin-left: var(--space-2);">{{ sevenZipOk ? `可用（${probe?.seven_zip?.source ?? 'system'}）` : '未找到 — 用于解压 Sandboxie 安装包' }}</span>
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

            <!-- DACL 定向档三键（配置页批次 5；Windows + acl feature 装配才显示）。
                 ⚠ 与上方「严格模式」（全局 executor.strict）是两个键——文案刻意区分。 -->
            <div v-if="isWindows && daclCfg" style="border-top: 1px solid var(--border); padding-top: var(--space-4); margin-top: var(--space-4);" data-test="acl-card">
              <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-2);">
                <span style="font-weight: 500;">DACL 定向档（executor.acl，实验性半档隔离）</span>
                <button
                  class="btn btn-sm"
                  :class="{ 'btn-primary': aclOn }"
                  data-test="acl-dacl-toggle"
                  @click="toggleAclDacl"
                  :disabled="!!busy"
                >
                  {{ aclOn ? '已开启' : '已关闭' }}
                </button>
              </div>
              <div style="padding-left: var(--space-4); font-size: var(--text-sm); color: var(--text-secondary);">
                <span :style="{ color: aclOn ? 'var(--success)' : 'var(--text-secondary)' }">{{ aclOn ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">
                  {{ aclOn ? 'executor 子进程以 write-restricted 受限令牌运行 + workspace 根 ACL 收窄（No-Write-Up）' : '关闭：executor 子进程按现状路径执行' }}
                </span>
              </div>
              <div style="padding-left: var(--space-4); margin-top: var(--space-3); display: flex; align-items: center; gap: var(--space-3); flex-wrap: wrap;">
                <button
                  class="btn btn-sm"
                  :class="{ 'btn-primary': aclStrictOn }"
                  data-test="acl-strict-toggle"
                  @click="toggleAclStrict"
                  :disabled="!!busy"
                >
                  ACL 档 fail-closed：{{ aclStrictOn ? '已开启' : '已关闭' }}
                </button>
                <span style="font-size: var(--text-xs); color: var(--text-secondary);">
                  仅作用于 ACL 档内部（受限令牌铸造失败时拒绝执行而非降级）——与上方全局严格模式是两个键
                </span>
              </div>
              <div style="padding-left: var(--space-4); margin-top: var(--space-3); display: flex; align-items: center; gap: var(--space-3); flex-wrap: wrap;">
                <span style="font-size: var(--text-sm); font-weight: 500;">授予树文件数上限</span>
                <input class="form-input" type="number" min="1" style="max-width: 160px;" data-test="acl-max-files"
                  v-model="aclMaxFilesInput" @change="saveAclMaxFiles" :disabled="!!busy">
                <span style="font-size: var(--text-xs); color: var(--text-secondary);">给 workspace 根补 ACL 授予时最多改写的文件数（缺省 20000；超出截断留痕）</span>
              </div>
              <div style="padding-left: var(--space-4); margin-top: var(--space-2); font-size: var(--text-xs); color: var(--text-secondary);">
                三键均对后续工具调用实时生效（hook 每次调用读配置）；受限令牌铸造是一次性的——铸造失败后关闭再开不会重铸，需重启 Agent。是 Sandboxie 全隔离之下的半档，两者可叠加。
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
                <span :style="{ color: probe?.sbiesvc_state === 'Running' ? 'var(--success)' : 'var(--text-secondary)' }">{{ probe?.sbiesvc_state === 'Running' ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">SbieSvc（服务）：{{ probe?.sbiesvc_state ?? '未知' }}</span>
              </div>
              <div>
                <span :style="{ color: probe?.sbiedrv_state === 'Running' ? 'var(--success)' : 'var(--text-secondary)' }">{{ probe?.sbiedrv_state === 'Running' ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">SbieDrv（驱动）：{{ probe?.sbiedrv_state ?? '未知' }}</span>
              </div>
              <div>
                <span :style="{ color: probe?.start_exe_present ? 'var(--success)' : 'var(--text-secondary)' }">{{ probe?.start_exe_present ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">Start.exe：{{ probe?.start_exe_present ? '存在' : '缺失' }}</span>
              </div>
              <div>
                <span :style="{ color: overview?.ready ? 'var(--success)' : 'var(--text-secondary)' }">{{ overview?.ready ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">沙盒就绪：{{ overview?.ready ? '是' : '否' }}</span>
              </div>
              <!-- D4 状态面：DACL 定向档一行（engaged / workspace SID / 根 ACE 观测 / 缺口明细） -->
              <div v-if="daclCfg">
                <span :style="{ color: daclCfg.engaged ? 'var(--success)' : 'var(--text-secondary)' }">{{ daclCfg.engaged ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">
                  DACL 定向档：{{ daclCfg.engaged ? '已生效（下次 executor spawn 走受限令牌）' : '未生效' }}
                  <template v-if="daclCfg.workspace_sid"> · SID <code>{{ daclCfg.workspace_sid }}</code></template>
                  <template v-if="daclCfg.root_ace"> · 根 ACE grant={{ daclCfg.root_ace.grant ? '✓' : '✗' }} deny_child={{ daclCfg.root_ace.deny_child ? '✓' : '✗' }}</template>
                  <template v-if="daclCfg.availability && daclCfg.availability !== 'full'">
                    · 可用性 {{ daclCfg.availability }}{{ daclCfg.availability_reason ? `（${daclCfg.availability_reason}）` : '' }}
                  </template>
                </span>
              </div>
              <div style="margin-top: var(--space-3); padding-top: var(--space-3); border-top: 1px solid var(--border); display: flex; justify-content: space-between; align-items: center;">
                <div style="font-size: var(--text-sm); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">
                  <span style="color: var(--text-secondary);">沙箱缓存路径：</span>
                  <code>{{ probe?.box_root || '(未知)' }}</code>
                </div>
                <div style="display: flex; gap: var(--space-2);">
                  <button class="btn btn-sm" @click="openBox" :disabled="!probe?.box_root">打开沙箱</button>
                  <button class="btn btn-sm btn-primary" @click="openExplorer" :disabled="!!busy || !sandboxReady">打开盒内资源管理器</button>
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
      <!-- ════════ 顶层 TAB：MXC（D8） ════════ -->
      <template v-if="activeTopTab === 'mxc'">
        <MxcPanel :mxc="mxc" :busy="busy" :current-backend="currentBackend" hide-warning @probe="probeMxc" />
      </template>
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
        <!-- MXC 护栏后端（D10 单后端平铺：无 SandboxIE TAB，MXC 直接平铺在
             用户态面板之后；单管理面不渲染 TAB 行） -->
        <MxcPanel :mxc="mxc" :busy="busy" :current-backend="currentBackend" @probe="probeMxc" />
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
.denial-backend {
  flex-shrink: 0;
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  color: var(--warning, #d97706);
  border: 1px solid currentColor;
  border-radius: 999px;
  padding: 0 8px;
}
.denial-hidden {
  flex-shrink: 0;
  font-size: var(--text-xs);
  color: var(--text-secondary);
  border: 1px dashed var(--border);
  border-radius: 999px;
  padding: 0 8px;
}
</style>
