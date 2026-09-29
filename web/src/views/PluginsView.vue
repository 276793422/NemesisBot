<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

// 插件状态总览页（2026-08-29 phase 1，只读）：枚举已知插件库
// （探测 exe 旁 plugins/）与当前构建的子系统 feature 状态。
// 数据源 = plugins.list WSAPI（handlers/plugins.rs）。
// W6（2026-09-29）：新增「WASM 插件」分节（plugins.wasm.* WSAPI）——
// 沙盒化组件插件的管理面（安装走九步装配漏斗 + 审批卡，启停热生效）。

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

interface FeatureEntry {
  id: string
  label: string
  enabled: boolean
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

const { request } = useWSAPI()
const toast = useToast()

const plugins = ref<PluginEntry[]>([])
const features = ref<FeatureEntry[]>([])
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

const enabledFeatureCount = computed(() => features.value.filter(f => f.enabled).length)

// 前端同步裁剪（customize 流程）：VITE_FEATURE_PLUGINS_WASM=false 时整节
// 隐藏（构建期 tree-shake 由路由级门控承担；页内节用运行时常量）。
const wasmFeatureEnabled = import.meta.env.VITE_FEATURE_PLUGINS_WASM !== 'false'

async function loadPlugins() {
  loading.value = true
  try {
    const data = await request('plugins', 'list')
    plugins.value = data?.plugins || []
    features.value = data?.features || []
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
  } catch (e: any) {
    wasmAvailable.value = false
  }
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
  loadWasm()
})
</script>

<template>
  <div class="page-plugins">
    <div class="page-header" style="display: flex; justify-content: space-between; align-items: center;">
      <h2>插件</h2>
      <button class="btn btn-sm" @click="loadPlugins" :disabled="loading">重载</button>
    </div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-else>
        <p style="font-size: var(--text-sm); color: var(--text-secondary); margin: 0 0 var(--space-4);">
          插件库位于运行目录旁的 <code>plugins/</code> 子目录，为宿主提供可选能力
          （嵌入推理、WebView UI 等）。页面为只读总览；插件文件放对位置后点「重载」即可识别。
        </p>

        <!-- 插件卡片 -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>插件库（{{ plugins.filter(p => p.found).length }}/{{ plugins.length }} 已就绪）</h3></div>
          <div class="card-body">
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

        <!-- WASM 插件（沙盒化组件插件；plugins.wasm.* WSAPI；前端裁剪门控） -->
        <div v-if="wasmFeatureEnabled" class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>WASM 插件（沙盒化，{{ wasmPlugins.length }}）</h3></div>
          <div class="card-body">
            <div v-if="wasmAvailable === false" style="color: var(--text-muted); font-size: var(--text-sm);">
              WASM 插件子系统未装配（未启用 plugins.wasm 或当前构建裁掉了 plugins-wasm feature）。
            </div>
            <template v-else>
              <p style="font-size: var(--text-xs); color: var(--text-muted); margin: 0 0 var(--space-2);">
                WebAssembly 组件插件：guest 侧能力受 fuel/内存/墙钟三重限制，出站网络 deny-by-default，
                凭据走 vault 注入（原文不进 guest 日志）。安装走九步装配漏斗（验签 → 病毒扫描 → 审批卡），
                弹出审批卡后请在聊天面板确认。
              </p>
              <!-- 安装表单 -->
              <div style="display: flex; gap: var(--space-2); align-items: center; margin-bottom: var(--space-3); flex-wrap: wrap;">
                <input
                  v-model="wasmInstallDir"
                  class="form-input"
                  style="flex: 1; min-width: 260px;"
                  placeholder="插件源目录绝对路径（含 plugin.toml + wasm 载荷）"
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
              <!-- 插件行 -->
              <div v-if="!wasmPlugins.length" style="color: var(--text-muted); font-size: var(--text-sm);">尚未安装任何 WASM 插件</div>
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
            </template>
          </div>
        </div>

        <!-- 管线插件（T2 三段化的进程内插件，可启停） -->
        <div class="card" style="margin-bottom: var(--space-4);">
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

        <!-- 编译期 feature 状态 -->
        <div class="card">
          <div class="card-header"><h3>子系统 feature（编译期，{{ enabledFeatureCount }}/{{ features.length }} 开启）</h3></div>
          <div class="card-body">
            <p style="font-size: var(--text-xs); color: var(--text-muted); margin: 0 0 var(--space-2);">
              由构建时的 cargo feature 决定（customize / menuconfig 裁剪），变更需重新构建。
            </p>
            <div style="display: flex; gap: var(--space-2); flex-wrap: wrap;">
              <span v-for="f in features" :key="f.id" class="plugin-badge" :class="f.enabled ? 'plugin-badge--ok' : 'plugin-badge--off'">
                {{ f.label }} · {{ f.enabled ? '开' : '关' }}
              </span>
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
