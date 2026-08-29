<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

// 插件状态总览页（2026-08-29 phase 1，只读）：枚举已知插件库
// （探测 exe 旁 plugins/）与当前构建的子系统 feature 状态。
// 数据源 = plugins.list WSAPI（handlers/plugins.rs）。
// 安装/启停（需要 effect/disposer 注册机制）为后续扩展，见
// docs/PLAN/2026-08-29_plugins-page-goal.md。

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

const { request } = useWSAPI()
const toast = useToast()

const plugins = ref<PluginEntry[]>([])
const features = ref<FeatureEntry[]>([])
const pipelinePlugins = ref<PipelinePlugin[]>([])
const loading = ref(true)

const enabledFeatureCount = computed(() => features.value.filter(f => f.enabled).length)

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

onMounted(loadPlugins)
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
.plugin-badge--off {
  color: var(--text-muted);
}
.plugin-filename {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--text-muted);
}
</style>
