<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

// Backend returns: model_name, model, api_base, api_key (masked), proxy, is_default
// + P3-2 raw extras (null = 未设置) + catalog_match (exact-key hit, nullable)
interface CatalogMatch {
  context_window?: number
  max_output_tokens?: number
  family?: string
}
interface Model {
  model_name: string
  model: string
  api_base?: string
  api_key?: string
  proxy?: string
  is_default?: boolean
  model_tier?: string | null
  reasoning_effort?: string | null
  model_size_b?: number | string | null
  real_name?: string | null
  context_window?: number | string | null
  catalog_match?: CatalogMatch | null
}

const models = ref<Model[]>([])
const loading = ref(true)
const showAdd = ref(false)
// Backend add expects: name, model, key, base_url?, proxy?
const addForm = ref({ name: '', model: '', key: '', base_url: '', proxy: '' })
const testing = ref<string | null>(null)
const switching = ref<string | null>(null)

// P3-2: attribute editor — which cards are expanded + per-card drafts.
const expandedAttrs = ref<Set<string>>(new Set())
interface AttrDraft {
  tier: string
  effort: string // '' = off（不发送）
  size: string
  realName: string
  ctx: string
}
const attrDrafts = ref<Record<string, AttrDraft>>({})

// P3-2: models.dev catalog cache state + refresh busy flag.
interface CatalogInfo { exists: boolean; fetched_at: string; entries: number }
const catalogInfo = ref<CatalogInfo | null>(null)
const catalogUpdating = ref(false)

async function loadModels() {
  try {
    const data = await request('models', 'list')
    models.value = data?.models || []
  } catch (e: any) {
    toast.error('加载模型失败: ' + e)
  }
  loading.value = false
}

async function loadCatalogInfo() {
  try {
    catalogInfo.value = await request('models', 'catalog_info')
  } catch {
    catalogInfo.value = { exists: false, fetched_at: '', entries: 0 }
  }
}

/** P3-2: 拉取 models.dev 目录（后端 spawn CLI `model catalog-update`，90s 超时）。 */
async function updateCatalog() {
  if (catalogUpdating.value) return
  catalogUpdating.value = true
  try {
    catalogInfo.value = await request('models', 'catalog_update')
    toast.success(`模型目录已更新（${catalogInfo.value?.entries ?? 0} 条）`)
    await loadModels() // catalog_match per model may have changed
  } catch (e: any) {
    toast.error('目录更新失败: ' + e)
  }
  catalogUpdating.value = false
}

function toggleAttrs(m: Model) {
  const s = new Set(expandedAttrs.value)
  if (s.has(m.model_name)) {
    s.delete(m.model_name)
  } else {
    s.add(m.model_name)
    // Seed the draft from current values (null → empty/off).
    attrDrafts.value[m.model_name] = {
      tier: m.model_tier || 'auto',
      effort: m.reasoning_effort || '',
      size: m.model_size_b != null ? String(m.model_size_b) : '',
      realName: m.real_name || '',
      ctx: m.context_window != null ? String(m.context_window) : '',
    }
  }
  expandedAttrs.value = s
}

/** Fields whose draft differs from the loaded values (only these are saved). */
function dirtyFields(m: Model): string[] {
  const d = attrDrafts.value[m.model_name]
  if (!d) return []
  const out: string[] = []
  if ((m.model_tier || 'auto') !== d.tier) out.push('model_tier')
  if ((m.reasoning_effort || '') !== d.effort) out.push('reasoning_effort')
  const sizeStr = m.model_size_b != null ? String(m.model_size_b) : ''
  if (sizeStr !== d.size.trim()) out.push('model_size_b')
  if ((m.real_name || '') !== d.realName.trim()) out.push('real_name')
  const ctxStr = m.context_window != null ? String(m.context_window) : ''
  if (ctxStr !== d.ctx.trim()) out.push('context_window')
  return out
}

/** 生效方式标注（对码结论）：tier/effort 走 agent 每轮 LLM 开头的
 * check_config_reload 重读（改完即时生效）；size/real_name 参与 auto 档
 * 检测、同链路生效；显式 tier 下 size/real_name 不参与。 */
