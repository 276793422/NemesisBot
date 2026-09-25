<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { renderMarkdownHtml } from '../utils/markdown'
import { apiUrl } from '../lib/appBase'

const licenseContent = ref('')
const loading = ref(true)
const error = ref('')

onMounted(async () => {
  try {
    const resp = await fetch(apiUrl('/api/system/license'))
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`)
    const data = await resp.json()
    licenseContent.value = data.content || ''
  } catch (e: any) {
    error.value = e.message || '加载失败'
  } finally {
    loading.value = false
  }
})

const renderedLicense = computed(() => {
  if (!licenseContent.value) return ''
  return renderMarkdownHtml(licenseContent.value)
})
</script>

<template>
  <div class="page-license">
    <div class="page-header"><h2>License</h2></div>
    <div class="page-body">
      <div class="card" style="margin-bottom: var(--space-4); border-color: var(--warning, #d97706);">
        <div class="card-body" style="border-left: 4px solid var(--warning, #d97706);">
          <strong>附加使用限制（Additional Terms）</strong>
          <p style="margin: var(--space-2) 0 0;">
            本人所有项目均禁止 联想(北京)有限公司 下 "Shrek Ding" 所属部门使用。
          </p>
          <p style="margin: var(--space-1) 0 0; color: var(--text-muted); font-size: var(--text-sm);">
            本限制为版权持有人依 AGPL-3.0 第 7 条附加的补充条款，对 AGPL 开源版与商业授权版均生效；全文见下方许可证尾部「附加条款」节。
          </p>
        </div>
      </div>
      <div class="card">
        <div class="card-body">
          <div v-if="loading" style="text-align: center; padding: var(--space-8);">
            <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
          </div>
          <div v-else-if="error" class="empty-state">
            <p>{{ error }}</p>
          </div>
          <div v-else class="markdown-body" v-html="renderedLicense"></div>
        </div>
      </div>
    </div>
  </div>
</template>
