<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface Skill { name: string; has_skill_md?: boolean; description?: string }
interface SearchResult { name?: string; slug?: string; description?: string; source?: string }
interface Source {
  type: string; name: string; repo?: string; base_url?: string;
  enabled: boolean; deletable?: boolean; branch?: string;
  index_type?: string; skill_path_pattern?: string;
}

const activeTab = ref('installed')
const skills = ref<Skill[]>([])
const loading = ref(true)
const detailContent = ref('')
const detailName = ref('')
const searchQuery = ref('')
const searchResults = ref<SearchResult[]>([])
const searching = ref(false)

// Config tab state
const skillsConfig = ref<any>({})
const configEditing = ref(false)
const configEditContent = ref('')

// Source management state
const sources = ref<Source[]>([])
const sourcesLoading = ref(false)
const showAddDialog = ref(false)
const showManualDialog = ref(false)
const addUrl = ref('')
const adding = ref(false)
const partialData = ref<any>(null)
const detectError = ref('')
const manualForm = ref({ name: '', repo: '', branch: 'main', index_type: 'github_api', skill_path_pattern: 'skills/{slug}/SKILL.md' })
const confirmDelete = ref('')

async function loadInstalled() {
  try {
    const data = await request('skills', 'installed')
    skills.value = data?.skills || []
  } catch (e: any) {
    toast.error('加载失败: ' + e)
  }
  loading.value = false
}

async function showDetail(name: string) {
  try {
    const data = await request('skills', 'detail', { name })
    detailContent.value = data?.content || ''
    detailName.value = name
  } catch (e: any) {
    toast.error('读取失败: ' + e)
  }
}

async function uninstallSkill(name: string) {
  if (!confirm(`确定卸载技能 "${name}" 吗？`)) return
  try {
    await request('skills', 'uninstall', { name })
    toast.success('已卸载')
    if (detailName.value === name) { detailName.value = ''; detailContent.value = '' }
    await loadInstalled()
  } catch (e: any) {
    toast.error('卸载失败: ' + e)
  }
}

async function searchSkills() {
  if (!searchQuery.value) return
  searching.value = true
  try {
    const data = await request('skills', 'search', { query: searchQuery.value })
    searchResults.value = data?.results || []
    if (data?.message) toast.info(data.message)
  } catch (e: any) {
    toast.error('搜索失败: ' + e)
  }
  searching.value = false
}

async function loadConfig() {
  try {
    const data = await request('skills', 'config.get')
    skillsConfig.value = data || {}
    configEditContent.value = JSON.stringify(data, null, 2)
  } catch { /* ignore */ }
}

