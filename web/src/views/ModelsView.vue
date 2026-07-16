<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { usePageTab } from '../lib/pageTab'
import LocalModelsView from './LocalModelsView.vue'
import { PROVIDER_PRESETS, findProvider } from '../lib/providerPresets'

const { request } = useWSAPI()
const toast = useToast()
const pageTab = ref('cloud')
const { setTab } = usePageTab(pageTab, ['cloud', 'local'] as const, 'cloud')

// Backend returns: model_name, model, api_base, api_key (masked), proxy, is_default
interface Model {
  model_name: string
  model: string
  api_base?: string
  api_key?: string
  proxy?: string
  is_default?: boolean
}

const models = ref<Model[]>([])
const loading = ref(true)
const showAdd = ref(false)
const providerId = ref(PROVIDER_PRESETS[0]?.id || 'openai')
const modelChoice = ref('')
const apiKey = ref('')
const customModelId = ref('')
const customBaseUrl = ref('')
const customName = ref('')
const testing = ref<string | null>(null)
const switching = ref<string | null>(null)

const currentProvider = computed(() => findProvider(providerId.value) || PROVIDER_PRESETS[0])
const isCustom = computed(() => providerId.value === 'custom')

watch(providerId, (id) => {
  const p = findProvider(id)
  if (p && p.models.length) modelChoice.value = p.models[0].id
  else modelChoice.value = ''
}, { immediate: true })

async function loadModels() {
  try {
    const data = await request('models', 'list')
    models.value = data?.models || []
  } catch (e: any) {
    toast.error('加载模型失败: ' + e)
  }
  loading.value = false
}

function resetAddWizard() {
  providerId.value = PROVIDER_PRESETS[0]?.id || 'openai'
  apiKey.value = ''
  customModelId.value = ''
  customBaseUrl.value = ''
  customName.value = ''
  const p = findProvider(providerId.value)
  modelChoice.value = p?.models[0]?.id || ''
}

async function addModel() {
  const p = currentProvider.value
  if (!p) return
  if (!apiKey.value.trim()) { toast.warn('请粘贴 API Key'); return }

  let name = ''
  let model = ''
  let base_url = p.baseUrl

  if (isCustom.value) {
    if (!customModelId.value.trim()) { toast.warn('请填写模型 ID'); return }
    if (!customBaseUrl.value.trim()) { toast.warn('请填写接口地址'); return }
    model = customModelId.value.trim()
    base_url = customBaseUrl.value.trim()
    name = customName.value.trim() || model
  } else {
    if (!modelChoice.value) { toast.warn('请选择模型'); return }
    model = modelChoice.value
    const choice = p.models.find((m) => m.id === model)
    name = `${p.namePrefix} · ${choice?.label || model}`
  }

  try {
    const payload: any = { name, model, key: apiKey.value.trim() }
    if (base_url) payload.base_url = base_url
    await request('models', 'add', payload)
    toast.success('模型已添加')
    showAdd.value = false
    resetAddWizard()
    await loadModels()
  } catch (e: any) {
    toast.error('添加失败: ' + e)
  }
}

async function deleteModel(name: string) {
  if (!confirm(`确定删除模型 "${name}" 吗？`)) return
  try {
    await request('models', 'delete', { name })
    toast.success('已删除')
    await loadModels()
  } catch (e: any) {
    toast.error('删除失败: ' + e)
  }
}

async function setDefault(name: string) {
  const prev = switching.value
  switching.value = name
  try {
    await request('models', 'set_default', { name })
    toast.success(`${name} 已设为默认模型，立即生效`)
    await loadModels()
  } catch (e: any) {
    toast.error('设置失败: ' + e)
  }
  switching.value = null
}

async function testModel(name: string) {
  testing.value = name
  try {
    const data = await request('models', 'test', { name })
    if (data?.status === 'not_implemented') {
      toast.info('模型测试功能尚未实现')
    } else {
      toast.success(data?.message || '测试通过')
    }
  } catch (e: any) {
    toast.error('测试失败: ' + e)
  }
  testing.value = null
}

onMounted(loadModels)
</script>

