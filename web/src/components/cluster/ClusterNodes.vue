<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import NodeCard from './NodeCard.vue'

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const nodes = ref<any[]>([])
const expandedId = ref<string | null>(null)
const filterRole = ref('')
const filterCap = ref('')
const filterStatus = ref('')
const showAddModal = ref(false)
const addForm = ref({ address: '', name: '', id: '', role: 'worker', category: '' })
const addSubmitting = ref(false)

let refreshTimer: ReturnType<typeof setInterval> | null = null

const allRoles = computed(() => [...new Set(nodes.value.map(n => n.role).filter(Boolean))])
const allCaps = computed(() => [...new Set(nodes.value.flatMap(n => n.capabilities || []))])

const filtered = computed(() => {
  return nodes.value.filter(n => {
    if (filterRole.value && n.role !== filterRole.value) return false
    if (filterCap.value && !(n.capabilities || []).includes(filterCap.value)) return false
    if (filterStatus.value === 'online' && !n.online) return false
    if (filterStatus.value === 'offline' && n.online) return false
    return true
  })
})

async function loadNodes() {
  try {
    const data = await request('cluster', 'nodes.list')
    if (data?.nodes) nodes.value = data.nodes
  } catch { /* backend not ready */ }
}

function toggleExpand(id: string) {
  expandedId.value = expandedId.value === id ? null : id
}

async function pingNode(id: string) {
  try {
    const res = await request('cluster', 'nodes.ping', { node_id: id })
    toast.success(res?.latency ? `Ping ${id}: ${res.latency}ms` : `Ping ${id}: OK`)
  } catch (e: any) {
    toast.error('Ping 失败: ' + (e || '未知错误'))
  }
}

async function removeNode(id: string) {
  try {
    await request('cluster', 'nodes.remove', { node_id: id })
    toast.success('节点已移除')
    await loadNodes()
  } catch (e: any) {
    toast.error('移除失败: ' + (e || '未知错误'))
  }
}

async function refreshNodeId(id: string) {
  try {
    const res = await request('cluster', 'nodes.refresh', { node_id: id })
    if (res?.upgraded_from_placeholder) {
      toast.success(`ID 已升级为真实 ID: ${res.canonical_id}`)
    } else if (res?.canonical_id) {
      toast.success(`已刷新: ${res.canonical_id}`)
    } else {
      toast.success('已刷新')
    }
    await loadNodes()
  } catch (e: any) {
    toast.error('刷新失败: ' + (e || '未知错误'))
  }
}

async function submitAddNode() {
  if (!addForm.value.address.trim()) {
    toast.warn('请输入节点地址')
    return
  }
  addSubmitting.value = true
  try {
    await request('cluster', 'nodes.add', {
      address: addForm.value.address.trim(),
      id: addForm.value.id.trim() || undefined,
      name: addForm.value.name.trim() || undefined,
      role: addForm.value.role || undefined,
      category: addForm.value.category.trim() || undefined,
    })
    toast.success('节点添加成功')
    showAddModal.value = false
    addForm.value = { address: '', name: '', id: '', role: 'worker', category: '' }
    await loadNodes()
  } catch (e: any) {
    toast.error('添加失败: ' + (e || '未知错误'))
  } finally {
    addSubmitting.value = false
  }
}

onMounted(async () => {
  await loadNodes()
  loading.value = false
  refreshTimer = setInterval(loadNodes, 10000)
})

onUnmounted(() => {
  if (refreshTimer) clearInterval(refreshTimer)
})
</script>

