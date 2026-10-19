<script setup lang="ts">
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

// P5 第 4 步 ②：桌面级 VM 管理页。三页签（配置/镜像/实例）+
// 探针横幅 + 档位建议。铁律：总开关默认关——建议是建议不是决定，
// 页面绝不自动开启；关闭态 = 零 VM 路径激活。
const { request } = useWSAPI()
const toast = useToast()

// --- State ---
const loading = ref(true)
const activeTab = ref<'config' | 'images' | 'instances'>('config')
const overview = ref<any>(null)
const image = ref<any>(null)
const busy = ref<string | null>(null)

const probe = computed(() => overview.value?.probe ?? null)
const enabled = computed(() => !!overview.value?.enabled)
const cfg = computed(() => overview.value?.config ?? null)
const suggestion = computed(() => overview.value?.suggestion ?? null)
const sleepLine = computed(() => overview.value?.sleep_line ?? null)
const sleepNote = computed<string>(() => overview.value?.sleep_line_note ?? '')
const gc = computed(() => overview.value?.gc ?? null)
const instances = computed<any[]>(() =>
  Array.isArray(overview.value?.instances) ? overview.value.instances : [],
)
const buildRunning = computed(() => !!image.value?.build?.running)

// --- 表单编辑缓冲（单一写路径：saveConfig 一次性写整段 executor.vm） ---
interface VmForm {
  enabled: boolean
  arch: string
  hypervisor: string
  desktop: string
  image_source: string
  image_sha256: string
  vcpu: number
  ram_mb: number
  disk_gb: number
  max_instances: number
  idle_sleep_secs: number
  gc_max_total_disk_gb: number
  gc_over_limit: string
}

// 后端缺省值镜像（ExecutorVmConfig serde default；config 段缺席时表单初始值）。
function defaultForm(): VmForm {
  return {
    enabled: false,
    arch: 'x86_64',
    hypervisor: 'auto',
    desktop: 'minimal',
    image_source: 'https://cloud.debian.org/images/cloud/bookworm/latest/debian-12-genericcloud-amd64.qcow2',
    image_sha256: '',
    vcpu: 2,
    ram_mb: 2048,
    disk_gb: 20,
    max_instances: 2,
    idle_sleep_secs: 300,
    gc_max_total_disk_gb: 50,
    gc_over_limit: 'warn',
  }
}

const form = ref<VmForm>(defaultForm())
const formDirty = ref(false)

function loadForm() {
  const c = cfg.value
  if (!c) {
    form.value = defaultForm()
    return
  }
  form.value = {
    enabled: !!c.enabled,
    arch: c.arch ?? 'x86_64',
    hypervisor: c.hypervisor ?? 'auto',
    desktop: c.desktop ?? 'minimal',
    image_source: c.image?.source ?? defaultForm().image_source,
    image_sha256: c.image?.sha256 ?? '',
    vcpu: c.resources?.vcpu ?? 2,
    ram_mb: c.resources?.ram_mb ?? 2048,
    disk_gb: c.resources?.disk_gb ?? 20,
    max_instances: c.max_instances ?? 2,
    idle_sleep_secs: c.idle_sleep_secs ?? 300,
    gc_max_total_disk_gb: c.gc?.max_total_disk_gb ?? 50,
    gc_over_limit: c.gc?.over_limit ?? 'warn',
  }
  formDirty.value = false
}

// --- 镜像来源预设（可看可选可配可改：预设下拉 + 自由编辑 + 校验和） ---
const IMAGE_PRESETS = [
  {
    label: 'Debian 12 genericcloud x86_64（默认）',
    url: 'https://cloud.debian.org/images/cloud/bookworm/latest/debian-12-genericcloud-amd64.qcow2',
  },
  {
    label: 'Debian 13 genericcloud x86_64',
    url: 'https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2',
  },
  {
    label: 'Debian 12 genericcloud aarch64（接口预留档）',
    url: 'https://cloud.debian.org/images/cloud/bookworm/latest/debian-12-genericcloud-arm64.qcow2',
  },
]
const presetIdx = computed(() => IMAGE_PRESETS.findIndex((p) => p.url === form.value.image_source))

function onPresetChange(e: Event) {
  const v = Number((e.target as HTMLSelectElement).value)
  if (!Number.isNaN(v)) applyPreset(v)
}

function applyPreset(i: number) {
  if (i < 0) return
  form.value.image_source = IMAGE_PRESETS[i].url
  formDirty.value = true
}

function gcDecisionLabel(d: string): string {
  return d === 'allow' ? '正常' : d === 'warn' ? '接近预算' : '超限'
}

// --- Data loading ---
async function refreshAll() {
  loading.value = true
  try {
    const ov = await request('vm', 'overview').catch(() => null)
    overview.value = ov
    if (ov) await refreshBootstrap()
    await refreshImages()
    await refreshTry()
    loadForm()
  } finally {
    loading.value = false
  }
}

async function refreshImages() {
  const img = await request('vm', 'image.list').catch(() => null)
  image.value = img
}

// 构建进行中轮询日志尾部（2s；页面离开/构建结束自动停）。
let pollTimer: ReturnType<typeof setInterval> | null = null
function ensurePolling() {
  if (pollTimer) return
  pollTimer = setInterval(async () => {
    if (!buildRunning.value) {
      stopPolling()
      // 构建刚结束：刷一次总览（镜像尺寸/GC 数据变了）。
      await refreshAll().catch(() => null)
      return
    }
    await refreshImages().catch(() => null)
  }, 2000)
}
function stopPolling() {
  if (pollTimer) {
    clearInterval(pollTimer)
    pollTimer = null
  }
}
onBeforeUnmount(stopPolling)

// --- B4 环境安装（对齐沙盒页「环境管理」分步卡） ---
// bootstrap.status 只读无闸（查询不是安装动作）；install_qemu/enable_accel
// enabled-gated（关闭态后端诚实拒绝——前端预闸只为少一次徒劳请求，后端仍兜底）。
const bootstrap = ref<any>(null)
const bsInstallRunning = computed(() => !!bootstrap.value?.install?.running)
const qemuState = computed(() => bootstrap.value?.qemu?.state ?? null)
const accelState = computed(() => bootstrap.value?.accel?.state ?? null)
const imageReady = computed(() => !!bootstrap.value?.image_ready)
// B7 构建路由（bootstrap.image_route）：「能不能点」与「缺什么怎么补」
// 同一份后端事实。旧后端无此段时闸交后端（启动时诚实拒绝），不卡按钮。
const imageRoute = computed(() => (bootstrap.value?.image_route ?? null) as { state?: string; missing?: string[]; command?: string; note?: string; guest_ready?: boolean } | null)
const imageRouteReady = computed(() => {
  const s = imageRoute.value?.state
  if (!s) return true
  return s === 'native' || s === 'wsl_ready'
})
const imageRouteIsWsl = computed(() => imageRoute.value?.state === 'wsl_ready')
const guestReady = computed(() => imageRoute.value?.guest_ready !== false)
const guestBuildCmd = computed(() => {
  const target = form.value.arch === 'aarch64' ? 'aarch64-unknown-linux-musl' : 'x86_64-unknown-linux-musl'
  return `bash scripts/build-guest.sh "${target}"`
})

