<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

interface DocInfo { name: string; exists: boolean; instruction_chain?: boolean }

const docs = ref<DocInfo[]>([])
const activeDoc = ref('')
const docContent = ref('')
const editing = ref(false)
const editContent = ref('')
const loading = ref(true)

async function loadDocs() {
  try {
    const data = await request('identity', 'list')
    docs.value = data?.documents || []
  } catch (e: any) {
    toast.error('加载失败: ' + e)
  }
  loading.value = false
}

let loadSeq = 0
// docContent 实际所属的文档 —— 保存安全性的真相源（不变量：只允许把
// contentDoc 的内容保存回 contentDoc；activeDoc 只是「正在查看」的 tab）。
const contentDoc = ref('')

async function loadDoc(name: string) {
  const seq = ++loadSeq
  activeDoc.value = name
  editing.value = false
  // G5：缺失文档直接呈现空内容（保存即创建），不打后端（get 对缺失文件报错）。
  const info = docs.value.find(d => d.name === name)
  if (info && info.exists === false) {
    docContent.value = ''
    contentDoc.value = name
    return
  }
  try {
    const data = await request('identity', 'get', { name })
    if (seq !== loadSeq) return // 已被更新的切换取代（乱序响应），不得覆盖
    docContent.value = data?.content || ''
    contentDoc.value = name
  } catch (e: any) {
    if (seq !== loadSeq) return
    // 读取失败：回退到内容实际所属的文档（contentDoc），恢复
    // docContent ↔ activeDoc 一致 —— 否则旧内容挂在新标题下，
    // 编辑保存会把旧文档写进新文档（跨文档覆盖）。
    activeDoc.value = contentDoc.value
    toast.error('读取失败: ' + e)
  }
}

function startEdit() {
  editContent.value = docContent.value
  editing.value = true
}

async function saveDoc() {
  // 内容不属于当前文档（加载中 / 失败回退窗口）→ 拒绝保存，
  // 从源头杜绝把 A 文档的内容写进 B 文档。
  if (contentDoc.value !== activeDoc.value) {
    toast.error('文档内容尚未加载完成，已取消保存')
    return
  }
  try {
    await request('identity', 'save', { name: activeDoc.value, content: editContent.value })
    toast.success('已保存')
    docContent.value = editContent.value
    editing.value = false
    // 新建文件后刷新 exists 标志。
    const info = docs.value.find(d => d.name === activeDoc.value)
    if (info && !info.exists) {
      await loadDocs()
    }
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  }
}

const docLabels: Record<string, string> = {
  'AGENT.md': '行为指南',
  'IDENTITY.md': '身份定义',
  'SOUL.md': '核心原则',
  'USER.md': '用户偏好',
  'AGENTS.md': '指令链 AGENTS',
  'CLAUDE.md': '指令链 CLAUDE',
}

// G5：指令链文档徽标 —— 与人格四件套的注入机制不同（每轮重建注入）。
const activeInfo = computed(() => docs.value.find(d => d.name === activeDoc.value))
const activeIsChain = computed(() => activeInfo.value?.instruction_chain === true)

onMounted(async () => {
  await loadDocs()
  if (docs.value.length > 0) {
    await loadDoc(docs.value[0].name)
  }
})
</script>

<template>
  <div class="page-identity">
    <div class="page-header"><h2>身份管理</h2></div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <div class="tabs">
          <button v-for="d in docs" :key="d.name" class="tab" :class="{ active: activeDoc === d.name }" @click="loadDoc(d.name)">
            {{ docLabels[d.name] || d.name }}<span v-if="d.instruction_chain" class="chain-dot" title="指令链 · 每轮注入">⛓</span>
          </button>
        </div>

        <div class="card">
          <div class="card-header">
            <h3>
              {{ activeDoc }} — {{ docLabels[activeDoc] || '文档' }}
              <!-- G5：指令链徽标（注入机制区别于人格四件套） -->
              <span v-if="activeIsChain" class="chain-badge" title="本文件属于 AGENTS/CLAUDE 指令链：每轮对话注入 system-reminder，工具触及链上文件即自动重读">⛓ 指令链 · 每轮注入</span>
              <span v-if="activeInfo && activeInfo.exists === false" class="chain-badge chain-badge--new" title="该文件尚未创建；点击「编辑」写入内容后保存即创建">未创建 · 保存即创建</span>
            </h3>
            <div style="display: flex; gap: var(--space-2);">
              <template v-if="!editing">
                <button class="btn btn-sm" @click="startEdit">编辑</button>
              </template>
              <template v-else>
                <button class="btn btn-sm" @click="editing = false">取消</button>
                <button class="btn btn-sm btn-primary" @click="saveDoc">保存</button>
              </template>
            </div>
          </div>
          <div class="card-body">
            <div v-if="editing">
              <textarea class="form-textarea" style="min-height: 60vh; font-family: var(--font-mono); font-size: var(--text-sm);" v-model="editContent"></textarea>
            </div>
            <div v-else class="markdown-body" style="max-height: 65vh; overflow-y: auto;">
              <pre style="white-space: pre-wrap; word-break: break-word;">{{ docContent || '（空文件）' }}</pre>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* G5：指令链徽标（tab 圆点 + 标题徽章） */
.chain-dot {
  margin-left: 4px;
  font-size: 11px;
  opacity: 0.9;
}

.chain-badge {
  display: inline-block;
  margin-left: var(--space-2);
  padding: 1px 8px;
  border-radius: var(--radius-sm);
  font-size: 11px;
  font-weight: 600;
  background: rgba(139, 92, 246, 0.15);
  color: #7c3aed;
  vertical-align: middle;
}

.chain-badge--new {
  background: rgba(245, 158, 11, 0.15);
  color: #d97706;
}
</style>
