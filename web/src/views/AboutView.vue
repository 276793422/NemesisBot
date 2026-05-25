<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { useAppStore } from '../stores/app'
import { marked } from 'marked'
import hljs from 'highlight.js'

const appStore = useAppStore()
const activeTab = ref('about')
const readmeContent = ref('')
const readmeLoading = ref(false)
const readmeError = ref('')

const version = computed(() => appStore.version || '--')

async function loadReadme() {
  if (readmeContent.value) return
  readmeLoading.value = true
  readmeError.value = ''
  try {
    const resp = await fetch('/api/system/readme')
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`)
    const data = await resp.json()
    readmeContent.value = data.content || ''
  } catch (e: any) {
    readmeError.value = e.message || '加载失败'
  } finally {
    readmeLoading.value = false
  }
}

function switchTab(tab: string) {
  activeTab.value = tab
  if (tab === 'readme') loadReadme()
}

const renderedReadme = computed(() => {
  if (!readmeContent.value) return ''
  return marked.parse(readmeContent.value, {
    gfm: true,
    breaks: false,
  })
})
</script>

<template>
  <div class="page-about">
    <div class="page-header"><h2>关于</h2></div>
    <div class="page-body">
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'about' }" @click="switchTab('about')">关于</button>
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
              <span class="about-info-val">MIT License</span>
            </div>
            <p style="color: var(--text-muted); margin-top: var(--space-6); font-size: var(--text-sm);">
              本仓库是 NemesisBot Go 版的 Rust 重写，实现 100% 功能对等。
            </p>
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