async function refreshBootstrap() {
  bootstrap.value = await request('vm', 'bootstrap.status').catch(() => null)
}

// 安装轮询复检（对齐沙盒页「安装完成自动转绿」交互）：
// install.running 中无界轮询；enable_accel 后有界复检（30 跳 × 2s ≈ 60s，
// 覆盖 UAC/DISM 完成窗口）；running 翻 false 或有界窗耗尽 → 全量刷新收尾。
let bsPollTimer: ReturnType<typeof setInterval> | null = null
let bsPollTicksLeft = 0
function ensureBootstrapPolling(boundedTicks = 0) {
  if (boundedTicks > bsPollTicksLeft) bsPollTicksLeft = boundedTicks
  if (bsPollTimer) return
  bsPollTimer = setInterval(async () => {
    await refreshBootstrap()
    if (bsInstallRunning.value) {
      bsPollTicksLeft = 0 // 转无界：装完的收尾由 running→false 分支处理
      return
    }
    bsPollTicksLeft -= 1
    if (bsPollTicksLeft <= 0) {
      stopBootstrapPolling()
      // 收尾全量刷新：QEMU/镜像/探针态统一转绿（对齐沙盒页）
      await refreshAll().catch(() => null)
    }
  }, 2000)
}
function stopBootstrapPolling() {
  if (bsPollTimer) {
    clearInterval(bsPollTimer)
    bsPollTimer = null
  }
  bsPollTicksLeft = 0
}
onBeforeUnmount(stopBootstrapPolling)

