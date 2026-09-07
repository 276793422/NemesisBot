<script setup lang="ts">
/**
 * FileTreePanel — M4（2026-09-06，devtool-upgrade 阶段 5）。
 *
 * ChatPanel 左侧的工作区文件树（ChatView `.chat-page-layout` 左列）：
 * - 数据：WSAPI `fs.tree {path?, depth?=3}`（handlers/fs.rs，忽略表与
 *   @补全同源——node_modules/logs 等运行时目录永不出现）。
 * - 懒展开：根请求 depth=3 给概览；`children: null` 的目录（深度边界
 *   未加载）点击时再查 `{path, depth: 1}` 原位填充；`children: []` 的
 *   目录是真空（展开无内容）。
 * - 点击文件 → 输入框追加 `@相对路径 `（联动 I2 补全体系；尾部带空白
 *   所以不会误触发 @ 补全弹层）。
 * - 折叠态记忆 localStorage（同 TodoPanel 的全局开合偏好约定）；默认
 *   折叠成左侧细条，不打扰窄屏。
 */
import { computed, onMounted, ref, watch } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useChatStore } from '../../stores/chat'

interface TreeNode {
  name: string
  path: string
  type: 'dir' | 'file'
  /** null = 深度边界未加载（懒展开点）；[] = 真空目录。 */
  children?: TreeNode[] | null
}

const { request } = useWSAPI()
const chatStore = useChatStore()

/** 默认折叠（localStorage '0' = 用户偏好展开）；'1'/缺省都折叠。 */
const collapsed = ref(localStorage.getItem('nb_filetree_collapsed') !== '0')
const entries = ref<TreeNode[]>([])
const truncated = ref(false)
const error = ref('')
const loading = ref(false)
/** 本地展开的目录路径集合。 */
const expanded = ref(new Set<string>())
const loadingDirs = ref(new Set<string>())
const loadedOnce = ref(false)

function toggleCollapsed() {
  collapsed.value = !collapsed.value
  localStorage.setItem('nb_filetree_collapsed', collapsed.value ? '1' : '0')
}

async function loadRoot() {
  if (loading.value) return
  loading.value = true
  error.value = ''
  try {
    const out = await request('fs', 'tree', {})
    entries.value = (out?.entries as TreeNode[] | undefined) ?? []
    truncated.value = !!(out?.truncated)
    loadedOnce.value = true
  } catch (e: any) {
    error.value = e?.message ?? String(e)
  } finally {
    loading.value = false
  }
}

watch(collapsed, (isCollapsed) => {
  if (!isCollapsed && !loadedOnce.value) loadRoot()
})

onMounted(() => {
  if (!collapsed.value && !loadedOnce.value) loadRoot()
})

function toggleDir(node: TreeNode) {
  if (node.type !== 'dir') return
  if (expanded.value.has(node.path)) {
    const next = new Set(expanded.value)
    next.delete(node.path)
    expanded.value = next
    return
  }
  const open = () => {
    const next = new Set(expanded.value)
    next.add(node.path)
    expanded.value = next
  }
  if (node.children === null) {
    // 深度边界未加载——懒展开再查一层。
    if (loadingDirs.value.has(node.path)) return
    loadingDirs.value = new Set(loadingDirs.value).add(node.path)
    request('fs', 'tree', { path: node.path, depth: 1 })
      .then((out) => {
        node.children = (out?.entries as TreeNode[] | undefined) ?? []
        open()
      })
      .catch((e) => {
        error.value = e?.message ?? String(e)
      })
      .finally(() => {
        const next = new Set(loadingDirs.value)
        next.delete(node.path)
        loadingDirs.value = next
      })
  } else {
    open()
  }
}

interface Row {
  node: TreeNode
  depth: number
}

/** 展开态投影成可见行（只走 expanded 的目录）。 */
const visibleRows = computed<Row[]>(() => {
  const rows: Row[] = []
  const walk = (list: TreeNode[], depth: number) => {
    for (const n of list) {
      rows.push({ node: n, depth })
      if (n.type === 'dir' && expanded.value.has(n.path) && Array.isArray(n.children)) {
        walk(n.children, depth + 1)
      }
    }
  }
  walk(entries.value, 0)
  return rows
})

