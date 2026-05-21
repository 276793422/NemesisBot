<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const content = ref('')
const editing = ref(false)
const editContent = ref('')
const loading = ref(true)

async function loadTools() {
  try {
    const data = await request('tools', 'get')
    content.value = data?.content || ''
  } catch (e: any) {
    toast.error('加载失败: ' + e)
  }
  loading.value = false
}

function startEdit() {
  editContent.value = content.value
  editing.value = true
}

async function saveTools() {
  try {
    await request('tools', 'save', { content: editContent.value })
    toast.success('已保存')
    content.value = editContent.value
    editing.value = false
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

onMounted(loadTools)
</script>

<template>
  <div class="page-tools">
    <div class="page-header"><h2>Tools</h2></div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <div class="card">
          <div class="card-header">
            <h3>TOOLS.md — 本地工具笔记</h3>
            <div style="display: flex; gap: var(--space-2);">
              <template v-if="!editing">
                <button class="btn btn-sm" @click="startEdit">编辑</button>
              </template>
              <template v-else>
                <button class="btn btn-sm" @click="editing = false">取消</button>
                <button class="btn btn-sm btn-primary" @click="saveTools">保存</button>
              </template>
            </div>
          </div>
          <div class="card-body">
            <p style="color: var(--text-muted); font-size: var(--text-sm); margin-bottom: var(--space-4);">
              此文件用于记录本地环境特有信息，如摄像头名称、SSH 别名、TTS 偏好、扬声器名称、设备昵称等。Agent 运行时可以读取这些信息。
            </p>
            <div v-if="editing">
              <textarea class="form-textarea" style="min-height: 60vh; font-family: var(--font-mono); font-size: var(--text-sm);" v-model="editContent"></textarea>
            </div>
            <div v-else class="markdown-body">
              <pre style="white-space: pre-wrap; word-break: break-word;">{{ content || '（空文件 — 点击编辑添加工具使用笔记）' }}</pre>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
