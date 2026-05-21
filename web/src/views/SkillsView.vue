<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface Skill { name: string; has_skill_md?: boolean; description?: string }
interface SearchResult { name?: string; slug?: string; description?: string; source?: string }

const activeTab = ref('installed')
const skills = ref<Skill[]>([])
const loading = ref(true)
const detailContent = ref('')
const detailName = ref('')
const searchQuery = ref('')
const searchResults = ref<SearchResult[]>([])
const searching = ref(false)

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
  } catch (e: any) {
    toast.error('搜索失败: ' + e)
  }
  searching.value = false
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
        <button class="tab" :class="{ active: activeTab === 'config' }" @click="activeTab = 'config'">配置</button>
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
        <div class="card">
          <div class="card-header"><h3>Skills 配置</h3></div>
          <div class="card-body">
            <p style="color: var(--text-muted); font-size: var(--text-sm);">Skills 配置文件: config/config.skills.json</p>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
