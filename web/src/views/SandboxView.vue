<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

// --- State ---
const loading = ref(true)
const activeTab = ref<'inside' | 'config'>('config')
const status = ref<any>(null)        // sandbox.service status
const env = ref<any>(null)           // full env check (7z + sandboxie)
const pending = ref<any[]>([])       // pending workspace files
const busy = ref<string | null>(null) // action in progress

const ready = computed(() => !!status.value?.ready)
const sevenZipOk = computed(() => !!env.value?.seven_zip?.available)
const filesAcquired = computed(() => !!env.value?.sandboxie?.files_acquired)
const driverInstalled = computed(() => !!env.value?.sandboxie?.driver_installed)
const sbiesvcRunning = computed(() => !!env.value?.sandboxie?.sbiesvc_running)

async function refresh() {
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
    toast.success('环境检查完成')
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
    toast.success('Sandboxie 安装完成')
    await checkEnv()
    await refresh()
  } catch (e: any) {
    toast.error('Sandboxie 安装失败: ' + (e?.message ?? e))
  } finally {
    busy.value = null
  }
}

async function startSandboxie() {
  busy.value = 'start'
  try {
    await request('sandbox', 'start')
    toast.success('Sandboxie 引擎已启动')
    await refresh()
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
    toast.success('Sandboxie 引擎已停止')
    await refresh()
    await checkEnv()
  } catch (e: any) {
    toast.error('停止失败: ' + (e?.message ?? e))
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

onMounted(async () => {
  await Promise.all([refresh(), checkEnv()])
})
</script>

<template>
  <div class="page-sandbox">
    <div class="page-header"><h2>沙盒</h2></div>
    <div class="page-body">

      <!-- Tabs -->
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'inside' }" @click="activeTab = 'inside'">沙箱内部</button>
        <button class="tab" :class="{ active: activeTab === 'config' }" @click="activeTab = 'config'">沙箱配置</button>
        <button class="btn btn-sm" style="margin-left: auto;" @click="refresh" :disabled="!!busy">刷新</button>
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
             : '正在检查环境...' }}
          </span>
        </div>
      </div>

      <!-- ════════ 沙箱内部 ════════ -->
      <div v-if="activeTab === 'inside'" style="display: flex; flex-direction: column; gap: var(--space-3);">
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
            </div>
          </div>
        </div>

        <div class="card">
          <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
            <h3 style="margin: 0;">待提交文件（沙箱内工作区变更）</h3>
            <span style="font-size: var(--text-sm); color: var(--text-secondary);">{{ pending.length }} 个</span>
          </div>
          <div class="card-body">
            <div v-if="pending.length === 0" style="color: var(--text-secondary); font-size: var(--text-sm);">暂无待提交文件。</div>
            <div v-else style="display: flex; flex-direction: column; gap: var(--space-1); font-size: var(--text-sm); font-family: var(--font-mono);">
              <div v-for="p in pending" :key="p.real_path" style="display: flex; justify-content: space-between; align-items: center;">
                <span>{{ p.real_path }}</span>
                <span style="color: var(--text-secondary);">{{ formatSize(p.size) }}</span>
              </div>
            </div>
          </div>
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

            <!-- Sandboxie 文件 (acquire: download + extract; no UAC) -->
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

    </div>
  </div>
</template>

<style scoped>
code { background: var(--bg-secondary, rgba(0,0,0,0.05)); padding: 1px 4px; border-radius: 3px; font-size: var(--text-xs); }
</style>
