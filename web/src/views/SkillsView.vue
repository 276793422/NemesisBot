<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface Skill { name: string; has_skill_md?: boolean; description?: string }
interface SearchResult { name?: string; slug?: string; description?: string; source?: string; source_repo?: string; version?: string; score?: number; installed?: boolean }
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
const installingSlug = ref('')
const alreadyInstalledSlugs = ref<Set<string>>(new Set())
const shopDetail = ref<any>(null)
const shopDetailLoading = ref(false)
const shopCode = ref('')
const shopCodeFilename = ref('SKILL.md')
const shopCodeLoading = ref(false)
const showShopCode = ref(false)

// Browse mode state
const SHOP_CATEGORIES = [
  { id: 'coding', name: '编程开发' },
  { id: 'git github', name: 'Git & GitHub' },
  { id: 'web frontend', name: 'Web & 前端' },
  { id: 'devops cloud', name: 'DevOps & 云服务' },
  { id: 'browser automation', name: '浏览器 & 自动化' },
  { id: 'search research', name: '搜索 & 研究' },
  { id: 'ai ml', name: 'AI & 机器学习' },
  { id: 'data analytics', name: '数据分析' },
  { id: 'productivity', name: '生产力工具' },
  { id: 'communication', name: '通讯' },
  { id: 'media streaming', name: '媒体 & 流媒体' },
  { id: 'notes pkm', name: '笔记 & 知识管理' },
  { id: 'security', name: '安全' },
  { id: 'cli utilities', name: 'CLI 工具' },
  { id: 'marketing sales', name: '营销' },
  { id: 'finance', name: '金融' },
  { id: 'smart home iot', name: '智能家居' },
]
const BUILTIN_SOURCES = [
  { id: 'clawhub', name: 'ClawHub' },
  { id: 'modelscope', name: 'ModelScope' },
]
const selectedSource = ref('clawhub')
const showCategoryDialog = ref(false)
const shopSort = ref('trending')
const browseResults = ref<SearchResult[]>([])
const browseDisplayCount = ref(20)
const browseCursor = ref<string | null>(null)
const browseLoading = ref(false)
const isBrowseMode = ref(true)
const browseCache = new Map<string, { data: { items: any[]; next_cursor: string | null }; ts: number }>()

// Config tab state
const skillsConfig = ref<any>({})
const configEditing = ref(false)
const configEditContent = ref('')

// Source management state
const sources = ref<Source[]>([])
const sourcesLoading = ref(false)
const showAddDialog = ref(false)
const showManualDialog = ref(false)

// Learn skill (distill a source into a skill)
const showLearnDialog = ref(false)
const learnSource = ref('')
const learnName = ref('')
const learning = ref(false)
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