// --- 环境安装动作 ---
async function installQemu() {
  busy.value = 'install_qemu'
  try {
    const r = await request('vm', 'bootstrap.install_qemu', {})
    toast.success(r?.note ?? 'QEMU 安装任务已启动')
    await refreshBootstrap()
    ensureBootstrapPolling()
  } catch (e: any) {
    toast.error('安装启动失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

async function enableAccel() {
  if (!window.confirm(
    '即将启用硬件加速（Windows WHPX）：\n' +
    '• 会弹出 UAC 提权框（DISM 启用虚拟机监控程序平台，不自动重启）\n' +
    '• 功能挂起重启时需重启 Windows 后复检\n\n' +
    '确认启用？'
  )) return
  busy.value = 'enable_accel'
  try {
    const r = await request('vm', 'bootstrap.enable_accel', {})
    toast.success(r?.note ?? '已发起——稍后自动复检')
    ensureBootstrapPolling(30)
  } catch (e: any) {
    toast.error('启用失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

async function copyCommand(cmd: string) {
  try {
    await navigator.clipboard.writeText(cmd)
    toast.success('命令已复制到剪贴板')
  } catch {
    toast.error('复制失败——请手动选择命令文本复制')
  }
}

// --- Config save（单一写路径） ---
function formPayload() {
  return {
    enabled: form.value.enabled,
    arch: form.value.arch,
    hypervisor: form.value.hypervisor,
    desktop: form.value.desktop,
    image: { source: form.value.image_source, sha256: form.value.image_sha256 },
    resources: { vcpu: form.value.vcpu, ram_mb: form.value.ram_mb, disk_gb: form.value.disk_gb },
    max_instances: form.value.max_instances,
    idle_sleep_secs: form.value.idle_sleep_secs,
    gc: { max_total_disk_gb: form.value.gc_max_total_disk_gb, over_limit: form.value.gc_over_limit },
  }
}

async function saveConfig() {
  busy.value = 'save'
  try {
    await request('vm', 'config.set', { vm: formPayload() })
    toast.success('VM 配置已保存')
    await refreshAll()
  } catch (e: any) {
    toast.error('保存失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

function toggleEnabled() {
  const next = !form.value.enabled
  if (next) {
    if (!window.confirm(
      '即将开启 VM 档（executor.vm.enabled=true）：\n' +
      '• 后续 agent 高危工具可分配桌面级 QEMU 虚拟机（独立内核隔离）\n' +
      '• 需要 ' + (probe.value?.accel === 'kvm' ? 'KVM' : 'WHPX') + ' 硬件加速与 base 镜像（首次会下载）\n' +
      '• 建议先完成镜像构建，再开启\n\n' +
      '确认开启？'
    )) return
  }
  form.value.enabled = next
  formDirty.value = true
  void saveConfig()
}

// --- Image build ---
async function buildImage(update: boolean) {
  if (update && !window.confirm(
    '即将更新 base 镜像：\n' +
    '• 新镜像构建成功后自动换装，存量 overlay 安全 rebase（数据完好次序）\n' +
    '• 任何一步失败：新旧两份 base 都在盘上，现有实例不受影响\n\n' +
    '确认开始？'
  )) return
  busy.value = 'build'
  try {
    const r = await request('vm', 'image.build', { update })
    toast.success(r?.note ?? '构建任务已启动')
    await refreshImages()
    ensurePolling()
  } catch (e: any) {
    toast.error('构建启动失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

async function runGc() {
  busy.value = 'gc'
  try {
    const r = await request('vm', 'gc.run')
    toast.success(`孤儿清理完成：清理 ${r?.swept_orphans ?? 0} 个（磁盘 ${r?.used_gb ?? '?'}GB）`)
    await refreshAll()
  } catch (e: any) {
    toast.error('清理失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

// --- Instance ops ---
async function instanceOp(op: 'sleep' | 'wake' | 'kill', agentId: string) {
  if (op === 'kill' && !window.confirm(
    `即将冷停 agent ${agentId} 的 VM：\n` +
    '• QEMU 进程退出（RAM 状态丢弃，磁盘保留）\n' +
    '• 下次启动同盘冷启\n\n' +
    '确认冷停？'
  )) return
  busy.value = `instance_${op}`
  try {
    const r = await request('vm', `instance.${op}`, { agent_id: agentId })
    toast.success(r?.note ?? '操作完成')
    await refreshAll()
  } catch (e: any) {
    toast.error(`操作失败: ` + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

// --- B6 快速试玩 ---
// try.status 只读无闸（查询不是动作）；enabled/镜像/arm64 闸在后端 bridge
// 层、estop 冻结闸在后端 handler 层——前端预闸只为少一次徒劳请求，后端仍兜底。
// 信任级 = dashboard 登录用户（对齐 PTY 终端先例）；命令跑在 VM 整机隔离边界内。
const tryState = ref<any>(null)
const tryRunning = computed(() => !!tryState.value?.running)
const tryStarting = computed(() => !!tryState.value?.starting)
const tryCommand = ref('')
const tryOutput = ref('')
const tryBusy = ref(false)
// B8c：命令超时档（秒；0=缺省 30s 不透传）。guest 上限 600s——apt 全程等
// 长命令不再被 30s 缺省假超时。请求预算 = max(120s, 档位 + 90s 协议余量)。
const tryTimeout = ref(0)

// 预设命令：guest 内秒级可回、不依赖 ICMP（SLIRP 不转发 ping——预设不放
// ping，防「第一条命令就失败」的观感；网络形态看 ip addr）。
const TRY_PRESETS = ['uname -a', 'uptime', 'cat /etc/os-release', 'ip addr', 'df -h']

async function refreshTry() {
  tryState.value = await request('vm', 'try.status').catch(() => null)
}

// 冷启动走后端后台任务（try.start 发起即返回 starting=true），前端轮询
// try.status 转绿——真机教训：同步等冷启 30-60s 必撞默认 30s 请求超时。
const TRY_START_POLL_MS = 2000
const TRY_START_POLL_BUDGET_MS = 180_000

async function tryStart() {
  if (tryStarting.value) return
  tryBusy.value = true
  try {
    const r = await request('vm', 'try.start', {}, 15_000)
    toast.info(r?.note ?? '冷启动已发起')
    const deadline = Date.now() + TRY_START_POLL_BUDGET_MS
    for (;;) {
      await refreshTry()
      if (!tryState.value) return // 网关不可达：状态空，停轮询（下次进页复检）
      if (tryState.value.running) {
        toast.success('试玩实例已就绪（guest agent 在线）')
        return
      }
      if (!tryState.value.starting) {
        const err = tryState.value.last_error
        toast.error(err ? '启动失败: ' + err : '启动未能完成（原因未知，可重试或看网关日志）')
        return
      }
      if (Date.now() >= deadline) {
        toast.error('启动超时（超过 3 分钟仍在进行）——可到「实例」页查看该实例状态')
        return
      }
      await new Promise((res) => setTimeout(res, TRY_START_POLL_MS))
    }
  } catch (e: any) {
    toast.error('启动失败: ' + (e?.message ?? e))
  } finally {
    tryBusy.value = false
    await refreshTry()
  }
}

async function tryRun(cmd?: string) {
  const command = (cmd ?? tryCommand.value).trim()
  if (!command) return
  tryBusy.value = true
  tryOutput.value += `$ ${command}\n`
  try {
    // 请求预算随超时档放大：guest 命令超时 Ns + 握手/协议余量（后端单
    // 调用信封 = N+30s）；缺省档维持 120s 下限（慢命令不假超时）。
    const guestSecs = tryTimeout.value || 30
    const budget = Math.max(120_000, (guestSecs + 90) * 1000)
    const r = await request(
      'vm',
      'try.exec',
      { command, timeout: tryTimeout.value || undefined },
      budget,
    )
    tryOutput.value += (r?.output ?? '') + '\n\n'
    if (!cmd) tryCommand.value = ''
  } catch (e: any) {
    tryOutput.value += `[出错] ${e?.message ?? e}\n\n`
  } finally {
    tryBusy.value = false
    await refreshTry()
  }
}

async function tryStop() {
  if (!window.confirm(
    '即将冷停试玩实例：\n' +
    '• QEMU 进程退出（磁盘状态保留，下次启动同盘冷启）\n\n' +
    '确认停止？'
  )) return
  tryBusy.value = true
  try {
    const r = await request('vm', 'try.stop', {})
    toast.success(r?.note ?? '已停止')
    tryOutput.value = ''
    await refreshTry()
  } catch (e: any) {
    toast.error('停止失败: ' + (e?.message ?? e))
  } finally {
    tryBusy.value = false
  }
}

// --- B9-3 项目工作副本同步 ---
// 状态卡 = vm.sync.status 只读（不开闸、不拽睡眠实例）；commit/resync 是
// agent 活动形态——estop 冻结闸在后端 handler 层，前端预闸只为少一次徒劳
// 请求。冲突裁决卡骑既有提问卡 UI（question-asked SSE → 全局提问队列），
// 本页无独立冲突面。
const syncProjects = ref<any[]>([])
const syncProjectId = ref('')
const syncStatus = ref<any>(null)
const syncBusy = ref(false)

async function refreshSyncProjects() {
  const r = await request('projects', 'list').catch(() => null)
  syncProjects.value = r?.projects ?? []
  if (!syncProjects.value.some((p: any) => p.id === syncProjectId.value)) {
    syncProjectId.value = syncProjects.value[0]?.id ?? ''
    syncStatus.value = null
  }
}

async function refreshSyncStatus() {
  if (!syncProjectId.value) {
    syncStatus.value = null
    return
  }
  syncStatus.value = await request('vm', 'sync.status', { project: syncProjectId.value }).catch(
    (e: any) => ({ _error: e?.message ?? String(e) }),
  )
}

function onSyncProjectChange() {
  syncStatus.value = null
  void refreshSyncStatus()
}

async function syncCommit() {
  if (!syncProjectId.value) return
  syncBusy.value = true
  try {
    const r = await request('vm', 'commit', { project: syncProjectId.value }, 600_000)
    if (r?.no_baseline) {
      toast.info(r?.note ?? '项目尚未开工同步（无基线）')
    } else {
      toast.success(
        `已提交：${r?.changed?.length ?? 0} 个文件回传宿主，${r?.deleted?.length ?? 0} 个删除已同步`,
      )
    }
    await refreshSyncStatus()
  } catch (e: any) {
    toast.error('提交失败: ' + (e?.message ?? e))
  } finally {
    syncBusy.value = false
  }
}

async function syncResync() {
  if (!syncProjectId.value) return
  if (!window.confirm(
    `即将对该项目强制重同步：\n` +
    '• VM 工作副本将被清空\n' +
    '• 以宿主当前内容整体覆盖重建基线\n' +
    '• VM 内未回传的改动将丢失\n\n' +
    '确认继续？'
  )) return
  syncBusy.value = true
  try {
    const r = await request('vm', 'resync', { project: syncProjectId.value }, 600_000)
    toast.success(`重同步完成：${r?.files ?? 0} 个文件已推送（${formatSize(r?.bytes ?? 0)}）`)
    await refreshSyncStatus()
  } catch (e: any) {
    toast.error('重同步失败: ' + (e?.message ?? e))
  } finally {
    syncBusy.value = false
  }
}

function formatSize(n: number): string {
  if (!n) return '0B'
  if (n < 1024 * 1024) return `${Math.round(n / 1024)}K`
  if (n < 1024 * 1024 * 1024) return `${(n / 1048576).toFixed(1)}M`
  return `${(n / 1073741824).toFixed(1)}G`
}

function formatMs(ms: number | null): string {
  if (!ms) return '-'
  return new Date(ms).toLocaleString()
}

onMounted(async () => {
  await refreshAll()
  if (buildRunning.value) ensurePolling()
  if (bsInstallRunning.value) ensureBootstrapPolling()
  // B9-3 同步卡：项目清单 + 当前选中项目的状态（失败静默——卡内已有
  // 诚实错误位，不弹全局 toast 打扰）。
  await refreshSyncProjects()
  await refreshSyncStatus()
})
</script>

<template>
  <div class="page-vm">
    <div class="page-header">
      <h2>虚拟化</h2>
      <span v-if="probe" class="platform-badge">{{ probe.platform === 'windows' ? 'Windows · WHPX' : probe.platform === 'linux' ? 'Linux · KVM' : probe.platform }}</span>
    </div>
    <div class="page-body">

      <!-- 平台总览加载失败 -->
      <div v-if="!overview" class="card">
        <div class="card-body" style="color: var(--text-secondary); font-size: var(--text-sm);">
          {{ loading ? '加载中…' : '无法获取 VM 总览（vm.overview）— 请确认网关以完整构建运行（含 vm feature）。' }}
          <button class="btn btn-sm" style="margin-left: var(--space-2);" @click="refreshAll" :disabled="!!busy || loading">重试</button>
        </div>
      </div>

      <template v-else>
        <!-- ════════ 探针横幅：加速器实测 + 睡眠线 + 档位建议（建议不是决定） ════════ -->
        <div class="card" data-test="probe-banner">
          <div class="card-body" style="display: flex; flex-direction: column; gap: var(--space-1); font-size: var(--text-sm);">
            <div>
              <span :style="{ color: probe?.available ? 'var(--success)' : 'var(--warning, #d97706)' }">{{ probe?.available ? '●' : '○' }}</span>
              <span style="margin-left: var(--space-2); font-weight: 500;">
                {{ probe?.available ? `加速器 ${probe?.accel?.toUpperCase()} 可用（${probe?.layer}）` : `加速器 ${probe?.accel?.toUpperCase()} 不可用` }}
              </span>
              <span v-if="!probe?.available" style="margin-left: var(--space-2); color: var(--text-secondary);">{{ probe?.reason }}</span>
            </div>
            <div style="color: var(--text-secondary);">
              睡眠线：{{ sleepLine ? `✅ ${sleepLine}` : '⚠️ 无' }} — {{ sleepNote }}
            </div>
            <div v-if="suggestion" data-test="suggestion" :style="{ color: suggestion.enabled ? 'var(--success)' : 'var(--text-secondary)' }">
              建议：{{ suggestion.enabled ? '可开启' : '建议保持关闭' }} — {{ suggestion.note }}
            </div>
          </div>
        </div>

        <!-- Tabs -->
        <div class="tabs">
          <button class="tab" :class="{ active: activeTab === 'config' }" @click="activeTab = 'config'">配置</button>
          <button class="tab" :class="{ active: activeTab === 'images' }" @click="activeTab = 'images'">镜像</button>
          <button class="tab" :class="{ active: activeTab === 'instances' }" @click="activeTab = 'instances'">实例</button>
          <button class="btn btn-sm" style="margin-left: auto;" @click="refreshAll" :disabled="!!busy">刷新</button>
        </div>

        <!-- ════════ 配置 ════════ -->
        <div v-if="activeTab === 'config'">
          <!-- 总开关 -->
          <div class="card" data-test="master-switch-card">
            <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
              <h3 style="margin: 0;">总开关</h3>
              <button
                class="btn btn-sm"
                :class="{ 'btn-primary': form.enabled }"
                data-test="master-toggle"
                @click="toggleEnabled"
                :disabled="!!busy"
              >
                {{ form.enabled ? '已开启' : '已关闭' }}
              </button>
            </div>
            <div class="card-body" style="font-size: var(--text-sm); color: var(--text-secondary);">
              <div>
                <span :style="{ color: form.enabled ? 'var(--success)' : 'var(--text-secondary)' }">{{ form.enabled ? '●' : '○' }}</span>
                <span style="margin-left: var(--space-2);">
                  {{ form.enabled ? 'VM 档开启：agent 高危工具可分配桌面级虚拟机（独立内核隔离）' : 'VM 档关闭：零 QEMU 进程、零镜像下载，工具走原路径（默认态）' }}
                </span>
              </div>
              <div style="margin-top: var(--space-1); font-size: var(--text-xs);">
                开关默认关——是否启用由你决定，页面绝不代开。开启需保存后重启 Agent 生效。
              </div>
            </div>
          </div>

          <!-- ════════ 环境安装分步卡（B4：对齐沙盒页「环境管理」） ════════ -->
          <div class="card" data-test="env-card">
            <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
              <h3 style="margin: 0;">环境安装</h3>
              <button class="btn btn-sm" data-test="env-refresh" @click="refreshAll" :disabled="!!busy">复检</button>
            </div>
            <div class="card-body" style="font-size: var(--text-sm);">

              <!-- 关闭态诚实提示：安装动作 enabled-gated（后端同样拒绝），状态检测始终可看 -->
              <div v-if="!enabled" data-test="env-gate-hint" style="margin-bottom: var(--space-3); padding: var(--space-2) var(--space-3); border: 1px dashed var(--border); border-radius: 4px; color: var(--text-secondary);">
                VM 档当前关闭（默认态）：以下安装动作在开启总开关并保存后可用；状态检测始终可看。
              </div>

              <!-- 状态获取失败 -->
              <div v-if="!bootstrap" style="color: var(--text-secondary);">
                环境状态获取失败（bootstrap.status）。
                <button class="btn btn-sm" @click="refreshAll" :disabled="!!busy">重试</button>
              </div>
              <template v-else>

                <!-- ① QEMU 环境 -->
                <div data-test="step-qemu" style="margin-bottom: var(--space-4);">
                  <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-2);">
                    <span style="font-weight: 500;">① QEMU 环境</span>
                    <button class="btn btn-sm" data-test="qemu-install" @click="installQemu" :disabled="!!busy || bsInstallRunning || qemuState === 'ready' || !enabled">
                      {{ qemuState === 'ready' ? '已就绪' : '一键安装' }}
                    </button>
                  </div>
                  <div style="padding-left: var(--space-4); color: var(--text-secondary);">
                    <span :style="{ color: qemuState === 'ready' ? 'var(--success)' : 'var(--text-secondary)' }">{{ qemuState === 'ready' ? '●' : '○' }}</span>
                    <span style="margin-left: var(--space-2);" data-test="qemu-state">
                      <template v-if="qemuState === 'ready'">QEMU 可执行文件齐备（{{ bootstrap.qemu.source }}）</template>
                      <template v-else-if="qemuState === 'partial'">部分缺失（缺：{{ (bootstrap.qemu.missing ?? []).join('、') }}）— 一键下载 + 解压最小集（无 UAC）</template>
                      <template v-else-if="qemuState === 'absent'">未安装 — 一键下载 + 解压 QEMU 最小集（无 UAC）</template>
                      <template v-else>状态未知</template>
                    </span>
                  </div>
                  <div v-if="bsInstallRunning" data-test="qemu-running" style="padding-left: var(--space-4); margin-top: var(--space-2); display: flex; align-items: center; gap: var(--space-2); color: var(--accent);">
                    <div class="spinner spinner-sm"></div>
                    <span>QEMU 安装进行中（下载 + 解压，无 UAC）… 完成后自动复检转绿</span>
                  </div>
                </div>

                <!-- ② 硬件加速 -->
                <div data-test="step-accel" style="border-top: 1px solid var(--border); padding-top: var(--space-4); margin-bottom: var(--space-4);">
                  <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-2);">
                    <span style="font-weight: 500;">② 硬件加速</span>
                    <button v-if="bootstrap.accel?.action === 'button'" class="btn btn-sm btn-primary" data-test="accel-enable" @click="enableAccel" :disabled="!!busy || !enabled">一键启用（UAC）</button>
                  </div>
                  <div style="padding-left: var(--space-4); display: flex; flex-direction: column; gap: var(--space-1); color: var(--text-secondary);">
                    <div>
                      <span :style="{ color: accelState === 'ok' ? 'var(--success)' : 'var(--text-secondary)' }">{{ accelState === 'ok' ? '●' : '○' }}</span>
                      <span style="margin-left: var(--space-2);" data-test="accel-state">
                        <template v-if="accelState === 'ok'">硬件加速可用</template>
                        <template v-else-if="accelState === 'unknown'">诊断未定：{{ bootstrap.accel.reason }}</template>
                        <template v-else-if="bootstrap.accel?.note">{{ bootstrap.accel.note }}</template>
                        <template v-else>状态未知</template>
                      </span>
                    </div>
                    <!-- 引导命令 + 复制（needs_enable_windows / needs_kmod / needs_group） -->
                    <div v-if="bootstrap.accel?.command" style="display: flex; align-items: center; gap: var(--space-2);">
                      <code data-test="accel-command" style="flex: 1; overflow-x: auto; white-space: nowrap;">{{ bootstrap.accel.command }}</code>
                      <button class="btn btn-sm" data-test="accel-copy" @click="copyCommand(bootstrap.accel.command)">复制</button>
                    </div>
                  </div>
                </div>

                <!-- ③ 基础镜像（B7 构建路由：Windows 宿主自动经 WSL，缺什么补什么） -->
                <div data-test="step-image" style="border-top: 1px solid var(--border); padding-top: var(--space-4);">
                  <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-2);">
                    <span style="font-weight: 500;">③ 基础镜像</span>
                    <div style="display: flex; gap: var(--space-2);">
                      <button class="btn btn-sm" @click="activeTab = 'images'">查看构建日志</button>
                      <button v-if="!imageReady" class="btn btn-sm btn-primary" data-test="image-build" @click="buildImage(false)" :disabled="!!busy || buildRunning || !enabled || !imageRouteReady || !guestReady" :title="imageRoute?.note || ''">首次构建</button>
                    </div>
                  </div>
                  <div style="padding-left: var(--space-4); color: var(--text-secondary);">
                    <span :style="{ color: imageReady ? 'var(--success)' : 'var(--text-secondary)' }">{{ imageReady ? '●' : '○' }}</span>
                    <span style="margin-left: var(--space-2);" data-test="image-state">
                      <template v-if="imageReady">base 镜像已就绪（构建/更新与日志见「镜像」页签）</template>
                      <template v-else-if="imageRouteReady && imageRouteIsWsl">未构建 — 点「首次构建」自动经 WSL 跑构建管线（下载 base + 注入 guest 代理，产物直落镜像目录）</template>
                      <template v-else-if="imageRouteReady">未构建 — 点「首次构建」下载 base 并注入 guest 代理（日志见「镜像」页签）</template>
                      <template v-else>{{ imageRoute?.note || '未构建' }}</template>
                    </span>
                  </div>
                  <!-- 构建路由引导（B7）：每态给可执行下一步 + 复制 -->
                  <div v-if="!imageReady && imageRoute && !imageRouteReady" data-test="image-route-guide" style="padding-left: var(--space-4); margin-top: var(--space-2); display: flex; flex-direction: column; gap: var(--space-1); color: var(--text-secondary);">
                    <div v-if="imageRoute.missing?.length" data-test="image-route-missing" style="color: var(--warning, #b58900);">
                      缺：{{ imageRoute.missing.join('、') }}
                    </div>
                    <div v-if="imageRoute.command" style="display: flex; align-items: center; gap: var(--space-2);">
                      <code data-test="image-route-command" style="flex: 1; overflow-x: auto; white-space: nowrap;">{{ imageRoute.command }}</code>
                      <button class="btn btn-sm" data-test="image-route-copy" @click="copyCommand(imageRoute.command!)">复制</button>
                    </div>
                    <div v-if="imageRoute.note" data-test="image-route-note" style="font-size: var(--text-xs);">{{ imageRoute.note }}</div>
                  </div>
                  <!-- guest 代理未构建：Windows 侧 git-bash 直接交叉编译（guest 依赖纯 Rust，无需 WSL 工具链） -->
                  <div v-if="!imageReady && !guestReady" data-test="guest-missing-hint" style="padding-left: var(--space-4); margin-top: var(--space-1); font-size: var(--text-xs); color: var(--text-secondary);">
                    guest 代理二进制未构建——在 git-bash 执行后点「复检」：<code style="user-select: all;">{{ guestBuildCmd }}</code>
                  </div>
                  <div v-if="buildRunning" style="padding-left: var(--space-4); margin-top: var(--space-2); display: flex; align-items: center; gap: var(--space-2); color: var(--accent);">
                    <div class="spinner spinner-sm"></div>
                    <span>镜像构建进行中… 完成后自动换装</span>
                  </div>
                </div>

                <!-- 安装日志尾部（QEMU 安装任务产出；镜像构建日志在「镜像」页签） -->
                <div v-if="bootstrap.log?.tail?.length" style="margin-top: var(--space-4); padding-top: var(--space-4); border-top: 1px solid var(--border);">
                  <div style="font-size: var(--text-xs); color: var(--text-secondary); margin-bottom: var(--space-1);">
                    安装日志尾部（<code>{{ bootstrap.log.path }}</code>）
                  </div>
                  <pre data-test="bs-log" style="margin: 0; max-height: 180px; overflow: auto; font-size: var(--text-xs); font-family: var(--font-mono); background: var(--bg-secondary, rgba(0,0,0,0.05)); padding: var(--space-2); border-radius: 4px;">{{ bootstrap.log.tail.join('\n') }}</pre>
                </div>
              </template>
            </div>
          </div>

          <!-- 基础配置 -->
          <div class="card">
            <div class="card-header"><h3 style="margin: 0;">基础配置</h3></div>
            <div class="card-body" style="display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: var(--space-3); font-size: var(--text-sm);">
              <label style="display: flex; flex-direction: column; gap: var(--space-1);">
                <span>Guest 架构</span>
                <select v-model="form.arch" data-test="arch" @change="formDirty = true">
                  <option value="x86_64">x86_64（当前产出档）</option>
                  <option value="aarch64">aarch64（接口预留，暂不产出）</option>
                </select>
              </label>
              <label style="display: flex; flex-direction: column; gap: var(--space-1);">
                <span>硬件加速</span>
                <select v-model="form.hypervisor" data-test="hypervisor" @change="formDirty = true">
                  <option value="auto">auto（探测裁决）</option>
                  <option value="kvm">kvm（Linux KVM）</option>
                  <option value="whpx">whpx（Windows WHPX）</option>
                </select>
              </label>
              <label style="display: flex; flex-direction: column; gap: var(--space-1);">
                <span>桌面形态</span>
                <select v-model="form.desktop" data-test="desktop" @change="formDirty = true">
                  <option value="minimal">minimal（轻量 WM + Chromium，默认）</option>
                  <option value="full">full（完整桌面环境）</option>
                </select>
              </label>
            </div>
          </div>

          <!-- 镜像来源 -->
          <div class="card">
            <div class="card-header"><h3 style="margin: 0;">镜像来源</h3></div>
            <div class="card-body" style="display: flex; flex-direction: column; gap: var(--space-3); font-size: var(--text-sm);">
              <label style="display: flex; flex-direction: column; gap: var(--space-1);">
                <span>预设</span>
                <select :value="presetIdx" data-test="preset" @change="onPresetChange">
                  <option v-for="(p, i) in IMAGE_PRESETS" :key="p.url" :value="i">{{ p.label }}</option>
                  <option v-if="presetIdx < 0" :value="-1" disabled>（自定义来源）</option>
                </select>
              </label>
              <label style="display: flex; flex-direction: column; gap: var(--space-1);">
                <span>镜像 URL（可自由编辑）</span>
                <input
                  v-model="form.image_source"
                  data-test="image-source"
                  type="text"
                  spellcheck="false"
                  style="font-family: var(--font-mono);"
                  @input="formDirty = true"
                />
              </label>
              <label style="display: flex; flex-direction: column; gap: var(--space-1);">
                <span>SHA-256（空 = 构建时从来源同目录 SHA256SUMS 查表；换源必须钉死）</span>
                <input
                  v-model="form.image_sha256"
                  data-test="image-sha256"
                  type="text"
                  spellcheck="false"
                  placeholder="留空 = sidecar 查表"
                  style="font-family: var(--font-mono);"
                  @input="formDirty = true"
                />
              </label>
            </div>
          </div>

          <!-- 资源与纪律面 -->
          <div class="card">
            <div class="card-header"><h3 style="margin: 0;">资源与纪律面</h3></div>
            <div class="card-body" style="display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: var(--space-3); font-size: var(--text-sm);">
              <label style="display: flex; flex-direction: column; gap: var(--space-1);">
                <span>vCPU / 台</span>
                <input v-model.number="form.vcpu" data-test="vcpu" type="number" min="1" max="64" @input="formDirty = true" />
              </label>
              <label style="display: flex; flex-direction: column; gap: var(--space-1);">
                <span>内存 MB / 台</span>
                <input v-model.number="form.ram_mb" data-test="ram" type="number" min="256" @input="formDirty = true" />
              </label>
              <label style="display: flex; flex-direction: column; gap: var(--space-1);">
                <span>磁盘 GB / 台</span>
                <input v-model.number="form.disk_gb" data-test="disk" type="number" min="5" @input="formDirty = true" />
              </label>
              <label style="display: flex; flex-direction: column; gap: var(--space-1);">
                <span>实例上限</span>
                <input v-model.number="form.max_instances" data-test="max-instances" type="number" min="1" @input="formDirty = true" />
              </label>
              <label style="display: flex; flex-direction: column; gap: var(--space-1);">
                <span>空闲自动睡眠（秒，0=不睡）</span>
                <input v-model.number="form.idle_sleep_secs" data-test="idle-secs" type="number" min="0" @input="formDirty = true" />
              </label>
              <label style="display: flex; flex-direction: column; gap: var(--space-1);">
                <span>磁盘总预算 GB（0=不限）</span>
                <input v-model.number="form.gc_max_total_disk_gb" data-test="gc-budget" type="number" min="0" @input="formDirty = true" />
              </label>
              <label style="display: flex; flex-direction: column; gap: var(--space-1);">
                <span>超限动作</span>
                <select v-model="form.gc_over_limit" data-test="gc-over-limit" @change="formDirty = true">
                  <option value="warn">warn（告警不停机）</option>
                  <option value="stop">stop（拒绝新实例）</option>
                </select>
              </label>
            </div>
          </div>

          <!-- 保存 -->
          <div class="card">
            <div class="card-body" style="display: flex; justify-content: flex-end; align-items: center; gap: var(--space-3);">
              <span v-if="formDirty" data-test="dirty-hint" style="font-size: var(--text-xs); color: var(--warning, #d97706);">有未保存修改</span>
              <button class="btn btn-primary" data-test="save" @click="saveConfig" :disabled="!!busy">保存配置</button>
            </div>
          </div>
        </div>

        <!-- ════════ 镜像 ════════ -->
        <div v-if="activeTab === 'images'">
          <div class="card">
            <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
              <h3 style="margin: 0;">base 镜像</h3>
              <div style="display: flex; gap: var(--space-2);">
                <button class="btn btn-sm" data-test="build-first" @click="buildImage(false)" :disabled="!!busy || buildRunning">首次构建</button>
                <button class="btn btn-sm" data-test="build-update" @click="buildImage(true)" :disabled="!!busy || buildRunning">更新镜像</button>
              </div>
            </div>
            <div class="card-body" style="font-size: var(--text-sm);">
              <div v-if="buildRunning" data-test="build-running" style="display: flex; align-items: center; gap: var(--space-2); color: var(--accent); margin-bottom: var(--space-3);">
                <div class="spinner"></div>
                <span>镜像构建进行中（{{ image?.build?.update ? '更新' : '首次' }}）… 完成后自动换装</span>
              </div>
              <div v-if="!image?.images?.length" style="color: var(--text-secondary);">
                暂无镜像。<template v-if="enabled">点「首次构建」下载 base 镜像并注入 guest 代理（构建脚本在 Linux/WSL 宿主运行，日志见下方）。</template>
                <template v-else>VM 档关闭中；构建功能在开启 VM 档后可用。</template>
              </div>
              <div v-else style="display: flex; flex-direction: column;">
                <div
                  v-for="img in image.images"
                  :key="img.name"
                  data-test="image-row"
                  style="display: flex; align-items: baseline; gap: var(--space-3); padding: var(--space-1) 0; border-bottom: 1px solid var(--border);"
                >
                  <code style="flex-shrink: 0;">{{ img.name }}</code>
                  <span style="flex: 1; color: var(--text-secondary); font-family: var(--font-mono); font-size: var(--text-xs); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;" :title="image.images_root">{{ image.images_root }}/{{ img.name }}</span>
                  <span style="flex-shrink: 0; color: var(--text-secondary);">{{ formatSize(img.size_bytes) }}</span>
                  <span style="flex-shrink: 0; color: var(--text-secondary); font-size: var(--text-xs);">{{ formatMs(img.modified_ms) }}</span>
                </div>
              </div>
              <!-- GC 状态 -->
              <div v-if="gc" style="margin-top: var(--space-3); padding-top: var(--space-3); border-top: 1px solid var(--border); display: flex; justify-content: space-between; align-items: center;">
                <span style="color: var(--text-secondary);" data-test="gc-status">
                  磁盘用量 {{ gc.used_gb }}GB
                  <template v-if="gc.budget_gb > 0">/ 预算 {{ gc.budget_gb }}GB（{{ gcDecisionLabel(gc.decision) }}）</template>
                </span>
                <button class="btn btn-sm" data-test="gc-run" @click="runGc" :disabled="!!busy">清理孤儿 / 刷新用量</button>
              </div>
              <!-- 构建日志尾部 -->
              <div v-if="image?.log?.tail?.length" style="margin-top: var(--space-3); padding-top: var(--space-3); border-top: 1px solid var(--border);">
                <div style="font-size: var(--text-xs); color: var(--text-secondary); margin-bottom: var(--space-1);">
                  构建日志尾部（<code>{{ image.log.path }}</code>）
                </div>
                <pre data-test="build-log" style="margin: 0; max-height: 240px; overflow: auto; font-size: var(--text-xs); font-family: var(--font-mono); background: var(--bg-secondary, rgba(0,0,0,0.05)); padding: var(--space-2); border-radius: 4px;">{{ image.log.tail.join('\n') }}</pre>
              </div>
            </div>
          </div>
        </div>

        <!-- ════════ 实例 ════════ -->
        <div v-if="activeTab === 'instances'">
          <!-- B6 快速试玩卡 -->
          <div class="card" data-test="try-card" style="margin-bottom: var(--space-4);">
            <div class="card-header"><h3 style="margin: 0;">快速试玩</h3></div>
            <div class="card-body" style="font-size: var(--text-sm);">
              <div v-if="!tryState" style="color: var(--text-secondary);">试玩状态不可用（网关未装配 VM 管理面）。</div>
              <div v-else-if="!tryState.enabled" data-test="try-off" style="color: var(--text-secondary);">
                VM 档未开启——先到「配置」页打开总开关，再回来试玩。
                <button class="btn btn-sm" style="margin-left: var(--space-2);" @click="activeTab = 'config'">去配置</button>
              </div>
              <div v-else-if="!tryState.arch_supported" data-test="try-arch">
                当前档位 {{ tryState.arch }} 暂不支持试玩（aarch64 镜像管线尚未产出，属接口预留）——切回 x86_64 档可用。
              </div>
              <div v-else-if="!tryState.image_ready" data-test="try-noimage" style="color: var(--text-secondary);">
                基础镜像未就绪——先在「配置」页的环境安装卡完成 QEMU 安装与镜像构建。
                <button class="btn btn-sm" style="margin-left: var(--space-2);" @click="activeTab = 'config'">去环境安装</button>
              </div>
              <template v-else>
                <div v-if="!tryRunning" style="color: var(--text-secondary); margin-bottom: var(--space-2);">
                  一台专属试玩实例（try-dashboard）：启动后在 Debian 里跑几条命令，直观感受整机隔离；闲置约 5 分钟自动睡眠省资源。
                </div>
                <div style="display: flex; gap: var(--space-2); align-items: center; flex-wrap: wrap;">
                  <span :style="{ color: tryRunning ? 'var(--success)' : 'var(--text-secondary)' }" data-test="try-state-label">{{ tryRunning ? '● 运行中' : (tryStarting ? '◐ 启动中…' : '○ 已停止') }}</span>
                  <button v-if="!tryRunning" class="btn btn-sm btn-primary" data-test="try-start" @click="tryStart" :disabled="tryBusy || tryStarting || !!busy">{{ tryStarting ? '启动中…' : '启动试玩实例' }}</button>
                  <button v-if="tryRunning || tryStarting" class="btn btn-sm btn-danger" data-test="try-stop" @click="tryStop" :disabled="!!busy">停止试玩</button>
                  <template v-if="tryRunning">
                    <button
                      v-for="p in TRY_PRESETS"
                      :key="p"
                      class="btn btn-sm"
                      data-test="try-preset"
                      @click="tryRun(p)"
                      :disabled="tryBusy || !!busy"
                    >{{ p }}</button>
                  </template>
                </div>
                <div v-if="tryRunning" style="margin-top: var(--space-2); display: flex; gap: var(--space-2); align-items: center;">
                  <input
                    v-model="tryCommand"
                    data-test="try-input"
                    placeholder="输入 shell 命令（在 VM 内执行，如 ls /）"
                    style="flex: 1;"
                    :disabled="tryBusy || !!busy"
                    @keydown.enter="tryRun()"
                  />
                  <select v-model.number="tryTimeout" data-test="try-timeout" :disabled="tryBusy || !!busy" title="命令超时（apt 等长命令选长档）">
                    <option :value="0">默认 30s</option>
                    <option :value="120">120s</option>
                    <option :value="300">300s</option>
                    <option :value="600">600s</option>
                  </select>
                  <button class="btn btn-sm btn-primary" data-test="try-run" @click="tryRun()" :disabled="tryBusy || !!busy || !tryCommand.trim()">执行</button>
                </div>
                <pre
                  v-if="tryOutput"
                  data-test="try-output"
                  style="margin-top: var(--space-2); max-height: 300px; overflow: auto; background: var(--bg-secondary, rgba(0,0,0,0.05)); padding: var(--space-2); border-radius: 4px; white-space: pre-wrap; word-break: break-all;"
                >{{ tryOutput }}</pre>
              </template>
            </div>
          </div>

          <!-- B9-3 项目工作副本同步卡 -->
          <div class="card" data-test="sync-card" style="margin-bottom: var(--space-4);">
            <div class="card-header"><h3 style="margin: 0;">项目工作副本同步</h3></div>
            <div class="card-body" style="font-size: var(--text-sm);">
              <div v-if="syncProjects.length === 0" data-test="sync-no-projects" style="color: var(--text-secondary);">
                暂无注册项目——侧栏「项目」区注册并开启 VM 档后，项目工作副本每轮对话自动双向对齐；这里提供手动提交与强制重同步。
              </div>
              <template v-else>
                <div style="display: flex; gap: var(--space-2); align-items: center; flex-wrap: wrap; margin-bottom: var(--space-2);">
                  <select v-model="syncProjectId" data-test="sync-project" :disabled="syncBusy" @change="onSyncProjectChange">
                    <option v-for="p in syncProjects" :key="p.id" :value="p.id">{{ p.name }}</option>
                  </select>
                  <button class="btn btn-sm btn-primary" data-test="sync-commit" :disabled="syncBusy || !!busy" @click="syncCommit">一键提交</button>
                  <button class="btn btn-sm btn-danger" data-test="sync-resync" :disabled="syncBusy || !!busy" @click="syncResync">强制重同步</button>
                  <span style="color: var(--text-secondary); font-size: var(--text-xs);">每轮对话结束自动回传；「一键提交」立即对齐一次。冲突时弹出裁决卡。</span>
                </div>
                <div v-if="syncStatus?._error" data-test="sync-error" style="color: var(--danger, #c0392b);">{{ syncStatus._error }}</div>
                <template v-else-if="syncStatus">
                  <div v-if="!syncStatus.has_baseline" data-test="sync-no-baseline" style="color: var(--text-secondary);">
                    尚未开工同步（无基线）——给该项目会话发一条消息即触发开工全量同步。
                  </div>
                  <template v-else>
                    <div data-test="sync-baseline" style="color: var(--text-secondary);">
                      基线：{{ syncStatus.baseline_files }} 个文件 · {{ formatMs(syncStatus.synced_at_ms) }} · 选源 {{ syncStatus.selection }}
                    </div>
                    <div data-test="sync-pending" style="margin-top: var(--space-1);">
                      <span v-if="syncStatus.host_pending.length === 0 && syncStatus.host_pending_deletes.length === 0" style="color: var(--success);">✓ 宿主无待同步变更</span>
                      <span v-else style="color: var(--warning, #b58900);">
                        宿主待同步：{{ syncStatus.host_pending.length }} 改 / {{ syncStatus.host_pending_deletes.length }} 删（下轮开工自动对齐，或「一键提交」立即推）
                      </span>
                    </div>
                    <div v-if="syncStatus.last_commit" data-test="sync-last-commit" style="margin-top: var(--space-1); color: var(--text-secondary); font-size: var(--text-xs);">
                      上次提交：{{ formatMs(syncStatus.last_commit.committed_at_ms) }} — {{ syncStatus.last_commit.changed?.length ?? 0 }} 改 / {{ syncStatus.last_commit.deleted?.length ?? 0 }} 删 / {{ formatSize(syncStatus.last_commit.bytes ?? 0) }}
                    </div>
                  </template>
                </template>
              </template>
            </div>
          </div>

          <div class="card">
            <div class="card-header"><h3 style="margin: 0;">VM 实例</h3></div>
            <div class="card-body" style="font-size: var(--text-sm);">
              <div v-if="instances.length === 0" style="color: var(--text-secondary);">
                暂无实例。VM 档开启后，agent 触发隔离执行时按需创建（每 agent 至多一台，睡眠/唤醒/停机都在这里管理）。
              </div>
              <div v-else style="display: flex; flex-direction: column;">
                <div
                  v-for="inst in instances"
                  :key="inst.agent_id"
                  data-test="instance-row"
                  style="display: flex; align-items: center; gap: var(--space-2); padding: var(--space-2) 0; border-bottom: 1px solid var(--border); flex-wrap: wrap;"
                >
                  <span :style="{ color: inst.running ? 'var(--success)' : 'var(--text-secondary)' }">{{ inst.running ? '●' : '○' }}</span>
                  <code style="flex-shrink: 0;" :title="inst.agent_id">{{ inst.agent_id.slice(0, 12) }}</code>
                  <span class="instance-tag">{{ inst.running ? '运行中' : inst.asleep_at ? `睡眠（${sleepLine ?? '冷'}）` : '已停止' }}</span>
                  <span style="color: var(--text-secondary); font-size: var(--text-xs);">{{ inst.arch }} · {{ inst.accel }} · {{ inst.vcpu }}C/{{ inst.ram_mb }}M/{{ inst.disk_gb }}G</span>
                  <span v-if="inst.has_snapshot" class="instance-tag">有快照</span>
                  <span v-if="inst.has_state_file" class="instance-tag">有状态文件</span>
                  <span style="flex: 1;"></span>
                  <div style="display: flex; gap: var(--space-2); flex-shrink: 0;">
                    <button v-if="inst.running" class="btn btn-sm" data-test="inst-sleep" @click="instanceOp('sleep', inst.agent_id)" :disabled="!!busy">睡眠</button>
                    <button v-else class="btn btn-sm btn-primary" data-test="inst-wake" @click="instanceOp('wake', inst.agent_id)" :disabled="!!busy">唤醒</button>
                    <button v-if="inst.running" class="btn btn-sm btn-danger" data-test="inst-kill" @click="instanceOp('kill', inst.agent_id)" :disabled="!!busy">冷停</button>
                  </div>
                </div>
              </div>
            </div>
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
.instance-tag {
  flex-shrink: 0;
  font-size: var(--text-xs);
  color: var(--text-secondary);
  border: 1px solid var(--border);
  border-radius: 999px;
  padding: 0 8px;
}
</style>
