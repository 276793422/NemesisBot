<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

// --- State ---
const loading = ref(true)
const activeTab = ref<'config' | 'status' | 'files'>('config')
const status = ref<any>(null)
const env = ref<any>(null)
const pending = ref<any[]>([])
const busy = ref<string | null>(null)
const selected = ref<Set<string>>(new Set())

const ready = computed(() => !!status.value?.ready)
const sevenZipOk = computed(() => !!env.value?.seven_zip?.available)
const filesAcquired = computed(() => !!env.value?.sandboxie?.files_acquired)
const driverInstalled = computed(() => !!env.value?.sandboxie?.driver_installed)
const sbiesvcRunning = computed(() => !!env.value?.sandboxie?.sbiesvc_running)

async function refreshAll() {
  loading.value = true
  try {
    const [st, pend] = await Promise.all([
      request('sandbox', 'status').catch(() => null),
      request('sandbox', 'pending').catch(() => []),
    ])
    status.value = st
    pending.value = Array.isArray(pend) ? pend : (pend?.files ?? [])
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
  busy.value = 'start'
  try {
    await request('sandbox', 'start')
    toast.success('Sandboxie 引擎已启动 · config 已更新 (executor+sandbox=true)。重启 gateway 生效。')
    await refreshAll()
    await checkEnv()
  } catch (e: any) {
    toast.error('启动失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

async function stopSandboxie() {
  busy.value = 'stop'
  try {
    await request('sandbox', 'stop')
    toast.success('Sandboxie 引擎已停止 · config 已更新 (executor+sandbox=false)。重启 gateway 生效。')
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

onMounted(async () => {
  await Promise.all([refreshAll(), checkEnv()])
})
</script>

<template>
  <div class="page-sandbox">
    <div class="page-header"><h2>沙盒</h2></div>
    <div class="page-body">

      <!-- Tabs -->
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'config' }" @click="activeTab = 'config'">沙箱配置</button>
        <button class="tab" :class="{ active: activeTab === 'status' }" @click="activeTab = 'status'">沙箱状态</button>
        <button class="tab" :class="{ active: activeTab === 'files' }" @click="activeTab = 'files'">沙箱文件</button>
        <button class="btn btn-sm" style="margin-left: auto;" @click="refreshAll" :disabled="!!busy">刷新</button>
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
             : '正在检查环境...' }}
          </span>
        </div>
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
                <button class="btn btn-sm" @click="openBox" :disabled="!status?.box_root">打开沙箱</button>
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

    </div>
  </div>
</template>

<style scoped>
code { background: var(--bg-secondary, rgba(0,0,0,0.05)); padding: 1px 4px; border-radius: 3px; font-size: var(--text-xs); }
</style>
