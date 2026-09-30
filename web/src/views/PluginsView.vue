<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

// 插件页（2026-09-29 三 Tab 重构）：异质插件体系各自成 Tab，不再单页混装。
// - WASM 插件（默认 Tab，主角）：沙盒化组件插件管理面 + 「是什么/怎么做」
//   引导 + 开发包（devkit）一键下载——guide 原则：告诉用户是什么、怎么做。
// - 本地插件库：C-ABI 动态库（plugin_onnx/plugin_ui），只读探测总览。
// - 管线插件：T2 三段化（pre/around/post）进程内插件，启停即时生效。
// 编译期子系统 feature 状态已迁「关于」页「构建形态」tab（system.features）。

interface PluginEntry {
  id: string
  label: string
  used_by: string
  found: boolean
  filename: string
  path?: string
  capabilities?: string[]
  detail?: any
}

interface PipelinePlugin {
  name: string
  scope: string | null
  enabled: boolean
  description: string
}

interface WasmPluginRow {
  slug: string
  name?: string
  version: string
  kind: string
  trust: string
  enabled: boolean
  loaded: boolean
  wasm_sha256?: string
  min_tier?: string
  egress?: string[]
  x_secret?: string[]
  tool?: { name: string; description: string; operation_type: string; min_tier: string } | null
  signed_by?: string
}

interface WasmConfigData {
  slug: string
  enabled: boolean
  entries: Record<string, string>
  schema_keys: string[]
  secret_keys: string[]
}

interface WasmLogLine {
  ts_ms: number
  level: string
  message: string
}

interface DevkitInfo {
  path: string
  dir: string
  size: number
  files: number
}

const { request } = useWSAPI()
const toast = useToast()

// 前端同步裁剪（customize 流程）：VITE_FEATURE_PLUGINS_WASM=false 时整个
// WASM Tab 隐藏（构建期 tree-shake 由路由级门控承担；页内 Tab 用运行时常量）。
const wasmFeatureEnabled = import.meta.env.VITE_FEATURE_PLUGINS_WASM !== 'false'

const activeTab = ref(wasmFeatureEnabled ? 'wasm' : 'native')

const plugins = ref<PluginEntry[]>([])
const pipelinePlugins = ref<PipelinePlugin[]>([])
const loading = ref(true)

const wasmPlugins = ref<WasmPluginRow[]>([])
const wasmAvailable = ref<boolean | null>(null) // null = 未探测
const wasmInstallDir = ref('')
const wasmAllowUnsigned = ref(false)
const wasmInstalling = ref(false)
const wasmExpanded = ref<string>('') // 展开详情的 slug（config/logs 二选一切换）
const wasmDetailTab = ref<'config' | 'logs'>('config')
const wasmConfig = ref<WasmConfigData | null>(null)
const wasmLogs = ref<WasmLogLine[]>([])
const wasmLogDropped = ref(0)

// devkit 下载（wasm.devkit_download）：zip 落 workspace/wasm-plugin-devkit/，
// 解包 devkit/；已存在默认拒绝，勾「覆盖」重下。
const devkitBusy = ref(false)
const devkitInfo = ref<DevkitInfo | null>(null)
const devkitOverwrite = ref(false)

const tabs = computed(() => {
  const t = [{ id: 'native', label: '本地插件库' }, { id: 'pipeline', label: '管线插件' }]
  return wasmFeatureEnabled ? [{ id: 'wasm', label: 'WASM 插件' }, ...t] : t
})

function humanSize(n: number): string {
  if (n >= 1024 * 1024) return (n / (1024 * 1024)).toFixed(1) + ' MB'
  if (n >= 1024) return (n / 1024).toFixed(1) + ' KB'
  return n + ' B'
}

async function loadPlugins() {
  loading.value = true
  try {
    const data = await request('plugins', 'list')
    plugins.value = data?.plugins || []
    pipelinePlugins.value = data?.pipeline_plugins || []
  } catch (e: any) {
    toast.error('加载插件状态失败: ' + e)
  }
  loading.value = false
}