async function learnSkill() {
  if (!learnSource.value.trim()) return
  learning.value = true
  try {
    const data = await request('skills', 'learn', {
      source: learnSource.value.trim(),
      name: learnName.value.trim() || undefined,
    })
    toast.success(data?.message || '已开始学习，请在对话窗口查看进度')
    showLearnDialog.value = false
    learnSource.value = ''
    learnName.value = ''
    // The agent runs asynchronously and writes the skill via skill_manage.
    // Refresh the installed list shortly after so the new skill appears.
    setTimeout(() => { loadInstalled() }, 8000)
  } catch (e: any) {
    toast.error('学习失败: ' + e)
  }
  learning.value = false
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

async function openDir(name: string) {
  try {
    await request('skills', 'open_dir', { name })
  } catch (e: any) {
    toast.error('打开目录失败: ' + e)
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
  isBrowseMode.value = false
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

function isInstalled(slug: string | undefined): boolean {
  if (!slug) return false
  return skills.value.some(s => s.name === slug)
}

async function installSkill(registry: string, slug: string, force = false) {
  installingSlug.value = slug
  try {
    const data = await request('skills', 'install', { registry, slug, force })
    if (data?.already_installed) {
      alreadyInstalledSlugs.value.add(slug)
      toast.info(`${slug} 已安装，点击"更新"可覆盖安装`)
    } else {
      toast.success(force ? `已更新: ${slug}` : `已安装: ${slug}`)
      alreadyInstalledSlugs.value.delete(slug)
      await loadInstalled()
    }
  } catch (e: any) {
    toast.error('安装失败: ' + e)
  }
  installingSlug.value = ''
}

function handleInstallClick(registry: string, slug: string) {
  if (alreadyInstalledSlugs.value.has(slug)) {
    installSkill(registry, slug, true)
  } else {
    installSkill(registry, slug, false)
  }
}

async function showShopDetailFn(registry: string, slug: string) {
  shopDetailLoading.value = true
  shopDetail.value = null
  shopCode.value = ''
  showShopCode.value = false
  try {
    const data = await request('skills', 'shop_detail', { registry, slug })
    shopDetail.value = data
  } catch (e: any) {
    toast.error('获取详情失败: ' + e)
  }
  shopDetailLoading.value = false
}

async function viewShopCode(registry: string, slug: string) {
  if (showShopCode.value) {
    showShopCode.value = false
    return
  }
  shopCodeLoading.value = true
  try {
    const data = await request('skills', 'shop_code', { registry, slug }, 60000)
    shopCode.value = data?.code || ''
    shopCodeFilename.value = data?.filename || 'SKILL.md'
    showShopCode.value = true
  } catch (e: any) {
    toast.error('获取源码失败: ' + e)
  }
  shopCodeLoading.value = false
}

function closeShopDetail() {
  shopDetail.value = null
  shopCode.value = ''
  shopCodeFilename.value = 'SKILL.md'
  showShopCode.value = false
}

async function browseSkills(sort?: string) {
  if (sort) shopSort.value = sort
  browseLoading.value = true
  browseResults.value = []
  browseCursor.value = null
  browseDisplayCount.value = 20

  const cacheKey = `browse:${selectedSource.value}:${shopSort.value}`
  const cached = browseCache.get(cacheKey)
  if (cached && (Date.now() - cached.ts) < 60000) {
    browseResults.value = cached.data.items
    browseCursor.value = cached.data.next_cursor
    browseLoading.value = false
    return
  }

  try {
    const data = await request('skills', 'browse', { registry: selectedSource.value, sort: shopSort.value, limit: 100 })
    browseResults.value = data?.items || []
    browseCursor.value = data?.next_cursor || null
    browseCache.set(cacheKey, { data: { items: browseResults.value, next_cursor: browseCursor.value }, ts: Date.now() })
  } catch (e: any) {
    toast.error('浏览失败: ' + e)
  }
  browseLoading.value = false
}

async function loadMore() {
  // Client-side pagination: show 20 more from already-loaded results
  if (browseDisplayCount.value < browseResults.value.length) {
    browseDisplayCount.value += 20
    return
  }
  // Server-side pagination: fetch more from API if cursor available
  if (!browseCursor.value || browseLoading.value) return
  browseLoading.value = true
  try {
    const data = await request('skills', 'browse', { registry: selectedSource.value, sort: shopSort.value, limit: 100, cursor: browseCursor.value })
    const items = data?.items || []
    browseResults.value = [...browseResults.value, ...items]
    browseCursor.value = data?.next_cursor || null
    browseDisplayCount.value = browseResults.value.length
  } catch (e: any) {
    toast.error('加载更多失败: ' + e)
  }
  browseLoading.value = false
}

function searchCategory(cat: typeof SHOP_CATEGORIES[number]) {
  searchQuery.value = cat.id
  isBrowseMode.value = false
  showCategoryDialog.value = false
  searchSkills()
}

function clearSearch() {
  searchQuery.value = ''
  searchResults.value = []
  isBrowseMode.value = true
  browseSkills()
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
  } else if (tab === 'shop' && isBrowseMode.value && browseResults.value.length === 0) {
    browseSkills()
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
        <button class="tab" :class="{ active: activeTab === 'shop' }" @click="switchTab('shop')">商店</button>
        <button class="tab" :class="{ active: activeTab === 'config' }" @click="switchTab('config')">配置</button>
      </div>

      <!-- Installed tab -->
      <div v-if="activeTab === 'installed'">
        <div style="display: flex; justify-content: flex-end; margin-bottom: var(--space-3);">
          <button class="btn btn-sm btn-primary" @click="showLearnDialog = true">＋ 学习技能</button>
        </div>
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
              <div class="skill-name">{{ s.name }}</div>
              <span class="badge" :class="s.has_skill_md ? 'badge-success' : 'badge-neutral'">
                {{ s.has_skill_md ? '有效' : '缺少定义' }}
              </span>
            </div>
            <div v-if="s.description" class="skill-description" style="display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden;">{{ s.description }}</div>
            <div class="skill-description" v-else>暂无描述</div>
            <div style="display: flex; gap: var(--space-2); margin-top: var(--space-3);">
              <button class="btn btn-sm" @click="showDetail(s.name)">查看</button>
              <button class="btn btn-sm" @click="openDir(s.name)">目录</button>
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
        <!-- Source selector -->
        <div style="display: flex; gap: var(--space-2); margin-bottom: var(--space-3);">
          <span v-for="src in BUILTIN_SOURCES" :key="src.id" class="filter-pill" :class="{ active: selectedSource === src.id }" @click="selectedSource = src.id; browseSkills()">{{ src.name }}</span>
        </div>
        <!-- Search bar -->
        <div style="display: flex; gap: var(--space-3); margin-bottom: var(--space-4);">
          <input class="form-input" v-model="searchQuery" placeholder="搜索技能..." @keyup.enter="searchSkills" style="max-width: 400px;">
          <button class="btn btn-primary" @click="searchSkills" :disabled="searching">搜索</button>
          <button v-if="!isBrowseMode" class="btn" @click="clearSearch">返回浏览</button>
          <button v-if="isBrowseMode" class="btn" @click="browseSkills()" :disabled="browseLoading">{{ browseLoading ? '刷新中...' : '刷新' }}</button>
        </div>

        <!-- Browse mode: sort pills + category button -->
        <template v-if="isBrowseMode">
          <div style="display: flex; gap: var(--space-2); align-items: center; margin-bottom: var(--space-3);">
            <div class="filter-pills" style="margin-bottom: 0;">
              <span class="filter-pill" :class="{ active: shopSort === 'trending' }" @click="browseSkills('trending')">热门</span>
              <span class="filter-pill" :class="{ active: shopSort === 'downloads' }" @click="browseSkills('downloads')">下载量</span>
              <span class="filter-pill" :class="{ active: shopSort === 'updated' }" @click="browseSkills('updated')">最近更新</span>
            </div>
            <button class="btn btn-sm" @click="showCategoryDialog = true">分类搜索</button>
          </div>
        </template>

        <!-- Search mode: results count -->
        <template v-if="!isBrowseMode">
          <div v-if="!searching && searchResults.length > 0" style="display: flex; justify-content: space-between; align-items: center; margin-bottom: var(--space-3);">
            <span style="font-size: var(--text-sm); color: var(--text-muted);">{{ searchResults.length }} 个结果</span>
          </div>
        </template>

        <!-- Loading spinner -->
        <div v-if="(isBrowseMode && browseLoading && browseResults.length === 0) || (!isBrowseMode && searching)" style="text-align: center; padding: var(--space-4);">
          <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
        </div>

        <!-- Empty state -->
        <div v-if="isBrowseMode && !browseLoading && browseResults.length === 0" class="empty-state">
          <p>暂无技能，请检查 ClawHub 源是否启用</p>
        </div>
        <div v-if="!isBrowseMode && !searching && searchResults.length === 0" class="empty-state">
          <p>未找到匹配的技能</p>
        </div>

        <!-- Results grid (shared by browse and search) -->
        <div v-if="(isBrowseMode && browseResults.length > 0) || (!isBrowseMode && searchResults.length > 0)">
          <div style="display: grid; grid-template-columns: repeat(auto-fill, minmax(280px, 1fr)); gap: var(--space-4);">
            <template v-if="isBrowseMode">
              <div v-for="(r, idx) in browseResults.slice(0, browseDisplayCount)" :key="'b-'+idx" class="skill-card" style="cursor: pointer;" @click="showShopDetailFn(r.source!, r.slug!)">
                <div class="skill-card-header">
                  <div class="skill-name">{{ r.name || r.slug }}</div>
                  <span class="badge badge-info" style="font-size: 0.65rem;">{{ r.source }}</span>
                </div>
                <div class="skill-description" style="display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden;">{{ r.description || '暂无描述' }}</div>
                <div style="display: flex; justify-content: space-between; align-items: center; margin-top: var(--space-3);">
                  <span v-if="r.version && r.version !== 'latest'" style="font-size: var(--text-xs); color: var(--text-muted);">v{{ r.version }}</span>
                  <span v-else></span>
                  <button class="btn btn-sm btn-primary" :disabled="installingSlug === r.slug || r.installed" @click.stop="handleInstallClick(r.source!, r.slug!)">
                    {{ r.installed ? '已安装' : alreadyInstalledSlugs.has(r.slug!) ? '更新' : installingSlug === r.slug ? '安装中...' : '安装' }}
                  </button>
                </div>
              </div>
            </template>
            <template v-else>
              <div v-for="(r, idx) in searchResults" :key="'s-'+idx" class="skill-card" style="cursor: pointer;" @click="showShopDetailFn(r.source!, r.slug!)">
                <div class="skill-card-header">
                  <div class="skill-name">{{ r.name || r.slug }}</div>
                  <span class="badge badge-info" style="font-size: 0.65rem;">{{ r.source }}</span>
                </div>
                <div class="skill-description" style="display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden;">{{ r.description || '暂无描述' }}</div>
                <div style="display: flex; justify-content: space-between; align-items: center; margin-top: var(--space-3);">
                  <span v-if="r.version && r.version !== 'latest'" style="font-size: var(--text-xs); color: var(--text-muted);">v{{ r.version }}</span>
                  <span v-else></span>
                  <button class="btn btn-sm btn-primary" :disabled="installingSlug === r.slug || r.installed" @click.stop="handleInstallClick(r.source!, r.slug!)">
                    {{ r.installed ? '已安装' : alreadyInstalledSlugs.has(r.slug!) ? '更新' : installingSlug === r.slug ? '安装中...' : '安装' }}
                  </button>
                </div>
              </div>
            </template>
          </div>

          <!-- Load More (browse mode only) -->
          <div v-if="isBrowseMode && (browseDisplayCount < browseResults.length || browseCursor)" style="text-align: center; padding: var(--space-4);">
            <button class="btn" @click="loadMore" :disabled="browseLoading">
              {{ browseLoading ? '加载中...' : '加载更多' }}
            </button>
          </div>
        </div>

        <!-- Category dialog -->
        <div v-if="showCategoryDialog" class="modal-backdrop" @click.self="showCategoryDialog = false">
          <div class="modal" style="max-width: 420px;">
            <div class="modal-header">
              <h3>分类搜索</h3>
              <button class="modal-close" @click="showCategoryDialog = false">&times;</button>
            </div>
            <div class="modal-body">
              <div style="display: flex; flex-wrap: wrap; gap: 6px;">
                <button v-for="cat in SHOP_CATEGORIES" :key="cat.id" class="filter-pill" style="font-size: 0.8rem; padding: 6px 14px;" @click="searchCategory(cat)">{{ cat.name }}</button>
              </div>
            </div>
          </div>
        </div>

        <!-- Shop detail modal -->
        <div v-if="shopDetail || shopDetailLoading" class="modal-backdrop" @click.self="closeShopDetail()">
          <div class="modal" style="max-width: 600px;">
            <div v-if="shopDetailLoading" style="text-align: center; padding: var(--space-8);">
              <div class="spinner" style="margin: 0 auto;"></div>
            </div>
            <template v-if="shopDetail">
              <div class="modal-header">
                <h3>{{ shopDetail.name || shopDetail.slug }}</h3>
                <button class="modal-close" @click="closeShopDetail()">&times;</button>
              </div>
              <div class="modal-body">
                <div style="display: flex; gap: var(--space-2); align-items: center; flex-wrap: wrap; margin-bottom: var(--space-3);">
                  <span class="badge badge-info">{{ shopDetail.registry }}</span>
                  <span v-if="shopDetail.version && shopDetail.version !== 'latest'" style="font-size: var(--text-xs); color: var(--text-muted);">v{{ shopDetail.version }}</span>
                  <span v-if="shopDetail.author" style="font-size: var(--text-xs); color: var(--text-muted);">by {{ shopDetail.author }}</span>
                </div>
                <div v-if="shopDetail.downloads" style="margin-bottom: var(--space-3);">
                  <span style="font-size: var(--text-sm); color: var(--text-muted);">{{ shopDetail.downloads.toLocaleString() }} downloads</span>
                </div>
                <div v-if="shopDetail.description" style="margin-bottom: var(--space-4);">
                  <p>{{ shopDetail.description }}</p>
                </div>
                <div style="display: flex; gap: var(--space-2);">
                  <button class="btn btn-primary" style="flex: 1;" :disabled="installingSlug === shopDetail.slug || shopDetail.installed" @click="handleInstallClick(shopDetail.registry, shopDetail.slug)">
                    {{ shopDetail.installed ? '已安装' : alreadyInstalledSlugs.has(shopDetail.slug) ? '更新' : installingSlug === shopDetail.slug ? '安装中...' : '安装' }}
                  </button>
                  <button class="btn" @click="viewShopCode(shopDetail.registry, shopDetail.slug)" :disabled="shopCodeLoading">
                    {{ shopCodeLoading ? '加载中...' : showShopCode ? '隐藏源码' : '查看源码' }}
                  </button>
                </div>
                <div v-if="showShopCode && shopCode" style="margin-top: var(--space-3); max-height: 300px; overflow: auto; border: 1px solid var(--border); border-radius: var(--radius-md); background: var(--surface);">
                  <div style="padding: 4px 12px; border-bottom: 1px solid var(--border); font-size: var(--text-xs); color: var(--text-muted);">{{ shopCodeFilename }}</div>
                  <pre style="margin: 0; padding: var(--space-3); font-size: var(--text-xs); line-height: 1.5; white-space: pre-wrap; word-break: break-all;">{{ shopCode }}</pre>
                </div>
                <div style="font-size: var(--text-xs); color: var(--text-muted); text-align: center; margin-top: var(--space-2);">技能安装前会进行安全扫描</div>
              </div>
            </template>
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

      <!-- Learn skill dialog -->
      <div v-if="showLearnDialog" class="modal-backdrop" @click.self="showLearnDialog = false">
        <div class="modal" style="max-width: 520px;">
          <div class="modal-header">
            <h3>学习技能</h3>
            <button class="modal-close" @click="showLearnDialog = false">&times;</button>
          </div>
          <div class="modal-body">
            <div class="form-group">
              <label class="form-label">来源（本地目录 / URL / 操作笔记）</label>
              <textarea class="form-input" v-model="learnSource" rows="4"
                placeholder="例如：~/projects/acme-sdk 的 REST 客户端，关注 auth 与分页；或 https://docs.example.com/api/quickstart；或粘贴一个操作流程"
                style="width: 100%; min-height: 96px; resize: vertical; font-family: inherit;"></textarea>
            </div>
            <div class="form-group">
              <label class="form-label">技能名（可选，留空自动生成）</label>
              <input class="form-input" v-model="learnName" placeholder="my-skill" style="width: 100%;">
            </div>
            <p style="font-size: var(--text-xs); color: var(--text-muted); margin-top: var(--space-2);">
              Agent 会读取来源、按规范生成 SKILL.md 并保存为可复用技能。处理进度在对话窗口查看。
            </p>
          </div>
          <div class="modal-footer">
            <button class="btn btn-sm" @click="showLearnDialog = false">取消</button>
            <button class="btn btn-sm btn-primary" @click="learnSkill" :disabled="learning || !learnSource.trim()">
              {{ learning ? '提交中...' : '开始学习' }}
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
.filter-pills {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  margin-bottom: var(--space-3);
}
.filter-pill {
  padding: 4px 12px;
  border-radius: 20px;
  font-size: 11px;
  font-weight: 600;
  cursor: pointer;
  border: 1px solid var(--border);
  background: transparent;
  color: var(--text-muted);
  transition: all 0.15s;
}
.filter-pill:hover {
  border-color: var(--primary);
  color: var(--text);
}
.filter-pill.active {
  background: var(--primary);
  border-color: var(--primary);
  color: #fff;
}
.filter-pill-sm {
  font-size: 0.7rem;
  padding: 2px 8px;
}

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