async function saveConfig() {
  try {
    const parsed = JSON.parse(configEditContent.value)
    await request('skills', 'config.save', parsed)
    toast.success('配置已保存')
    configEditing.value = false
    await refreshAll()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

async function loadSources() {
  sourcesLoading.value = true
  try {
    const data = await request('skills', 'source.list')
    sources.value = data?.sources || []
  } catch (e: any) {
    toast.error('加载源列表失败: ' + e)
  }
  sourcesLoading.value = false
}

async function refreshAll() {
  await Promise.all([loadSources(), loadConfig()])
}

async function toggleSource(name: string, enabled: boolean) {
  try {
    await request('skills', 'source.toggle', { name, enabled })
    await refreshAll()
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

async function toggleSkillsEnabled(enabled: boolean) {
  try {
    await request('skills', 'config.update', { enabled })
    await refreshAll()
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

async function openJsonEditor() {
  await loadConfig()
  configEditContent.value = JSON.stringify(skillsConfig.value, null, 2)
  configEditing.value = true
}

async function addSource() {
  if (!addUrl.value.trim()) return
  adding.value = true
  try {
    const data = await request('skills', 'source.add', { url: addUrl.value.trim() }, 0)
    if (data?.success) {
      toast.success(`源 "${data.source.name}" 已添加`)
      showAddDialog.value = false
      addUrl.value = ''
      await refreshAll()
    } else if (data?.partial) {
      partialData.value = data.partial
      detectError.value = data.error || '自动探测失败'
      manualForm.value.name = data.partial.name || ''
      manualForm.value.repo = data.partial.repo || ''
      showAddDialog.value = false
      showManualDialog.value = true
    }
  } catch (e: any) {
    toast.error('添加失败: ' + e)
  }
  adding.value = false
}

async function addSourceManual() {
  try {
    const data = await request('skills', 'source.add.manual', manualForm.value)
    if (data?.success) {
      toast.success(`源 "${data.source.name}" 已添加`)
      showManualDialog.value = false
      partialData.value = null
      detectError.value = ''
      await refreshAll()
    }
  } catch (e: any) {
    toast.error('添加失败: ' + e)
  }
}

async function removeSource(name: string) {
  if (confirmDelete.value !== name) return
  try {
    await request('skills', 'source.remove', { name })
    toast.success(`源 "${name}" 已删除`)
    confirmDelete.value = ''
    await refreshAll()
  } catch (e: any) {
    toast.error('删除失败: ' + e)
  }
}

function openAddDialog() {
  addUrl.value = ''
  showAddDialog.value = true
}

function switchTab(tab: string) {
  activeTab.value = tab
  if (tab === 'config') {
    loadConfig()
    loadSources()
  }
}

onMounted(loadInstalled)
</script>

<template>
  <div class="page-skills">
    <div class="page-header"><h2>Skills 管理</h2></div>
    <div class="page-body">
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'installed' }" @click="activeTab = 'installed'">已安装</button>
        <button class="tab" :class="{ active: activeTab === 'shop' }" @click="activeTab = 'shop'">商店</button>
        <button class="tab" :class="{ active: activeTab === 'config' }" @click="switchTab('config')">配置</button>
      </div>

      <!-- Installed tab -->
      <div v-if="activeTab === 'installed'">
        <div v-if="loading" style="text-align: center; padding: var(--space-8);">
          <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
        </div>
        <div v-if="!loading && skills.length === 0" class="empty-state">
          <h3>暂无技能</h3>
          <p>通过商店搜索安装技能</p>
        </div>
        <div v-if="!loading && skills.length > 0" style="display: grid; grid-template-columns: repeat(auto-fill, minmax(300px, 1fr)); gap: var(--space-4);">
          <div v-for="s in skills" :key="s.name" class="skill-card">
            <div class="skill-card-header">
              <div>
                <div class="skill-name">{{ s.name }}</div>
                <div v-if="s.description" style="font-size: var(--text-xs); color: var(--text-muted); margin-top: 2px;">{{ s.description }}</div>
              </div>
              <span class="badge" :class="s.has_skill_md ? 'badge-success' : 'badge-neutral'">
                {{ s.has_skill_md ? '有效' : '缺少定义' }}
              </span>
            </div>
            <div class="skill-description" v-if="!s.description">暂无描述</div>
            <div style="display: flex; gap: var(--space-2); margin-top: var(--space-3);">
              <button class="btn btn-sm" @click="showDetail(s.name)">查看</button>
              <button class="btn btn-sm btn-danger" @click="uninstallSkill(s.name)">卸载</button>
            </div>
          </div>
        </div>
        <!-- Detail modal-like overlay -->
        <div v-if="detailName" class="modal-backdrop" @click.self="detailName = ''">
          <div class="modal" style="max-width: 700px;">
            <div class="modal-header">
              <h3>{{ detailName }}</h3>
              <button class="modal-close" @click="detailName = ''">&times;</button>
            </div>
            <div class="modal-body">
              <div class="markdown-body"><pre style="white-space: pre-wrap;">{{ detailContent }}</pre></div>
            </div>
          </div>
        </div>
      </div>

      <!-- Shop tab -->
      <div v-if="activeTab === 'shop'">
        <div style="display: flex; gap: var(--space-3); margin-bottom: var(--space-4);">
          <input class="form-input" v-model="searchQuery" placeholder="搜索技能..." @keyup.enter="searchSkills" style="max-width: 400px;">
          <button class="btn btn-primary" @click="searchSkills" :disabled="searching">搜索</button>
        </div>
        <div v-if="searching" style="text-align: center; padding: var(--space-4);">
          <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
        </div>
        <div v-if="!searching && searchResults.length === 0" class="empty-state">
          <p>输入关键词搜索远程技能</p>
        </div>
        <div v-if="!searching && searchResults.length > 0" style="display: grid; grid-template-columns: repeat(auto-fill, minmax(280px, 1fr)); gap: var(--space-4);">
          <div v-for="(r, idx) in searchResults" :key="idx" class="skill-card">
            <div class="skill-name">{{ r.name || r.slug }}</div>
            <div class="skill-description">{{ r.description || '暂无描述' }}</div>
          </div>
        </div>
      </div>

      <!-- Config tab -->
      <div v-if="activeTab === 'config'">
        <!-- Basic config -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>基础配置</h3>
            <button class="btn btn-sm" @click="openJsonEditor">JSON 编辑</button>
          </div>
          <div class="card-body">
            <div v-if="configEditing">
              <textarea class="form-textarea" style="min-height: 40vh; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="configEditContent"></textarea>
              <div style="display: flex; gap: var(--space-2); margin-top: var(--space-3);">
                <button class="btn btn-sm" @click="configEditing = false">取消</button>
                <button class="btn btn-sm btn-primary" @click="saveConfig">保存</button>
              </div>
            </div>
            <div v-else class="settings-grid">
              <span class="settings-key">Skills 系统</span>
              <span class="settings-value" style="display: flex; align-items: center; gap: var(--space-2);">
                <span class="badge" :class="skillsConfig.enabled ? 'badge-success' : 'badge-error'">{{ skillsConfig.enabled ? '启用' : '停用' }}</span>
                <label class="toggle-switch" :title="skillsConfig.enabled ? '点击禁用' : '点击启用'">
                  <input type="checkbox" :checked="skillsConfig.enabled" @change="toggleSkillsEnabled(($event.target as HTMLInputElement).checked)">
                  <span class="toggle-slider"></span>
                </label>
              </span>
              <span class="settings-key">搜索缓存</span>
              <span class="settings-value">{{ skillsConfig.search_cache?.enabled ? '启用' : '停用' }}</span>
              <span class="settings-key">并发搜索数</span>
              <span class="settings-value">{{ skillsConfig.max_concurrent_searches }}</span>
            </div>
          </div>
        </div>

        <!-- Source management -->
        <div class="card">
          <div class="card-header">
            <h3>源管理</h3>
            <button class="btn btn-sm btn-primary" @click="openAddDialog">+ 添加源</button>
          </div>
          <div class="card-body">
            <div v-if="sourcesLoading" style="text-align: center; padding: var(--space-4);">
              <div class="spinner" style="margin: 0 auto;"></div>
            </div>
            <div v-else-if="sources.length === 0" class="empty-state" style="padding: var(--space-6);">
              <p>暂无源，点击上方按钮添加</p>
            </div>
            <div v-else class="source-grid">
              <div v-for="s in sources" :key="s.name" class="source-card" :class="{ 'source-card-enabled': s.enabled, 'source-card-disabled': !s.enabled }">
                <div class="source-card-main">
                  <div class="source-card-info">
                    <div class="source-card-name">{{ s.name }}</div>
                    <div class="source-card-detail">{{ s.repo || s.base_url }}</div>
                  </div>
                  <label class="toggle-switch" :title="s.enabled ? '点击禁用' : '点击启用'">
                    <input type="checkbox" :checked="s.enabled" @change="toggleSource(s.name, ($event.target as HTMLInputElement).checked)">
                    <span class="toggle-slider"></span>
                  </label>
                </div>
                <div class="source-card-actions">
                  <template v-if="s.deletable === false">
                    <span class="badge badge-neutral">内置源</span>
                  </template>
                  <template v-else-if="confirmDelete === s.name">
                    <span style="font-size: var(--text-xs); color: var(--error);">确定删除？</span>
                    <button class="btn btn-sm btn-danger" @click="removeSource(s.name)">确认</button>
                    <button class="btn btn-sm" @click="confirmDelete = ''">取消</button>
                  </template>
                  <template v-else>
                    <button class="btn btn-sm btn-danger" @click="confirmDelete = s.name">删除</button>
                  </template>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- Add source dialog -->
      <div v-if="showAddDialog" class="modal-backdrop" @click.self="showAddDialog = false">
        <div class="modal" style="max-width: 480px;">
          <div class="modal-header">
            <h3>添加源</h3>
            <button class="modal-close" @click="showAddDialog = false">&times;</button>
          </div>
          <div class="modal-body">
            <label class="form-label">源地址</label>
            <input class="form-input" v-model="addUrl" placeholder="https://github.com/user/repo" @keyup.enter="addSource" style="width: 100%;">
            <p style="font-size: var(--text-xs); color: var(--text-muted); margin-top: var(--space-2);">
              支持 GitHub 仓库地址、owner/repo 格式
            </p>
          </div>
          <div class="modal-footer">
            <button class="btn btn-sm" @click="showAddDialog = false">取消</button>
            <button class="btn btn-sm btn-primary" @click="addSource" :disabled="adding || !addUrl.trim()">
              {{ adding ? '探测中...' : '添加' }}
            </button>
          </div>
        </div>
      </div>

      <!-- Manual add dialog -->
      <div v-if="showManualDialog" class="modal-backdrop" @click.self="showManualDialog = false">
        <div class="modal" style="max-width: 480px;">
          <div class="modal-header">
            <h3>手动配置源</h3>
            <button class="modal-close" @click="showManualDialog = false">&times;</button>
          </div>
          <div class="modal-body">
            <div class="form-notice">{{ detectError }}</div>
            <div class="form-group">
              <label class="form-label">名称 *</label>
              <input class="form-input" v-model="manualForm.name" style="width: 100%;">
            </div>
            <div class="form-group">
              <label class="form-label">仓库 *</label>
              <input class="form-input" v-model="manualForm.repo" placeholder="owner/repo" style="width: 100%;">
            </div>
            <div class="form-group">
              <label class="form-label">分支</label>
              <input class="form-input" v-model="manualForm.branch" style="width: 100%;">
            </div>
            <div class="form-group">
              <label class="form-label">索引类型</label>
              <select class="form-input" v-model="manualForm.index_type" style="width: 100%;">
                <option value="github_api">github_api</option>
                <option value="skills_json">skills_json</option>
              </select>
            </div>
            <div class="form-group">
              <label class="form-label">路径模式</label>
              <input class="form-input" v-model="manualForm.skill_path_pattern" style="width: 100%;">
            </div>
          </div>
          <div class="modal-footer">
            <button class="btn btn-sm" @click="showManualDialog = false">取消</button>
            <button class="btn btn-sm btn-primary" @click="addSourceManual" :disabled="!manualForm.name || !manualForm.repo">保存</button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.source-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: var(--space-3);
}
.source-card {
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-3) var(--space-4);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  transition: background 0.2s;
}
.source-card-enabled {
  background: rgba(59, 130, 246, 0.08);
  border-color: rgba(59, 130, 246, 0.2);
}
.source-card-disabled {
  background: var(--surface);
  opacity: 0.6;
}
.source-card-main {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
}
.source-card-info {
  flex: 1;
  min-width: 0;
}
.source-card-name {
  font-weight: 600;
  font-size: var(--text-sm);
  color: var(--text);
}
.source-card-detail {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-top: 2px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.source-card-actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding-top: var(--space-2);
  border-top: 1px solid var(--border-light);
}

/* Toggle switch */
.toggle-switch {
  position: relative;
  display: inline-block;
  width: 36px;
  height: 20px;
  flex-shrink: 0;
  cursor: pointer;
}
.toggle-switch input {
  opacity: 0;
  width: 0;
  height: 0;
}
.toggle-slider {
  position: absolute;
  inset: 0;
  background: rgba(239, 68, 68, 0.25);
  border-radius: 10px;
  transition: background 0.2s;
}
.toggle-slider::before {
  content: '';
  position: absolute;
  width: 16px;
  height: 16px;
  left: 2px;
  bottom: 2px;
  background: var(--error);
  border-radius: 50%;
  transition: transform 0.2s, background 0.2s;
}
.toggle-switch input:checked + .toggle-slider {
  background: rgba(74, 222, 128, 0.25);
}
.toggle-switch input:checked + .toggle-slider::before {
  transform: translateX(16px);
  background: var(--success);
}

/* Form helpers */
.form-label {
  display: block;
  font-size: var(--text-sm);
  font-weight: 500;
  color: var(--text-secondary);
  margin-bottom: var(--space-1);
}
.form-group {
  margin-bottom: var(--space-3);
}
.form-notice {
  background: var(--warning-bg);
  color: var(--warning);
  padding: var(--space-2) var(--space-3);
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  margin-bottom: var(--space-4);
}
.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
  padding: var(--space-3) var(--space-4);
  border-top: 1px solid var(--border);
}

@media (max-width: 600px) {
  .source-grid {
    grid-template-columns: 1fr;
  }
}
</style>