<template>
  <div v-if="loading" style="text-align:center;padding:var(--space-8)">
    <div class="spinner spinner-lg" style="margin:0 auto" />
  </div>

  <div v-if="!loading">
    <div style="display:flex;gap:var(--space-2);margin-bottom:var(--space-3);flex-wrap:wrap;align-items:center">
      <select class="form-select" v-model="filterRole" style="width:auto">
        <option value="">全部角色</option>
        <option v-for="r in allRoles" :key="r" :value="r">{{ r }}</option>
      </select>
      <select class="form-select" v-model="filterCap" style="width:auto">
        <option value="">全部能力</option>
        <option v-for="c in allCaps" :key="c" :value="c">{{ c }}</option>
      </select>
      <select class="form-select" v-model="filterStatus" style="width:auto">
        <option value="">全部状态</option>
        <option value="online">在线</option>
        <option value="offline">离线</option>
      </select>
      <div style="flex:1" />
      <button class="btn btn-sm btn-primary" @click="showAddModal = true">添加节点</button>
      <button class="btn btn-sm" @click="loadNodes">刷新</button>
    </div>

    <div v-if="!filtered.length" class="empty-state">
      <h3>暂无节点</h3>
      <p>启动集群节点后，它们会自动出现在这里</p>
    </div>

    <div v-for="node in filtered" :key="node.id">
      <div class="node-row" @click="toggleExpand(node.id)">
        <span class="node-dot" :class="node.online ? 'online' : 'offline'" />
        <span class="node-name-col">
          {{ node.name }}
          <span v-if="node.isLocal" class="badge badge-primary" style="font-size:var(--text-xs);margin-left:var(--space-1)">本节点</span>
          <span v-else-if="!node.id.startsWith('node-')" class="badge badge-warning" style="font-size:var(--text-xs);margin-left:var(--space-1)" title="点击右侧『刷新』按钮拉取远端真实 ID">占位 ID</span>
        </span>
        <span class="badge" :class="node.role === 'manager' ? 'badge-info' : 'badge-neutral'">{{ node.role }}</span>
        <span class="node-caps">{{ (node.capabilities || []).slice(0, 3).join(', ') }}</span>
        <span class="node-seen">{{ node.lastSeen || '--' }}</span>
        <span class="node-expand">{{ expandedId === node.id ? '▲' : '▼' }}</span>
      </div>
      <div v-if="expandedId === node.id" style="margin:var(--space-2) 0">
        <NodeCard :node="node" @ping="pingNode" @remove="removeNode" @refresh="refreshNodeId" />
      </div>
    </div>

    <!-- Add Node Modal -->
    <div v-if="showAddModal" class="modal-overlay" @click.self="showAddModal = false">
      <div class="modal">
        <div class="modal-header"><h3>添加节点</h3></div>
        <div class="modal-body">
          <div class="form-group">
            <label>地址 <span style="color:var(--error)">*</span></label>
            <input class="form-input" v-model="addForm.address" placeholder="例如 192.168.1.10:7900" />
          </div>
          <div class="form-group">
            <label>名称</label>
            <input class="form-input" v-model="addForm.name" placeholder="可选，留空则用 ID 或地址" />
          </div>
          <div class="form-group">
            <label>ID</label>
            <input class="form-input" v-model="addForm.id" placeholder="可选，留空则用名称或地址作占位" />
            <small style="color:var(--text-muted);font-size:var(--text-xs)">填写真实节点 ID 可立即与远端匹配；留空则作占位，待通信后自动更新</small>
          </div>
          <div class="form-group">
            <label>角色</label>
            <select class="form-select" v-model="addForm.role">
              <option value="worker">Worker</option>
              <option value="manager">Manager</option>
            </select>
          </div>
          <div class="form-group">
            <label>分类</label>
            <input class="form-input" v-model="addForm.category" placeholder="可选" />
          </div>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="showAddModal = false">取消</button>
          <button class="btn btn-primary" :disabled="addSubmitting" @click="submitAddNode">{{ addSubmitting ? '提交中...' : '确认添加' }}</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.node-row {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--border);
  cursor: pointer;
  font-size: var(--text-sm);
}
.node-row:hover { background: var(--surface-hover); }
.node-dot {
  width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0;
}
.node-dot.online { background: var(--success); }
.node-dot.offline { background: var(--text-muted); }
.node-name-col { font-weight: 600; min-width: 80px; }
.node-caps { flex: 1; color: var(--text-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.node-seen { color: var(--text-muted); font-size: var(--text-xs); min-width: 50px; text-align: right; }
.node-expand { color: var(--text-muted); font-size: 10px; }
</style>
