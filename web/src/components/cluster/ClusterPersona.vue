<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const saving = ref(false)

const files = ref<Record<string, string>>({})
const activeFile = ref('IDENTITY.md')

const allFiles = ['IDENTITY.md', 'SOUL.md']
const fileLabels: Record<string, string> = {
  'IDENTITY.md': '身份',
  'SOUL.md': '灵魂',
}

const currentContent = computed(() => files.value[activeFile.value] ?? '')

async function loadFiles() {
  loading.value = true
  try {
    const data = await request('cluster', 'identity.get_files')
    if (data) {
      files.value = {
        'IDENTITY.md': data.identity ?? '',
        'SOUL.md': data.soul ?? '',
      }
    }
  } catch {
    toast.error('加载人格文件失败')
  }
  loading.value = false
}

async function saveFile() {
  saving.value = true
  try {
    await request('cluster', 'identity.save_file', {
      file: activeFile.value,
      content: currentContent.value,
    })
    toast.success(`${activeFile.value} 已保存`)
  } catch (e: any) {
    toast.error('保存失败: ' + (e.message || e))
  }
  saving.value = false
}

onMounted(loadFiles)
</script>

<template>
  <div class="cluster-persona">
    <div v-if="loading" style="display:flex;align-items:center;justify-content:center;padding:var(--space-10)">
      <div class="spinner" style="width:32px;height:32px;"></div>
    </div>
    <template v-else>
      <div class="card">
        <div class="card-header">
          <h3>集群人格文件</h3>
          <div style="display:flex;gap:var(--space-1)">
            <button
              v-for="f in allFiles" :key="f"
              class="btn btn-sm"
              :class="{ 'btn-primary': activeFile === f }"
              @click="activeFile = f"
            >{{ fileLabels[f] || f }}</button>
          </div>
        </div>
        <div class="card-body" style="display:flex;flex-direction:column;gap:var(--space-3);">
          <div style="display:flex;align-items:center;justify-content:space-between;">
            <span style="font-size:var(--text-sm);color:var(--text-muted);">{{ activeFile }}</span>
            <button class="btn btn-primary btn-sm" :disabled="saving" @click="saveFile">
              {{ saving ? '保存中...' : '保存' }}
            </button>
          </div>
          <textarea
            class="form-textarea persona-editor"
            v-model="files[activeFile]"
            :placeholder="`${activeFile} 内容为空，点击此处开始编辑...`"
          ></textarea>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.cluster-persona {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
}

.persona-editor {
  width: 100%;
  min-height: 60vh;
  padding: var(--space-4);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface-alt);
  color: var(--text);
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  line-height: 1.6;
  resize: vertical;
  outline: none;
  tab-size: 2;
}

.persona-editor:focus {
  border-color: var(--accent);
}
</style>