<template>
  <div class="page-models">
    <div class="page-header">
      <h2>模型管理</h2>
      <div class="page-header-actions">
        <button v-if="pageTab === 'cloud'" class="btn btn-primary" @click="showAdd = !showAdd">{{ showAdd ? '取消' : '+ 添加模型' }}</button>
      </div>
    </div>
    <div class="page-body">
      <div class="tabs" style="margin-bottom: var(--space-4);">
        <button class="tab" :class="{ active: pageTab === 'cloud' }" @click="setTab('cloud')">云端 / API</button>
        <button class="tab" :class="{ active: pageTab === 'local' }" @click="setTab('local')">本地模型</button>
      </div>

      <div v-if="pageTab === 'local'">
        <LocalModelsView embedded />
      </div>

      <template v-if="pageTab === 'cloud'">
      <!-- Add wizard: provider → model → key -->
      <div v-if="showAdd" class="card" style="margin-bottom: var(--space-4);">
        <div class="card-header"><h3>添加云端模型</h3></div>
        <div class="card-body">
          <p class="form-hint" style="margin-bottom: var(--space-4);">选择服务商并粘贴密钥即可，无需手写 Base URL / 模型 ID。</p>
          <div class="form-group">
            <label class="form-label">1. 服务商</label>
            <div class="preset-grid">
              <button
                v-for="p in PROVIDER_PRESETS"
                :key="p.id"
                type="button"
                class="preset-chip"
                :class="{ active: providerId === p.id }"
                @click="providerId = p.id"
              >{{ p.label }}</button>
            </div>
          </div>
          <div v-if="!isCustom" class="form-group">
            <label class="form-label">2. 模型</label>
            <select class="form-select" v-model="modelChoice" style="max-width: 360px;">
              <option v-for="m in currentProvider?.models || []" :key="m.id" :value="m.id">{{ m.label }}</option>
            </select>
          </div>
          <template v-else>
            <div class="form-group">
              <label class="form-label">显示名称（可选）</label>
              <input class="form-input" v-model="customName" placeholder="我的模型" style="max-width: 360px;">
            </div>
            <div class="form-group">
              <label class="form-label">模型 ID</label>
              <input class="form-input" v-model="customModelId" placeholder="provider/model-name" style="max-width: 360px;">
            </div>
            <div class="form-group">
              <label class="form-label">接口地址</label>
              <input class="form-input" v-model="customBaseUrl" placeholder="https://…/v1" style="max-width: 360px;">
            </div>
          </template>
          <div class="form-group">
            <label class="form-label">{{ isCustom ? '3' : '3' }}. API Key</label>
            <input class="form-input" type="password" v-model="apiKey" :placeholder="currentProvider?.keyHint || '粘贴密钥'" style="max-width: 360px;" autocomplete="off">
          </div>
          <div style="margin-top: var(--space-4); display: flex; justify-content: flex-end; gap: var(--space-2);">
            <button class="btn" @click="showAdd = false; resetAddWizard()">取消</button>
            <button class="btn btn-primary" @click="addModel">添加并使用</button>
          </div>
        </div>
      </div>

      <!-- Loading -->
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <!-- Empty -->
      <div v-if="!loading && models.length === 0" class="empty-state">
        <h3>暂无模型</h3>
        <p>点击上方"添加模型"按钮配置第一个 AI 模型</p>
      </div>

      <!-- Model list -->
      <div v-if="!loading && models.length > 0" style="display: grid; grid-template-columns: repeat(auto-fill, minmax(340px, 1fr)); gap: var(--space-4);">
        <div
          v-for="m in models"
          :key="m.model_name"
          class="card model-card"
          :class="{ 'model-card--default': m.is_default, 'model-card--switching': switching === m.model_name }"
        >
          <div class="card-header">
            <h3>{{ m.model_name }}</h3>
            <div style="display: flex; gap: var(--space-2); align-items: center;">
              <span v-if="m.is_default" class="badge badge-success">&#10003; 默认</span>
              <span v-if="m.model" class="badge badge-info">{{ m.model }}</span>
            </div>
          </div>
          <div class="card-body">
            <div class="settings-grid" style="font-size: var(--text-sm);">
              <span class="settings-key">API Key</span>
              <span class="settings-value">{{ m.api_key || '--' }}</span>
              <span class="settings-key">Base URL</span>
              <span class="settings-value">{{ m.api_base || '--' }}</span>
              <span class="settings-key">代理</span>
              <span class="settings-value">{{ m.proxy || '--' }}</span>
            </div>
          </div>
          <div class="card-footer">
            <button class="btn btn-sm btn-ghost" @click="testModel(m.model_name)" :disabled="testing === m.model_name">
              <span v-if="testing === m.model_name" class="spinner" style="width:14px;height:14px;"></span>
              {{ testing === m.model_name ? '测试中...' : '测试' }}
            </button>
            <button
              v-if="!m.is_default"
              class="btn btn-sm btn-primary"
              @click="setDefault(m.model_name)"
              :disabled="switching !== null"
            >
              <span v-if="switching === m.model_name" class="spinner" style="width:14px;height:14px;"></span>
              {{ switching === m.model_name ? '切换中...' : '设为默认' }}
            </button>
            <span v-else class="model-active-label">当前使用中</span>
            <button class="btn btn-sm btn-danger" @click="deleteModel(m.model_name)" :disabled="switching !== null">删除</button>
          </div>
        </div>
      </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.model-card {
  transition: border-color 0.25s, box-shadow 0.25s, background-color 0.25s;
}

.model-card--default {
  border-color: var(--color-success, #22c55e);
  box-shadow: 0 0 0 1px var(--color-success, #22c55e), 0 2px 8px rgba(34, 197, 94, 0.15);
}

:root[data-theme='dark'] .model-card--default {
  box-shadow: 0 0 0 1px var(--color-success, #22c55e), 0 2px 12px rgba(34, 197, 94, 0.25);
}

.model-card--switching {
  opacity: 0.7;
  pointer-events: none;
}

.model-active-label {
  font-size: var(--text-sm, 13px);
  color: var(--color-success, #22c55e);
  font-weight: 500;
}
.preset-grid {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
}
.preset-chip {
  border: 1px solid var(--border);
  background: var(--surface);
  color: var(--text-secondary);
  border-radius: var(--radius-full);
  padding: 6px 14px;
  font-size: var(--text-sm);
  cursor: pointer;
}
.preset-chip.active {
  border-color: var(--accent);
  color: var(--accent);
  background: var(--accent-muted);
}
.form-select {
  width: 100%;
  padding: var(--space-2) var(--space-3);
  border-radius: var(--radius-md);
  border: 1px solid var(--border);
  background: var(--surface);
  color: var(--text);
  font: inherit;
}
</style>
