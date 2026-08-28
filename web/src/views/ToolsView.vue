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

// G6 (Phase4-a / Y1)：工具文档折叠开关（agents.tool_doc_folding）。
// 后端 current_tool_doc_folding() 每轮实时读 config.json → 改动即时生效。
const foldingEnabled = ref(false)
const foldingTopN = ref(8)
const foldingLoaded = ref(false)
const foldingSaving = ref(false)
const topNEditing = ref(false)
const topNDraft = ref('')

async function loadFolding() {
  try {
    const data = await request('config', 'get')
    foldingEnabled.value = data?.agents?.tool_doc_folding?.enabled === true
    const n = Number(data?.agents?.tool_doc_folding?.expand_top_n)
    foldingTopN.value = Number.isFinite(n) && n > 0 ? n : 8
    foldingLoaded.value = true
  } catch {
    foldingLoaded.value = false
  }
}

async function setFolding(enabled: boolean) {
  if (foldingSaving.value) return
  foldingSaving.value = true
  try {
    await request('config', 'set_field', { path: 'agents.tool_doc_folding.enabled', value: enabled })
    foldingEnabled.value = enabled
    toast.success(enabled ? '工具文档折叠已开启（即时生效）' : '工具文档折叠已关闭')
  } catch (e: any) {
    toast.error('设置失败: ' + e)
  }
  foldingSaving.value = false
}

async function saveTopN() {
  const n = Number(topNDraft.value)
  if (!Number.isInteger(n) || n < 1) {
    toast.error('expand_top_n 必须是正整数')
    return
  }
  foldingSaving.value = true
  try {
    await request('config', 'set_field', { path: 'agents.tool_doc_folding.expand_top_n', value: n })
    foldingTopN.value = n
    topNEditing.value = false
    toast.success(`已保存：保留最相似 ${n} 个工具的完整描述`)
  } catch (e: any) {
    toast.error('设置失败: ' + e)
  }
  foldingSaving.value = false
}

onMounted(() => {
  loadTools()
  loadFolding()
})
</script>

<template>
  <div class="page-tools">
    <div class="page-header"><h2>Tools</h2></div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading">
        <!-- G6：工具文档折叠开关（config.agents.tool_doc_folding；每轮实时读 → 即时生效） -->
        <div class="card folding-card">
          <div class="card-body folding-row">
            <div class="folding-info">
              <h3>🧬 工具文档折叠</h3>
              <p class="folding-desc">
                开启后只保留与当前对话最相似的 {{ foldingTopN }} 个工具的完整描述，其余折叠为单行摘要 ——
                所有工具仍可调用（仅描述文本变化，参数 schema 不变）。mini 能力档（核心 13 工具）不参与；
                需要已配置嵌入后端（增强记忆 embed），否则自动退回完整描述。
              </p>
              <p class="folding-note">每轮对话前实时读取 config.json —— 改动即时生效，无需重启。默认关闭。</p>
            </div>
            <div class="folding-controls">
              <span v-if="!foldingLoaded" class="folding-unavailable">配置读取不可用</span>
              <template v-else>
                <div class="toggle" :class="{ active: foldingEnabled }" @click="setFolding(!foldingEnabled)"></div>
                <span class="folding-state">{{ foldingEnabled ? '已开启' : '已关闭' }}</span>
                <template v-if="foldingEnabled">
                  <template v-if="!topNEditing">
                    <span class="folding-topn">保留 {{ foldingTopN }} 个</span>
                    <button class="btn btn-sm" @click="topNDraft = String(foldingTopN); topNEditing = true">调整</button>
                  </template>
                  <template v-else>
                    <input class="form-input folding-topn-input" type="number" min="1" v-model="topNDraft" />
                    <button class="btn btn-sm btn-primary" :disabled="foldingSaving" @click="saveTopN">保存</button>
                    <button class="btn btn-sm" @click="topNEditing = false">取消</button>
                  </template>
                </template>
              </template>
            </div>
          </div>
        </div>

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

<style scoped>
/* G6：工具文档折叠开关卡 */
.folding-card {
  margin-bottom: var(--space-4);
  border-left: 3px solid var(--accent);
}

.folding-row {
  display: flex;
  align-items: flex-start;
  gap: var(--space-4);
}

.folding-info { flex: 1; }

.folding-info h3 { margin: 0 0 var(--space-1); font-size: var(--text-base); }

.folding-desc {
  color: var(--text-secondary);
  font-size: var(--text-sm);
  margin: 0 0 var(--space-1);
}

.folding-note {
  color: var(--text-muted);
  font-size: var(--text-xs);
  margin: 0;
}

.folding-controls {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  white-space: nowrap;
}

.folding-state { font-size: var(--text-sm); color: var(--text-secondary); }

.folding-topn { font-size: var(--text-sm); color: var(--text-muted); }

.folding-topn-input { width: 90px; }

.folding-unavailable { color: var(--text-muted); font-size: var(--text-sm); }
</style>