/** 点击文件 → 输入框追加 `@path `（尾部空白不误触 @ 补全弹层）。 */
function insertRef(node: TreeNode) {
  const cur = chatStore.input
  const sep = cur && !/\s$/.test(cur) ? ' ' : ''
  chatStore.input = cur + sep + '@' + node.path + ' '
}
</script>

<template>
  <div class="filetree-wrap" :class="{ collapsed }">
    <button
      v-if="collapsed"
      class="filetree-rail"
      type="button"
      title="展开工作区文件树"
      @click="toggleCollapsed"
    >📁</button>
    <div v-else class="filetree-panel">
      <div class="filetree-header">
        <span class="filetree-title">📁 工作区</span>
        <span class="filetree-spacer" />
        <button class="filetree-btn" type="button" title="刷新" @click="loadRoot">⟳</button>
        <button class="filetree-btn" type="button" title="折叠" @click="toggleCollapsed">⟨</button>
      </div>

      <div v-if="error" class="filetree-error">
        {{ error }}
        <button class="filetree-btn" type="button" @click="loadRoot">重试</button>
      </div>
      <div v-else-if="loading && !loadedOnce" class="filetree-loading">
        <span class="spinner" style="width:14px;height:14px;border-width:2px;"></span> 加载中...
      </div>
      <div v-else class="filetree-body">
        <div v-if="truncated" class="filetree-truncated">条目过多，仅显示前 500 项（可展开子目录查看）</div>
        <div
          v-for="row in visibleRows"
          :key="row.node.path"
          class="filetree-row"
          :class="{ dir: row.node.type === 'dir' }"
          :style="{ paddingLeft: 8 + row.depth * 14 + 'px' }"
          :title="row.node.path"
          @click="row.node.type === 'dir' ? toggleDir(row.node) : insertRef(row.node)"
        >
          <span v-if="row.node.type === 'dir'" class="filetree-caret">
            {{ loadingDirs.has(row.node.path) ? '…' : (expanded.has(row.node.path) ? '▾' : '▸') }}
          </span>
          <span v-else class="filetree-caret filetree-caret-file">·</span>
          <span class="filetree-name">{{ row.node.name }}</span>
        </div>
        <div v-if="!loading && loadedOnce && visibleRows.length === 0" class="filetree-empty">
          工作区为空
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.filetree-wrap {
  display: flex;
  min-width: 0;
  min-height: 0;
}
.filetree-rail {
  width: 26px;
  border: none;
  border-right: 1px solid var(--border);
  background: var(--bg-elev);
  color: var(--text-muted);
  cursor: pointer;
  font-size: 14px;
  padding: 8px 0;
  align-self: stretch;
}
.filetree-rail:hover {
  color: var(--text);
}
.filetree-panel {
  display: flex;
  flex-direction: column;
  width: 220px;
  min-height: 0;
  border-right: 1px solid var(--border);
  background: var(--bg-elev);
  overflow: hidden;
}
.filetree-header {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 6px 8px;
  border-bottom: 1px solid var(--border);
  font-size: var(--text-xs, 12px);
  color: var(--text-muted);
}
.filetree-title {
  font-weight: 600;
}
.filetree-spacer {
  flex: 1;
}
.filetree-btn {
  border: none;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 12px;
  padding: 2px 4px;
  border-radius: 4px;
}
.filetree-btn:hover {
  color: var(--text);
  background: var(--bg, rgba(128, 128, 128, 0.12));
}
.filetree-body {
  flex: 1;
  overflow-y: auto;
  padding: 4px 0;
}
.filetree-row {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 2px 6px 2px 8px;
  font-size: var(--text-xs, 12px);
  color: var(--text);
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
}
.filetree-row:hover {
  background: var(--bg, rgba(128, 128, 128, 0.12));
}
.filetree-caret {
  width: 12px;
  flex: none;
  text-align: center;
  color: var(--text-muted);
  font-size: 10px;
}
.filetree-caret-file {
  visibility: hidden;
}
.filetree-name {
  overflow: hidden;
  text-overflow: ellipsis;
}
.filetree-error,
.filetree-loading,
.filetree-empty,
.filetree-truncated {
  padding: 8px;
  font-size: var(--text-xs, 12px);
  color: var(--text-muted);
}
.filetree-error {
  color: var(--danger, #e05252);
}
.filetree-truncated {
  border-bottom: 1px dashed var(--border);
}
</style>