const FIELD_EFFECT: Record<string, string> = {
  model_tier: '即时生效（下一次 LLM 轮自动重读配置）',
  reasoning_effort: '即时生效（下次 LLM 调用前重读配置）',
  model_size_b: 'auto 档下即时生效（参与能力自动检测）',
  real_name: 'auto 档下即时生效（别名识别真名）',
  context_window: '保存后生效（上下文预算依据）',
}

/** P3-2: 保存属性 — 逐字段走后端 raw-JSON RMW（保留 config.json 其余键）。 */
async function saveAttrs(m: Model) {
  const d = attrDrafts.value[m.model_name]
  if (!d) return
  const fields = dirtyFields(m)
  if (fields.length === 0) {
    toast.info('没有修改过的属性')
    return
  }
  const values: Record<string, unknown> = {
    model_tier: d.tier,
    // ''（off）由后端归一为空串 = 不发送
    reasoning_effort: d.effort || 'off',
    model_size_b: d.size.trim() === '' ? null : Number(d.size.trim()),
    real_name: d.realName.trim(),
    context_window: d.ctx.trim() === '' ? null : Number(d.ctx.trim()),
  }
  // v1 不支持写 null：清空场景跳过该字段，保存名单只含真正落盘的字段，
  // toast 如实反映（不能宣称保存了实际没写的字段）。
  const skipped: string[] = []
  for (const f of fields) {
    if (values[f] == null) { skipped.push(f); continue } // 清空场景：v1 不支持写 null，留原值
    try {
      await request('models', 'update_field', { name: m.model_name, field: f, value: values[f] })
    } catch (e: any) {
      toast.error(`${f} 保存失败: ` + e)
      await loadModels()
      return
    }
  }
  const saved = fields.filter((f) => !skipped.includes(f))
  if (saved.length > 0) toast.success(`已保存：${saved.join('、')}`)
  if (skipped.length > 0) {
    toast.info(`未保存（v1 暂不支持清空，保留原值）：${skipped.join('、')}`)
  }
  await loadModels()
  // Re-seed the draft from the refreshed values so the dirty diff resets.
  const fresh = models.value.find((x) => x.model_name === m.model_name)
  if (fresh) {
    attrDrafts.value[m.model_name] = {
      tier: fresh.model_tier || 'auto',
      effort: fresh.reasoning_effort || '',
      size: fresh.model_size_b != null ? String(fresh.model_size_b) : '',
      realName: fresh.real_name || '',
      ctx: fresh.context_window != null ? String(fresh.context_window) : '',
    }
  }
}

