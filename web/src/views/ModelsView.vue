<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

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
// Backend add expects: name, model, key, base_url?, proxy?
const addForm = ref({ name: '', model: '', key: '', base_url: '', proxy: '' })
const testing = ref<string | null>(null)
const switching = ref<string | null>(null)

async function loadModels() {
  try {
    const data = await request('models', 'list')
    models.value = data?.models || []
  } catch (e: any) {
    toast.error('加载模型失败: ' + e)
  }
  loading.value = false
}

async function addModel() {
  if (!addForm.value.name) { toast.warn('请输入模型名称'); return }
  if (!addForm.value.model) { toast.warn('请输入模型 ID'); return }
  if (!addForm.value.key) { toast.warn('请输入 API Key'); return }
  try {
    const payload: any = {
      name: addForm.value.name,
      model: addForm.value.model,
      key: addForm.value.key,
    }
    if (addForm.value.base_url) payload.base_url = addForm.value.base_url
    if (addForm.value.proxy) payload.proxy = addForm.value.proxy
    await request('models', 'add', payload)
    toast.success('模型已添加')
    showAdd.value = false
    addForm.value = { name: '', model: '', key: '', base_url: '', proxy: '' }
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
        <button class="btn btn-primary" @click="showAdd = !showAdd">{{ showAdd ? '取消' : '+ 添加模型' }}</button>
      </div>
    </div>
    <div class="page-body">
      <!-- Add form -->
      <div v-if="showAdd" class="card" style="margin-bottom: var(--space-4);">
        <div class="card-header"><h3>添加模型</h3></div>
        <div class="card-body">
          <div style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-3);">
            <div class="form-group">
              <label class="form-label">名称 *（显示名称）</label>
              <input class="form-input" v-model="addForm.name" placeholder="例如: 我的GPT4">
            </div>
            <div class="form-group">
              <label class="form-label">模型 ID *（实际调用）</label>
              <input class="form-input" v-model="addForm.model" placeholder="例如: gpt-4o / zhipu/glm-4">
            </div>
            <div class="form-group">
              <label class="form-label">API Key *</label>
              <input class="form-input" type="password" v-model="addForm.key" placeholder="sk-...">
            </div>
            <div class="form-group">
              <label class="form-label">Base URL</label>
              <input class="form-input" v-model="addForm.base_url" placeholder="https://api.openai.com/v1">
            </div>
          </div>
          <div class="form-group" style="margin-top: var(--space-3);">
            <label class="form-label">代理</label>
            <input class="form-input" v-model="addForm.proxy" placeholder="http://proxy:port" style="max-width: 300px;">
          </div>
          <div style="margin-top: var(--space-3); display: flex; justify-content: flex-end; gap: var(--space-2);">
            <button class="btn" @click="showAdd = false">取消</button>
            <button class="btn btn-primary" @click="addModel">添加</button>
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
</style>
