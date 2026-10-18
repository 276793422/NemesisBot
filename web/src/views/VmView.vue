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
    await refreshImages()
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
