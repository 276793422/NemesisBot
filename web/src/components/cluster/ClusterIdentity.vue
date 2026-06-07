<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const saving = ref(false)

// Identity fields
const nodeId = ref('')
const nodeName = ref('')
const nodeRole = ref('worker')
const nodeCategory = ref('')
const nodeType = ref('')
const tags = ref<string[]>([])
const capabilities = ref<string[]>([])

// Tag input
const tagInput = ref('')

// Personality files
const identityContent = ref('')
const soulContent = ref('')
const activeFile = ref('IDENTITY.md')

async function loadIdentity() {
  try {
    const [config, files] = await Promise.all([
      request('cluster', 'config.get'),
      request('cluster', 'identity.get_files'),
    ])
    if (config) {
      nodeId.value = config.node_id ?? ''
      nodeName.value = config.name ?? ''
      nodeRole.value = config.role ?? 'worker'
      nodeCategory.value = config.category ?? ''
      nodeType.value = config.node_type ?? ''
      tags.value = config.tags ?? []
      capabilities.value = config.capabilities ?? []
    }
    if (files) {
      identityContent.value = files.identity ?? ''
      soulContent.value = files.soul ?? ''
    }
  } catch { /* ignore */ }
}

async function saveIdentity() {
  saving.value = true
  try {
    await request('cluster', 'node.update_identity', {
      name: nodeName.value,
      role: nodeRole.value,
      category: nodeCategory.value,
      tags: tags.value,
    })
    toast.success('节点身份已更新')
  } catch (e: any) {
    toast.error('更新失败: ' + (e || '未知错误'))
  }
  saving.value = false
}

function addTag() {
  const input = tagInput.value.trim()
  if (!input) return
  const newTags = input.split(',').map(t => t.trim()).filter(t => t && !tags.value.includes(t))
  if (newTags.length) {
    tags.value.push(...newTags)
  }
  tagInput.value = ''
}

function removeTag(index: number) {
  tags.value.splice(index, 1)
}

function onTagKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' || e.key === ',') {
    e.preventDefault()
    addTag()
  }
}

onMounted(async () => {
  await loadIdentity()
  loading.value = false
})
</script>

<template>
  <div v-if="loading" style="text-align:center;padding:var(--space-8)">
    <div class="spinner spinner-lg" style="margin:0 auto" />
  </div>

  <div v-if="!loading">
    <div class="card" style="margin-bottom:var(--space-4)">
      <div class="card-header"><h3>节点身份</h3></div>
      <div class="card-body">
        <div class="form-group">
          <label class="form-label">Node ID</label>
          <input class="form-input" :value="nodeId" readonly style="width:360px;font-family:var(--font-mono);font-size:var(--text-xs);opacity:0.7;cursor:default" />
        </div>
        <div class="form-group">
          <label class="form-label">
            节点名称
            <span class="form-hint" title="其他节点发现你时显示的名称。修改后立即生效，并持久化到配置文件。">ⓘ</span>
          </label>
          <input class="form-input" type="text" v-model="nodeName" style="width:240px" placeholder="例：Bot-Alpha" />
        </div>
        <div class="form-group">
          <label class="form-label">
            节点角色
            <span class="form-hint" title="manager 可调度任务，worker 执行任务。修改后立即生效。">ⓘ</span>
          </label>
          <select class="form-input" v-model="nodeRole" style="width:240px">
            <option value="worker">worker（执行者）</option>
            <option value="manager">manager（管理者）</option>
          </select>
        </div>
        <div class="form-group">
          <label class="form-label">
            节点分类
            <span class="form-hint" title="用于任务路由和节点分组，如 development、production。修改后立即生效。">ⓘ</span>
          </label>
          <input class="form-input" type="text" v-model="nodeCategory" style="width:240px" placeholder="例：development" />
        </div>
        <div class="form-group">
          <label class="form-label">节点类型</label>
          <div>
            <span class="badge badge-neutral">{{ nodeType || 'agent' }}</span>
            <span style="color:var(--text-muted);font-size:var(--text-xs);margin-left:var(--space-2)">定义节点架构能力，运行时不可修改</span>
          </div>
        </div>
        <div class="form-group">
          <label class="form-label">
            标签
            <span class="form-hint" title="自定义标签，用于节点分类和过滤。Enter 或逗号添加。">ⓘ</span>
          </label>
          <div style="display:flex;flex-wrap:wrap;gap:var(--space-2);align-items:center">
            <span v-for="(tag, i) in tags" :key="i" class="badge" style="display:inline-flex;align-items:center;gap:var(--space-1)">
              {{ tag }}
              <button style="background:none;border:none;cursor:pointer;color:var(--text-muted);padding:0;line-height:1;font-size:var(--text-xs)" @click="removeTag(i)">&times;</button>
            </span>
          </div>
          <input class="form-input" type="text" v-model="tagInput" style="width:240px;margin-top:var(--space-2)" placeholder="输入标签后按 Enter 添加" @keydown="onTagKeydown" />
        </div>
        <div class="form-group" v-if="capabilities.length">
          <label class="form-label">能力</label>
          <div style="display:flex;flex-wrap:wrap;gap:var(--space-2)">
            <span v-for="cap in capabilities" :key="cap" class="badge badge-neutral" style="opacity:0.7">{{ cap }}</span>
          </div>
          <div style="color:var(--text-muted);font-size:var(--text-xs);margin-top:var(--space-1)">由 AgentLoop 工具注册自动设置</div>
        </div>
        <div style="display:flex;gap:var(--space-2);margin-top:var(--space-4)">
          <button class="btn btn-primary" :disabled="saving" @click="saveIdentity">
            {{ saving ? '保存中...' : '更新身份' }}
          </button>
        </div>
      </div>
    </div>

    <div class="card">
      <div class="card-header">
        <h3>人格文件</h3>
        <div style="display:flex;gap:var(--space-1)">
          <button class="btn btn-sm" :class="{ 'btn-primary': activeFile === 'IDENTITY.md' }" @click="activeFile = 'IDENTITY.md'">集群身份</button>
          <button class="btn btn-sm" :class="{ 'btn-primary': activeFile === 'SOUL.md' }" @click="activeFile = 'SOUL.md'">集群行为准则</button>
        </div>
      </div>
      <div class="card-body">
        <div v-if="activeFile === 'IDENTITY.md'" style="background:var(--surface-alt);border-radius:var(--radius-md);padding:var(--space-4);max-height:400px;overflow:auto">
          <pre v-if="identityContent" style="white-space:pre-wrap;word-break:break-word;margin:0;font-size:var(--text-sm);font-family:var(--font-mono)">{{ identityContent }}</pre>
          <div v-else class="empty-state"><p>暂无 IDENTITY.md 文件</p></div>
        </div>
        <div v-else style="background:var(--surface-alt);border-radius:var(--radius-md);padding:var(--space-4);max-height:400px;overflow:auto">
          <pre v-if="soulContent" style="white-space:pre-wrap;word-break:break-word;margin:0;font-size:var(--text-sm);font-family:var(--font-mono)">{{ soulContent }}</pre>
          <div v-else class="empty-state"><p>暂无 SOUL.md 文件</p></div>
        </div>
      </div>
    </div>
  </div>
</template>
