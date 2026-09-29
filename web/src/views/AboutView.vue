<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { renderMarkdownHtml } from '../utils/markdown'
import { authedFetch } from '../lib/authFetch'

const { request } = useWSAPI()
const activeTab = ref('about')
const readmeContent = ref('')
const readmeLoading = ref(false)
const readmeError = ref('')
const version = ref('--')

// 构建形态（system.features）：build.rs 从 features.toml 解析 + 本构建
// CARGO_FEATURE_* env 判定的真实编译态。channels / subsystems 两组渲染。
interface FeatureInfo {
  id: string
  label: string
  desc: string
  category: string
  default: boolean
  enabled: boolean
}
const buildFeatures = ref<FeatureInfo[]>([])
const buildFeaturesLoaded = ref(false)
const buildFeaturesLoading = ref(false)
const buildFeaturesError = ref('')

onMounted(async () => {
  try {
    const data = await request('system', 'version')
    if (data?.version) version.value = data.version
  } catch { /* 保留默认 '--' */ }
})

async function loadReadme() {
  if (readmeContent.value) return
  readmeLoading.value = true
  readmeError.value = ''
  try {
    const resp = await authedFetch('/api/system/readme')
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`)
    const data = await resp.json()
    readmeContent.value = data.content || ''
  } catch (e: any) {
    readmeError.value = e.message || '加载失败'
  } finally {
    readmeLoading.value = false
  }
}

async function loadBuildFeatures() {
  if (buildFeaturesLoaded.value) return
  buildFeaturesLoading.value = true
  buildFeaturesError.value = ''
  try {
    const data = await request('system', 'features')
    buildFeatures.value = data?.features || []
    buildFeaturesLoaded.value = true
  } catch (e: any) {
    buildFeaturesError.value = e?.message || String(e) || '加载失败'
  } finally {
    buildFeaturesLoading.value = false
  }
}

function switchTab(tab: string) {
  activeTab.value = tab
  if (tab === 'readme') loadReadme()
  if (tab === 'build') loadBuildFeatures()
}

const renderedReadme = computed(() => {
  if (!readmeContent.value) return ''
  return renderMarkdownHtml(readmeContent.value)
})

const channelFeatures = computed(() => buildFeatures.value.filter(f => f.category === 'channels'))
const subsystemFeatures = computed(() => buildFeatures.value.filter(f => f.category === 'subsystems'))
const enabledCount = computed(() => buildFeatures.value.filter(f => f.enabled).length)
</script>

<template>
  <div class="page-about">
    <div class="page-header"><h2>关于</h2></div>
    <div class="page-body">
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'about' }" @click="switchTab('about')">关于</button>
        <button class="tab" :class="{ active: activeTab === 'build' }" @click="switchTab('build')">构建形态</button>
        <button class="tab" :class="{ active: activeTab === 'readme' }" @click="switchTab('readme')">Readme</button>
      </div>

      <!-- About Tab -->
      <div v-if="activeTab === 'about'">
        <div class="card">
          <div class="card-body" style="text-align: center; padding: var(--space-8) var(--space-4);">
            <h2 style="margin-bottom: var(--space-2); font-size: var(--text-2xl);">NemesisBot</h2>
            <p style="color: var(--text-muted); margin-bottom: var(--space-4);">
              安全第一的 AI 智能管家（Rust 版）
            </p>
            <div class="about-info-grid">
              <span class="about-info-key">版本</span>
              <span class="about-info-val">{{ version }}</span>
              <span class="about-info-key">运行时</span>
              <span class="about-info-val">Rust</span>
              <span class="about-info-key">协议</span>
              <span class="about-info-val">AGPL-3.0 / 商业双授权</span>
            </div>
            <p style="color: var(--text-muted); margin-top: var(--space-6); font-size: var(--text-sm);">
              多入口编码 agent · 分布式集群 · 九层安全体系 ·
              <router-link to="/license">查看许可与使用限制</router-link>
            </p>
          </div>
        </div>
      </div>

      <!-- 构建 Tab -->
      <div v-if="activeTab === 'build'">
        <div class="card">
          <div class="card-header">
            <h3>本构建的功能形态（{{ enabledCount }}/{{ buildFeatures.length }} 开启）</h3>
          </div>
          <div class="card-body">
            <p style="font-size: var(--text-xs); color: var(--text-muted); margin: 0 0 var(--space-3);">
              由编译期 cargo feature 决定（customize / menuconfig 裁剪），此处为当前二进制的真实编译态；变更需重新构建。
            </p>
            <div v-if="buildFeaturesLoading" style="text-align: center; padding: var(--space-6);">
              <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
            </div>
            <div v-else-if="buildFeaturesError" class="empty-state">
              <p>加载失败：{{ buildFeaturesError }}</p>
            </div>
            <div v-else-if="!buildFeatures.length" class="empty-state">
              <p>当前构建未提供形态清单。</p>
            </div>
            <template v-else>
              <h4 style="margin: var(--space-2) 0 var(--space-2); font-size: var(--text-sm);">子系统（{{ subsystemFeatures.filter(f => f.enabled).length }}/{{ subsystemFeatures.length }}）</h4>
              <div class="feature-grid">
                <div v-for="f in subsystemFeatures" :key="f.id" class="feature-item" :title="f.desc">
                  <span class="feature-dot" :style="{ background: f.enabled ? 'var(--success)' : 'var(--text-muted)' }"></span>
                  <span class="feature-label">{{ f.label || f.id }}</span>
                  <span class="feature-state" :style="{ color: f.enabled ? 'var(--success)' : 'var(--text-muted)' }">{{ f.enabled ? '开' : '关' }}</span>
                </div>
              </div>
              <h4 style="margin: var(--space-4) 0 var(--space-2); font-size: var(--text-sm);">消息通道（{{ channelFeatures.filter(f => f.enabled).length }}/{{ channelFeatures.length }}）</h4>
              <div class="feature-grid">
                <div v-for="f in channelFeatures" :key="f.id" class="feature-item" :title="f.desc">
                  <span class="feature-dot" :style="{ background: f.enabled ? 'var(--success)' : 'var(--text-muted)' }"></span>
                  <span class="feature-label">{{ f.label || f.id }}</span>
                  <span class="feature-state" :style="{ color: f.enabled ? 'var(--success)' : 'var(--text-muted)' }">{{ f.enabled ? '开' : '关' }}</span>
                </div>
              </div>
            </template>
          </div>
        </div>
      </div>

      <!-- Readme Tab -->
      <div v-if="activeTab === 'readme'">
        <div class="card">
          <div class="card-body">
            <div v-if="readmeLoading" style="text-align: center; padding: var(--space-8);">
              <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
            </div>
            <div v-else-if="readmeError" class="empty-state">
              <p>{{ readmeError }}</p>
            </div>
            <div v-else class="markdown-body" v-html="renderedReadme"></div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.feature-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
  gap: var(--space-1) var(--space-3);
}
.feature-item {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-xs);
  padding: 2px 0;
  min-width: 0;
}
.feature-dot {
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
}
.feature-label {
  flex: 1;
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.feature-state {
  flex: none;
  font-weight: 600;
}
</style>
