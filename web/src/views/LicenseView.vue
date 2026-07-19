<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { marked } from 'marked'

defineProps<{ embedded?: boolean }>()

const licenseContent = ref('')
const loading = ref(true)
const error = ref('')

onMounted(async () => {
  try {
    const resp = await fetch('/api/system/license')
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
  return marked.parse(licenseContent.value, {
    gfm: true,
    breaks: false,
  })
})
</script>

<template>
  <div :class="embedded ? 'license-embed' : 'page-license'">
    <div v-if="!embedded" class="page-header"><h2>License</h2></div>
    <div :class="embedded ? '' : 'page-body'">
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
