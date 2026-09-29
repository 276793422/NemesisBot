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
// 职能框架 M6：自报职能（看板派发匹配的硬条件数据源）+ 节点档位。
const professions = ref<string[]>([])
const nodeTier = ref('')

// 内置职能目录（与后端 nemesis-prompts::professions::meta::CATALOG 一致；
// min_tier 标注派发闸要求——声明 architecture/dev:cpp 的节点需要 big 档）。
const PROFESSION_CATALOG = [
  { slug: 'product', label: '产品经理', minTier: 'normal' },
  { slug: 'ui-design', label: 'UI 设计', minTier: 'normal' },
  { slug: 'architecture', label: '架构师', minTier: 'big' },
  { slug: 'dev', label: '开发工程师', minTier: 'normal' },
  { slug: 'dev:cpp', label: '开发（C/C++）', minTier: 'big' },
  { slug: 'test-whitebox', label: '白盒测试开发', minTier: 'normal' },
  { slug: 'test-blackbox', label: '黑盒测试', minTier: 'normal' },
]

function toggleProfession(slug: string) {
  const i = professions.value.indexOf(slug)
  if (i >= 0) professions.value.splice(i, 1)
  else professions.value.push(slug)
}

// Tag input
const tagInput = ref('')

async function loadIdentity() {
  try {
    const config = await request('cluster', 'config.get')
    if (config) {
      nodeId.value = config.node_id ?? ''
      nodeName.value = config.name ?? ''
      nodeRole.value = config.role ?? 'worker'
      nodeCategory.value = config.category ?? ''
      nodeType.value = config.node_type ?? ''
      tags.value = config.tags ?? []
      capabilities.value = config.capabilities ?? []
    }
  } catch { /* ignore */ }
  // 职能/档位：nodes.list 的本机行（行带 professions/tier，职能框架 M6）。
  try {
    const r = await request('cluster', 'nodes.list', {})
    const me = (r?.nodes || []).find((n: any) => n.isLocal)
    if (me) {
      professions.value = me.professions ?? []
      nodeTier.value = me.tier ?? ''
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
      professions: professions.value,
      tier: nodeTier.value || null,
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
    <div class="card">
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
        <div class="form-group">
          <label class="form-label">
            职能
            <span class="form-hint" title="自报职能清单：看板派发的硬匹配条件——planner 标注 required_profession 的子单只派给声明该职能的节点。architecture/dev:cpp 要求 big 档。下一轮 announce 生效并持久化。">ⓘ</span>
          </label>
          <div style="display:flex;flex-wrap:wrap;gap:var(--space-2)">
            <button
              v-for="p in PROFESSION_CATALOG"
              :key="p.slug"
              type="button"
              class="btn btn-sm"
              :class="{ 'btn-primary': professions.includes(p.slug) }"
              :title="'最低档位要求：' + p.minTier"
              @click="toggleProfession(p.slug)"
            >{{ professions.includes(p.slug) ? '✓ ' : '' }}{{ p.label }}（{{ p.slug }}）</button>
          </div>
          <div v-if="professions.length" style="margin-top:var(--space-2);display:flex;flex-wrap:wrap;gap:var(--space-2);align-items:center">
            <span class="muted" style="font-size:var(--text-xs)">当前声明：</span>
            <span v-for="s in professions" :key="s" class="badge badge-info">🛠 {{ s }}</span>
          </div>
        </div>
        <div class="form-group">
          <label class="form-label">
            节点档位
            <span class="form-hint" title="看板派发的 tier 闸：requirement 最低档位高于本节点档位时不派。留空 = 按声明职能自动推断（catalog min_tier 最大值）。">ⓘ</span>
          </label>
          <select class="form-input" v-model="nodeTier" style="width:240px">
            <option value="">auto（按声明职能自动推断）</option>
            <option value="mini">mini（小模型档）</option>
            <option value="normal">normal（中模型档）</option>
            <option value="big">big（大模型档）</option>
          </select>
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
  </div>
</template>