async function togglePipeline(p: PipelinePlugin) {
  try {
    const data = await request('plugins', 'set_metrics_enabled', { enabled: !p.enabled })
    p.enabled = data?.enabled ?? !p.enabled
    toast.success(`管线插件 ${p.name} 已${p.enabled ? '启用' : '停用'}`)
  } catch (e: any) {
    toast.error('切换失败: ' + e)
  }
}

async function loadWasm() {
  try {
    const data = await request('plugins', 'wasm.list')
    wasmPlugins.value = data?.plugins || []
    wasmAvailable.value = true
  } catch {
    wasmAvailable.value = false
  }
}

async function downloadDevkit() {
  devkitBusy.value = true
  try {
    const data = await request('plugins', 'wasm.devkit_download', { overwrite: devkitOverwrite.value })
    devkitInfo.value = data
    devkitOverwrite.value = false
    toast.success('开发包已下载并解包，按引导三步即可编译出可安装插件')
  } catch (e: any) {
    toast.error('开发包下载失败: ' + e)
  }
  devkitBusy.value = false
}

async function wasmInstall() {
  const dir = wasmInstallDir.value.trim()
  if (!dir) {
    toast.error('请填写插件源目录（含 plugin.toml）')
    return
  }
  wasmInstalling.value = true
  try {
    const data = await request('plugins', 'wasm.install', {
      source_dir: dir,
      allow_unsigned: wasmAllowUnsigned.value,
    })
    toast.success(`插件 ${data?.plugin?.slug || dir} 安装完成（九步漏斗通过）`)
    wasmInstallDir.value = ''
    await loadWasm()
  } catch (e: any) {
    toast.error('安装失败: ' + e)
  }
  wasmInstalling.value = false
}

async function wasmToggle(p: WasmPluginRow) {
  try {
    await request('plugins', p.enabled ? 'wasm.disable' : 'wasm.enable', { slug: p.slug })
    p.enabled = !p.enabled
    toast.success(`插件 ${p.slug} 已${p.enabled ? '启用' : '停用'}`)
  } catch (e: any) {
    toast.error('切换失败: ' + e)
  }
}

async function wasmUninstall(p: WasmPluginRow) {
  if (!confirm(`卸载插件 ${p.slug}？（载荷删除，数据目录保留）`)) return
  try {
    await request('plugins', 'wasm.uninstall', { slug: p.slug })
    toast.success(`插件 ${p.slug} 已卸载`)
    if (wasmExpanded.value === p.slug) wasmExpanded.value = ''
    await loadWasm()
  } catch (e: any) {
    toast.error('卸载失败: ' + e)
  }
}

async function wasmShowDetail(p: WasmPluginRow, tab: 'config' | 'logs') {
  if (wasmExpanded.value === p.slug && wasmDetailTab.value === tab) {
    wasmExpanded.value = ''
    return
  }
  wasmExpanded.value = p.slug
  wasmDetailTab.value = tab
  if (tab === 'config') {
    wasmConfig.value = null
    try {
      wasmConfig.value = await request('plugins', 'wasm.config.get', { slug: p.slug })
    } catch (e: any) {
      toast.error('读取配置失败: ' + e)
    }
  } else {
    wasmLogs.value = []
    try {
      const data = await request('plugins', 'wasm.logs', { slug: p.slug })
      wasmLogs.value = data?.lines || []
      wasmLogDropped.value = data?.dropped || 0
    } catch (e: any) {
      toast.error('读取日志失败: ' + e)
    }
  }
}