/** P3-2: context_window 一键填入 models.dev 目录值。 */
function fillCtxFromCatalog(m: Model) {
  const cw = m.catalog_match?.context_window
  if (cw == null) return
  const d = attrDrafts.value[m.model_name]
  if (d) d.ctx = String(cw)
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

onMounted(() => {
  loadModels()
  loadCatalogInfo()
})
</script>

<template>
  <div class="page-models">
    <div class="page-header">
      <h2>模型管理</h2>
      <div class="page-header-actions">
        <!-- P3-2: models.dev 目录缓存状态 + 一键刷新（后端 spawn CLI，90s 超时） -->
        <span v-if="catalogInfo" class="catalog-hint">
          {{ catalogInfo.exists
              ? `目录缓存：${catalogInfo.entries} 条 · ${catalogInfo.fetched_at || '时间未知'}`
              : '目录未拉取（context_window 自动填充需先更新目录）' }}
        </span>
        <button class="btn" :disabled="catalogUpdating" @click="updateCatalog">
          <span v-if="catalogUpdating" class="spinner" style="width:14px;height:14px;"></span>
          {{ catalogUpdating ? '拉取中…' : '更新模型目录' }}
        </button>
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
              <span class="settings-key">能力档</span>
              <span class="settings-value">{{ m.model_tier || 'auto（自动检测）' }}</span>
            </div>
          </div>
          <!-- P3-2: 属性编辑展开区（tier / effort / 参数量 / 真名 / context_window） -->
          <div v-if="expandedAttrs.has(m.model_name) && attrDrafts[m.model_name]" class="attr-editor">
            <div class="attr-grid">
              <div class="attr-field">
                <label class="form-label">能力档 tier</label>
                <select class="form-input" v-model="attrDrafts[m.model_name].tier">
                  <option value="auto">auto（自动检测）</option>
                  <option value="mini">mini（小模型 · 核心 13 工具）</option>
                  <option value="normal">normal（中模型 · ~26 工具）</option>
                  <option value="big">big（大模型 · 全量 42 工具）</option>
                </select>
                <span class="attr-effect">{{ FIELD_EFFECT['model_tier'] }}</span>
              </div>
              <div class="attr-field">
                <label class="form-label">推理力度 effort</label>
                <select class="form-input" v-model="attrDrafts[m.model_name].effort">
                  <option value="">off（不发送）</option>
                  <option value="low">low</option>
                  <option value="medium">medium</option>
                  <option value="high">high</option>
                </select>
                <span class="attr-effect">{{ FIELD_EFFECT['reasoning_effort'] }}</span>
              </div>
              <div class="attr-field">
                <label class="form-label">参数量（十亿参数，如 30 = 30B）</label>
                <input class="form-input" type="number" min="1" v-model="attrDrafts[m.model_name].size" placeholder="30" />
                <span class="attr-effect">{{ FIELD_EFFECT['model_size_b'] }}</span>
              </div>
              <div class="attr-field">
                <label class="form-label">真名（别名模型的实际型号名）</label>
                <input class="form-input" v-model="attrDrafts[m.model_name].realName" placeholder="如 Qwen3-30B" />
                <span class="attr-effect">{{ FIELD_EFFECT['real_name'] }}</span>
              </div>
              <div class="attr-field attr-field--wide">
                <label class="form-label">
                  context_window
                  <template v-if="m.catalog_match?.context_window">
                    · 目录值：{{ m.catalog_match.context_window.toLocaleString() }}
                    <a href="javascript:void(0)" @click="fillCtxFromCatalog(m)">填入</a>
                  </template>
                </label>
                <input class="form-input" type="number" min="1" v-model="attrDrafts[m.model_name].ctx" placeholder="131072" />
                <span class="attr-effect">
                  {{ FIELD_EFFECT['context_window'] }}
                  <template v-if="m.catalog_match?.family">（models.dev：{{ m.catalog_match.family }}）</template>
                </span>
              </div>
            </div>
            <div class="attr-actions">
              <span class="attr-dirty">{{ dirtyFields(m).length ? `已修改：${dirtyFields(m).join('、')}` : '无修改' }}</span>
              <button class="btn btn-sm" @click="toggleAttrs(m)">收起</button>
              <button class="btn btn-sm btn-primary" @click="saveAttrs(m)">保存属性</button>
            </div>
          </div>
          <div class="card-footer">
            <button class="btn btn-sm btn-ghost" @click="testModel(m.model_name)" :disabled="testing === m.model_name">
              <span v-if="testing === m.model_name" class="spinner" style="width:14px;height:14px;"></span>
              {{ testing === m.model_name ? '测试中...' : '测试' }}
            </button>
            <button class="btn btn-sm btn-ghost" @click="toggleAttrs(m)">
              {{ expandedAttrs.has(m.model_name) ? '收起属性' : '属性' }}
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

/* P3-2 attribute editor */
.catalog-hint {
  font-size: 12px;
  color: var(--text-muted);
  margin-right: var(--space-2);
}
.attr-editor {
  padding: 10px 16px;
  border-top: 1px dashed var(--border);
  background: var(--bg-primary);
}
.attr-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 10px 14px;
}
.attr-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.attr-field--wide {
  grid-column: 1 / -1;
}
.attr-effect {
  font-size: 11px;
  color: var(--text-muted);
}
.attr-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 10px;
}
.attr-dirty {
  flex: 1;
  font-size: 11px;
  color: var(--text-muted);
}
</style>
