<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { useBoardChanged } from '../../composables/useBoardChanged'
import { fmtTime } from './boardMeta'

// 项目面板（W2 P3）：项目列表 + 创建 + 编辑（字段级 patch）+ 归档/恢复。
// 归档 = project.update status="archived"（软删除；后端 project.update）。

const { request } = useWSAPI()
const toast = useToast()

interface Project {
  id: number
  name: string
  description: string
  status: string
  icon: string
  created_at: number
}

const loading = ref(true)
const projects = ref<Project[]>([])

const showCreate = ref(false)
const busy = ref(false)
const createForm = ref({ name: '', description: '', icon: '' })

// 编辑弹窗（改名/描述/图标）。
const editing = ref<Project | null>(null)
const editForm = ref({ name: '', description: '', icon: '' })

async function load(silent = false) {
  if (!silent) loading.value = true
  try {
    const r = await request('board', 'project.list', {})
    projects.value = r?.projects || []
  } catch (e: any) {
    if (silent) console.warn('[ProjectPanel] silent refresh failed:', e)
    else toast.error('加载项目失败: ' + e)
  } finally {
    loading.value = false
  }
}

async function submitCreate() {
  if (!createForm.value.name.trim()) {
    toast.warn('请填写项目名')
    return
  }
  busy.value = true
  try {
    await request('board', 'project.create', {
      name: createForm.value.name.trim(),
      description: createForm.value.description,
      icon: createForm.value.icon.trim(),
    })
    toast.success('已创建项目')
    showCreate.value = false
    await load()
  } catch (e: any) {
    toast.error('创建失败: ' + e)
  } finally {
    busy.value = false
  }
}

function openEdit(p: Project) {
  editing.value = p
  editForm.value = { name: p.name, description: p.description || '', icon: p.icon || '' }
}

async function submitEdit() {
  if (!editing.value) return
  if (!editForm.value.name.trim()) {
    toast.warn('项目名不能为空')
    return
  }
  busy.value = true
  try {
    await request('board', 'project.update', {
      id: editing.value.id,
      name: editForm.value.name.trim(),
      description: editForm.value.description,
      icon: editForm.value.icon.trim(),
    })
    toast.success('已更新项目')
    editing.value = null
    await load()
  } catch (e: any) {
    toast.error('更新失败: ' + e)
  } finally {
    busy.value = false
  }
}

async function setStatus(p: Project, status: 'active' | 'archived') {
  try {
    await request('board', 'project.update', { id: p.id, status })
    toast.success(status === 'archived' ? '已归档' : '已恢复')
    await load()
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

onMounted(load)
// board-changed 推送：项目被其他入口（CLI/集群/autopilot）改动时静默换新。
useBoardChanged(() => load(true))
</script>

<template>
  <div>
    <div class="panel-toolbar">
      <button class="btn btn-primary" @click="showCreate = true">+ 新建项目</button>
      <span class="muted">共 {{ projects.length }} 个项目（归档 {{ projects.filter((p) => p.status === 'archived').length }}）</span>
    </div>

    <div v-if="loading" style="text-align: center; padding: var(--space-8);">
      <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
    </div>

    <div v-else-if="projects.length === 0" class="empty-state">
      <h3>暂无项目</h3>
      <p>项目用于给 Issue 分组；创建后可在新建 Issue 时选择归属</p>
    </div>

    <div v-else class="table-wrap">
      <table>
        <thead>
          <tr><th>项目</th><th>描述</th><th>状态</th><th>创建时间</th><th style="width: 200px;">操作</th></tr>
        </thead>
        <tbody>
          <tr v-for="p in projects" :key="p.id">
            <td style="font-weight: 500;">{{ p.icon }} {{ p.name }}</td>
            <td style="font-size: var(--text-sm); color: var(--text-muted);">{{ p.description || '—' }}</td>
            <td>
              <span class="badge" :class="p.status === 'archived' ? 'badge-neutral' : 'badge-success'">
                {{ p.status === 'archived' ? '已归档' : '进行中' }}
              </span>
            </td>
            <td style="font-size: var(--text-sm); color: var(--text-muted);">{{ fmtTime(p.created_at) }}</td>
            <td>
              <button class="btn btn-sm" @click="openEdit(p)">编辑</button>
              <button v-if="p.status !== 'archived'" class="btn btn-sm" style="margin-left: var(--space-2);" @click="setStatus(p, 'archived')">归档</button>
              <button v-else class="btn btn-sm" style="margin-left: var(--space-2);" @click="setStatus(p, 'active')">恢复</button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 创建弹窗 -->
    <div v-if="showCreate" class="modal-backdrop" @click.self="showCreate = false">
      <div class="modal" style="max-width: 480px;">
        <div class="modal-header"><h3>新建项目</h3></div>
        <div class="modal-body">
          <div class="form-group">
            <label class="form-label">名称 *</label>
            <input class="form-input" v-model="createForm.name" placeholder="项目名（唯一）" @keyup.enter="submitCreate" />
          </div>
          <div class="form-group">
            <label class="form-label">描述</label>
            <textarea class="form-textarea" v-model="createForm.description" style="min-height: 60px;"></textarea>
          </div>
          <div class="form-group">
            <label class="form-label">图标（emoji，可选）</label>
            <input class="form-input" v-model="createForm.icon" placeholder="🚀" style="max-width: 120px;" />
          </div>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="showCreate = false">取消</button>
          <button class="btn btn-primary" :disabled="busy" @click="submitCreate">创建</button>
        </div>
      </div>
    </div>

    <!-- 编辑弹窗 -->
    <div v-if="editing" class="modal-backdrop" @click.self="editing = null">
      <div class="modal" style="max-width: 480px;">
        <div class="modal-header"><h3>编辑项目</h3></div>
        <div class="modal-body">
          <div class="form-group">
            <label class="form-label">名称 *</label>
            <input class="form-input" v-model="editForm.name" @keyup.enter="submitEdit" />
          </div>
          <div class="form-group">
            <label class="form-label">描述</label>
            <textarea class="form-textarea" v-model="editForm.description" style="min-height: 60px;"></textarea>
          </div>
          <div class="form-group">
            <label class="form-label">图标（emoji）</label>
            <input class="form-input" v-model="editForm.icon" style="max-width: 120px;" />
          </div>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="editing = null">取消</button>
          <button class="btn btn-primary" :disabled="busy" @click="submitEdit">保存</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.muted {
  color: var(--text-muted);
  font-size: var(--text-sm);
}
.panel-toolbar {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-4);
}
</style>