async function wasmConfigSave(slug: string, key: string, value: string) {
  try {
    await request('plugins', 'wasm.config.set', { slug, key, value })
    toast.success(`配置 ${key} 已保存（下次调用生效）`)
    wasmConfig.value = await request('plugins', 'wasm.config.get', { slug })
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

async function wasmConfigRemove(slug: string, key: string) {
  try {
    await request('plugins', 'wasm.config.set', { slug, key, remove: true })
    toast.success(`配置 ${key} 已清除`)
    wasmConfig.value = await request('plugins', 'wasm.config.get', { slug })
  } catch (e: any) {
    toast.error('清除失败: ' + e)
  }
}

function wasmTrustClass(trust: string): string {
  if (trust === 'trusted') return 'plugin-badge--ok'
  if (trust === 'review-recommended') return 'plugin-badge--warn'
  return 'plugin-badge--off'
}

onMounted(() => {
  loadPlugins()
  if (wasmFeatureEnabled) loadWasm()
})
</script>

<template>
  <div class="page-plugins">
    <div class="page-header" style="display: flex; justify-content: space-between; align-items: center;">
      <h2>插件</h2>
      <button class="btn btn-sm" @click="loadPlugins" :disabled="loading">重载</button>
    </div>
    <div class="page-body">
      <div class="tabs">
        <button
          v-for="tab in tabs"
          :key="tab.id"
          class="tab"
          :class="{ active: activeTab === tab.id }"
          @click="activeTab = tab.id"
        >{{ tab.label }}</button>
      </div>

      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <!-- ═══ Tab: WASM 插件（主角）═══ -->
      <div v-else-if="activeTab === 'wasm'" style="margin-top: var(--space-4);">
        <div v-if="wasmAvailable === false" class="card">
          <div class="card-body" style="color: var(--text-muted); font-size: var(--text-sm);">
            WASM 插件子系统未装配（未启用 plugins.wasm 或当前构建裁掉了 plugins-wasm feature）。
          </div>
        </div>
        <template v-else>
          <!-- 是什么 -->
          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header"><h3>WASM 插件是什么</h3></div>
            <div class="card-body" style="font-size: var(--text-sm); color: var(--text-secondary); line-height: 1.8;">
              <p style="margin: 0 0 var(--space-2);">
                用 Rust 写成、编译为<strong>单个 <code>.wasm</code> 文件</strong>的沙盒化插件——Windows / Linux / macOS
                三平台通用，无需按平台重新编译。运行在独立隔离环境里：默认<strong>零权限</strong>，能做什么完全由清单声明、宿主逐项授予。
              </p>
              <p style="margin: 0 0 var(--space-2);">安全边界（宿主强制，插件无法自行突破）：</p>
              <ul style="margin: 0 0 var(--space-2); padding-left: var(--space-5);">
                <li>出站网络 deny-by-default：只允许清单声明的域名（如 <code>api.example.com</code>）</li>
                <li>凭据经 vault 注入：插件拿到的是解锁后的值，原文不落 guest 日志、配置页不回显</li>
                <li>燃料 / 内存 / 墙钟三重限制：失控计算会被强制中断</li>
                <li>调用宿主能力（读写文件、发请求）一律过安全 8 层管线与审计链</li>
              </ul>
              <p style="margin: 0;">
                两种插件类型：<strong>Tool</strong>（给 agent 增加一个可调用的新工具）和
                <strong>Observer</strong>（订阅 agent 事件流，如记录活动、统计）。安装走九步装配漏斗
                （清单校验 → 信任验签 → 哈希对账 → 病毒扫描 → 审批卡确认），弹出审批卡后请在聊天面板确认。
              </p>
            </div>
          </div>

          <!-- 怎么做 + 开发包下载 -->
          <div class="card" style="margin-bottom: var(--space-4);">
            <div class="card-header"><h3>怎么开发一个插件</h3></div>
            <div class="card-body">
              <ol style="margin: 0 0 var(--space-4); padding-left: var(--space-5); font-size: var(--text-sm); color: var(--text-secondary); line-height: 2;">
                <li>点击下方按钮下载<strong>插件开发包</strong>（解压后即是可独立编译的最小 Rust 工程示例：打包工具 + 插件 SDK + 三个示例插件，与主程序仓库无依赖）</li>
                <li>在解压目录内执行 <code>cargo run -p pack</code>——自动编译全部示例插件、计算签名哈希、生成 <code>dist/&lt;插件名&gt;/</code> 安装目录</li>
                <li>把 <code>dist/&lt;插件名&gt;</code> 目录路径填到下方「安装」表单（或 CLI <code>nemesisbot plugin install &lt;目录&gt;</code>），审批通过即装即用</li>
              </ol>
              <div style="display: flex; align-items: center; gap: var(--space-3); flex-wrap: wrap;">
                <button class="btn btn-primary" :disabled="devkitBusy" @click="downloadDevkit">
                  {{ devkitBusy ? '下载中…' : '📦 下载插件开发包' }}
                </button>
                <label style="display: flex; align-items: center; gap: 4px; font-size: var(--text-xs); color: var(--text-secondary);">
                  <input v-model="devkitOverwrite" type="checkbox" :disabled="devkitBusy" />
                  已存在时覆盖重下
                </label>
              </div>
              <div v-if="devkitInfo" style="margin-top: var(--space-3); padding: var(--space-3); border: 1px solid var(--border-light); border-radius: var(--radius-md); background: var(--bg-secondary); font-size: var(--text-xs); line-height: 1.9;">
                <div>✅ 已就绪（{{ devkitInfo.files }} 个文件，{{ humanSize(devkitInfo.size) }}）——在终端里进入下方目录执行 <code>cargo run -p pack</code>：</div>
                <div style="font-family: var(--font-mono); word-break: break-all; color: var(--text-primary);">{{ devkitInfo.dir }}</div>
                <div style="color: var(--text-muted);">压缩包留存于 {{ devkitInfo.path }}</div>
              </div>
            </div>
          </div>

          <!-- 安装 + 已装列表 -->
          <div class="card">
            <div class="card-header"><h3>已安装（{{ wasmPlugins.length }}）</h3></div>
            <div class="card-body">
              <!-- 安装表单 -->
              <div style="display: flex; gap: var(--space-2); align-items: center; margin-bottom: var(--space-3); flex-wrap: wrap;">
                <input
                  v-model="wasmInstallDir"
                  class="form-input"
                  style="flex: 1; min-width: 260px;"
                  placeholder="插件源目录绝对路径（含 plugin.toml + wasm 载荷，如 devkit 的 dist/translate）"
                  :disabled="wasmInstalling"
                  @keyup.enter="wasmInstall"
                />
                <label style="display: flex; align-items: center; gap: 4px; font-size: var(--text-xs); color: var(--text-secondary); white-space: nowrap;">
                  <input v-model="wasmAllowUnsigned" type="checkbox" :disabled="wasmInstalling" />
                  允许无签名
                </label>
                <button class="btn btn-sm btn-primary" :disabled="wasmInstalling" @click="wasmInstall">
                  {{ wasmInstalling ? '安装中…' : '安装' }}
                </button>
              </div>
              <!-- #11（2026-09-30 插件体系复查）：升级语义说明——重装同 slug 即升级 -->
              <div style="color: var(--text-muted); font-size: var(--text-xs); margin-bottom: var(--space-3);">
                重新安装同 slug 的插件目录即为升级：漏斗重跑（验签/审批/装载），版本与载荷替换，<strong>插件数据目录保留</strong>；无需先卸载。
              </div>
              <!-- 插件行 -->
              <div v-if="!wasmPlugins.length" style="color: var(--text-muted); font-size: var(--text-sm);">尚未安装任何 WASM 插件——按上方三步下载开发包即可开始</div>
              <div v-for="p in wasmPlugins" :key="p.slug" class="plugin-card">
                <div style="display: flex; justify-content: space-between; align-items: center; gap: var(--space-2);">
                  <div style="display: flex; align-items: center; gap: var(--space-2); flex-wrap: wrap;">
                    <span style="font-weight: 600; font-family: var(--font-mono);">{{ p.slug }}</span>
                    <span class="plugin-badge">v{{ p.version }}</span>
                    <span class="plugin-badge">{{ p.kind }}</span>
                    <span class="plugin-badge" :class="wasmTrustClass(p.trust)">{{ p.trust }}</span>
                    <span v-if="!p.loaded" class="plugin-badge" style="color: var(--danger); border-color: var(--danger);">
                      未装载（重启后生效或装载失败）
                    </span>
                  </div>
                  <div style="display: flex; align-items: center; gap: var(--space-2);">
                    <template v-if="p.loaded">
                      <button class="btn btn-sm" @click="wasmShowDetail(p, 'config')">配置</button>
                      <button class="btn btn-sm" @click="wasmShowDetail(p, 'logs')">日志</button>
                      <label class="toggle-switch">
                        <input type="checkbox" :checked="p.enabled" @change="wasmToggle(p)" />
                        <span class="toggle-slider"></span>
                      </label>
                    </template>
                    <button class="btn btn-sm btn-danger" @click="wasmUninstall(p)">卸载</button>
                  </div>
                </div>
                <div v-if="p.name" style="margin-top: var(--space-1); color: var(--text-secondary); font-size: var(--text-sm);">{{ p.name }}</div>
                <div v-if="p.tool" style="margin-top: var(--space-1); font-size: var(--text-xs); color: var(--text-secondary);">
                  工具 <code>{{ p.tool.name }}</code>（声明 {{ p.tool.operation_type || '未声明(按只读基线)' }}，
                  min-tier {{ p.tool.min_tier || '未设' }}）—— {{ p.tool.description }}
                </div>
                <div v-if="p.egress?.length || p.x_secret?.length" style="margin-top: var(--space-2); display: flex; gap: var(--space-1); flex-wrap: wrap;">
                  <span v-for="d in p.egress" :key="'e' + d" class="plugin-badge" style="color: var(--warning);">出站: {{ d }}</span>
                  <span v-for="s in p.x_secret" :key="'s' + s" class="plugin-badge" style="color: var(--danger);">凭据: {{ s }}</span>
                </div>
                <!-- 展开详情 -->
                <div v-if="wasmExpanded === p.slug" style="margin-top: var(--space-3); border-top: 1px solid var(--border-light); padding-top: var(--space-3);">
                  <template v-if="wasmDetailTab === 'config'">
                    <div v-if="wasmConfig">
                      <div v-if="wasmConfig.secret_keys?.length" style="font-size: var(--text-xs); color: var(--text-muted); margin-bottom: var(--space-2);">
                        凭据键（{{ wasmConfig.secret_keys.join(', ') }}）经 vault 注入，此处不显示原文。
                      </div>
                      <div v-if="!Object.keys(wasmConfig.entries).length && !wasmConfig.schema_keys?.length" style="color: var(--text-muted); font-size: var(--text-sm);">
                        该插件未声明配置键。
                      </div>
                      <div v-for="k in wasmConfig.schema_keys" :key="k" style="display: flex; gap: var(--space-2); align-items: center; margin-bottom: var(--space-2);">
                        <span style="font-family: var(--font-mono); font-size: var(--text-xs); min-width: 120px;">{{ k }}</span>
                        <input
                          class="form-input"
                          style="flex: 1;"
                          :value="wasmConfig.entries[k] ?? ''"
                          :placeholder="'未设置'"
                          @change="wasmConfigSave(p.slug, k, ($event.target as HTMLInputElement).value)"
                        />
                        <button class="btn btn-sm" @click="wasmConfigRemove(p.slug, k)">清除</button>
                      </div>
                    </div>
                  </template>
                  <template v-else>
                    <div v-if="wasmLogDropped > 0" style="font-size: var(--text-xs); color: var(--warning); margin-bottom: var(--space-2);">
                      已丢弃 {{ wasmLogDropped }} 条（环形缓冲满）
                    </div>
                    <div v-if="!wasmLogs.length" style="color: var(--text-muted); font-size: var(--text-sm);">暂无宿主日志</div>
                    <div v-for="(l, i) in wasmLogs" :key="i" style="font-family: var(--font-mono); font-size: var(--text-xs); padding: 1px 0; word-break: break-all;">
                      <span style="color: var(--text-muted);">{{ new Date(l.ts_ms).toLocaleTimeString() }}</span>
                      <span :style="{ color: l.level === 'error' ? 'var(--danger)' : l.level === 'warn' ? 'var(--warning)' : 'var(--text-secondary)' }">
                        [{{ l.level }}]
                      </span>
                      {{ l.message }}
                    </div>
                  </template>
                </div>
              </div>
            </div>
          </div>
        </template>
      </div>

      <!-- ═══ Tab: 本地插件库（C-ABI，只读）═══ -->
      <div v-else-if="activeTab === 'native'" style="margin-top: var(--space-4);">
        <div class="card">
          <div class="card-header"><h3>本地插件库（{{ plugins.filter(p => p.found).length }}/{{ plugins.length }} 已就绪）</h3></div>
          <div class="card-body">
            <p style="font-size: var(--text-xs); color: var(--text-muted); margin: 0 0 var(--space-2);">
              原生动态库（.dll / .so），位于运行目录旁的 <code>plugins/</code> 子目录，与主程序同步分发。
              页面为只读总览；插件文件放对位置后点「重载」即可识别。新扩展请优先使用 WASM 插件（沙盒隔离 + 跨平台）。
            </p>
            <div v-for="p in plugins" :key="p.id" class="plugin-card">
              <div style="display: flex; justify-content: space-between; align-items: center;">
                <div style="display: flex; align-items: center; gap: var(--space-2);">
                  <span :style="{ color: p.found ? 'var(--success)' : 'var(--text-muted)' }" style="font-size: 18px;">{{ p.found ? '●' : '○' }}</span>
                  <span style="font-weight: 600; font-family: var(--font-mono);">{{ p.id }}</span>
                  <span class="plugin-badge" :class="p.found ? 'plugin-badge--ok' : 'plugin-badge--off'">
                    {{ p.found ? '已就绪' : '未找到' }}
                  </span>
                </div>
                <span class="plugin-filename">{{ p.filename }}</span>
              </div>
              <div style="margin-top: var(--space-1); color: var(--text-secondary); font-size: var(--text-sm);">
                {{ p.label }} —— 服务于{{ p.used_by }}
              </div>
              <div v-if="p.path" style="color: var(--text-muted); font-size: var(--text-xs); margin-top: 2px; word-break: break-all;">{{ p.path }}</div>
              <div v-if="p.capabilities?.length" style="margin-top: var(--space-2); display: flex; gap: var(--space-1); flex-wrap: wrap;">
                <span v-for="cap in p.capabilities" :key="cap" class="plugin-badge">{{ cap }}</span>
              </div>
              <div v-if="p.detail" style="margin-top: var(--space-2); font-size: var(--text-xs); color: var(--text-secondary);">
                <template v-if="p.detail.note">{{ p.detail.note }}</template>
                <template v-else>
                  强化记忆：{{ p.detail.enhanced_memory_enabled ? '已启用' : '未启用' }}
                  · 当前档 {{ p.detail.active_tier }}
                  · 模型 {{ p.detail.active_model || '?' }}
                  ·
                  <span :style="{ color: p.detail.model_ready ? 'var(--success)' : 'var(--danger)' }">
                    {{ p.detail.model_ready ? '模型就绪' : '模型未安装' }}
                  </span>
                  （可在「记忆」页环境准备卡安装）
                </template>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- ═══ Tab: 管线插件 ═══ -->
      <div v-else style="margin-top: var(--space-4);">
        <div class="card">
          <div class="card-header"><h3>管线插件</h3></div>
          <div class="card-body">
            <p style="font-size: var(--text-xs); color: var(--text-muted); margin: 0 0 var(--space-2);">
              工具管线三段化（pre / around / post）的进程内插件——启停即时生效，无泄漏（Guard 注销）。
            </p>
            <div v-if="!pipelinePlugins.length" style="color: var(--text-muted); font-size: var(--text-sm);">暂无注册的管线插件</div>
            <div v-for="p in pipelinePlugins" :key="p.name" style="display: flex; justify-content: space-between; align-items: center; padding: var(--space-2) 0;">
              <div>
                <span style="font-family: var(--font-mono); font-weight: 600; font-size: var(--text-sm);">{{ p.name }}</span>
                <span style="color: var(--text-muted); font-size: var(--text-xs); margin-left: var(--space-2);">{{ p.description }}</span>
              </div>
              <label class="toggle-switch">
                <input type="checkbox" :checked="p.enabled" @change="togglePipeline(p)" />
                <span class="toggle-slider"></span>
              </label>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.plugin-card {
  padding: var(--space-3);
  border: 1px solid var(--border-light);
  border-radius: var(--radius-md);
  margin-bottom: var(--space-3);
}
.plugin-badge {
  display: inline-block;
  padding: 1px 8px;
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  border: 1px solid var(--border-light);
  background: var(--bg-secondary);
  color: var(--text-secondary);
}
.plugin-badge--ok {
  color: var(--success);
  border-color: var(--success);
}
.plugin-badge--warn {
  color: var(--warning);
  border-color: var(--warning);
}
.plugin-badge--off {
  color: var(--text-muted);
}
.plugin-filename {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--text-muted);
}
</style>
