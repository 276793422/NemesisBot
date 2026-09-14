<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { useBoardChanged } from '../../composables/useBoardChanged'
import { fmtTime } from './boardMeta'

// 项目面板（W2 P3）：项目列表 + 创建 + 编辑（字段级 patch）+ 归档/恢复。
// 归档 = project.update status="archived"（软删除；后端 project.update）。
// C0（goal 2026-09-13）：状态徽章映射 ProjectStatus 完整四态（此前二元硬
// 编码把 completed 也显示成「进行中」）；C4：列表分「进行中 / 已归档（折叠）」
// 两组——已完结项目不碍眼、可恢复；不做硬删除（审计链完整性）。

const { request } = useWSAPI()
const toast = useToast()

interface Project {
  id: number
  name: string
  description: string
  status: string
  icon: string
  created_at: number
  // 看板项目档案 P2/B1：项目档案目录绝对路径（存量项目可能为 null；
  // 绑定不可变——编辑弹窗刻意不提供修改入口）。
  directory?: string | null
  // P5/F2+F3：冲突冻结标志 + 待补合并队列（project.list 全量序列化带上；
  // 系统管理字段，前端只读展示——解冻唯一入口是 project.resume）。
  conflict_frozen?: boolean
  pending_merges?: { task_id: string; issue_id: number; reason: string; parked_at_ms: number }[]
}

// ProjectStatus 四态 → 徽章（与后端 models.rs ProjectStatus 枚举同词表）。
const STATUS_BADGE: Record<string, { label: string; cls: string }> = {
  active: { label: '未启动', cls: 'badge-info' },
  in_progress: { label: '进行中', cls: 'badge-warning' },
  completed: { label: '已完成', cls: 'badge-success' },
  archived: { label: '已归档', cls: 'badge-neutral' },
}

function statusBadge(status: string): { label: string; cls: string } {
  // 未知状态（旧数据/后端新增）→ 原文中性徽章，不炸渲染。
  return STATUS_BADGE[status] ?? { label: status || '未知', cls: 'badge-neutral' }
}

const loading = ref(true)
const projects = ref<Project[]>([])
// C4：已归档分组默认折叠（活跃项目不被历史淹没；展开可恢复）。
const showArchived = ref(false)

// C1（goal P1）：项目进度聚合（board.project.progress 全项目摘要）。
interface ProjectProgress {
  project_id: number
  total: number
  counts: Record<string, number>
  stage: string
  // F11（档案 P6）：完整性投影（未绑定档案目录 = undefined，前端隐藏）。
  archive_integrity?: string | null
  archive_missing_blocks?: string[]
}
const progressMap = ref<Record<number, ProjectProgress>>({})

function progressLine(p: Project): string {
  const g = progressMap.value[p.id]
  if (!g) return ''
  return `${g.counts?.done ?? 0}/${g.total} · ${g.stage ?? ''}`
}

// F11（档案 P6）：完整性 ⚠ 徽章——缺失块非空才显示（hover 看明细）。
function missingBlocks(p: Project): string[] {
  return progressMap.value[p.id]?.archive_missing_blocks ?? []
}

// F10（档案 P6）：打开档案目录（board.project.open_dir；只放行注册表
// 已知项目路径——后端裁决，前端不传路径）。
async function openArchiveDir(p: Project) {
  try {
    await request('board', 'project.open_dir', { project_id: p.id })
    toast.success('已打开档案目录')
  } catch (e: any) {
    toast.error('打开档案目录失败: ' + e)
  }
}

// D（goal P2）：项目级「恢复发车」——入口项目上一键重试全部可派未派子单。
// 可见条件：卡点环节表明有活没派出去（停车/受阻/待重派/待派发）；
// P5/F2：冲突冻结中的项目恒可见——resume 是唯一解冻出口。
function canResume(p: Project): boolean {
  if (p.conflict_frozen) return true
  const g = progressMap.value[p.id]
  if (!g || !g.total) return false
  const stage = g.stage || ''
  return /停车|受阻|待重派|待派发/.test(stage)
}

const resumeTarget = ref<Project | null>(null)
const resumePreview = ref<{
  candidates?: { issue_id: number; number: string; title: string; target: string }[]
  frozen?: boolean
  note?: string
} | null>(null)
const resumeBusy = ref(false)

async function previewResume(p: Project) {
  resumeTarget.value = p
  try {
    const r = await request('board', 'project.resume', { project_id: p.id, dry_run: true })
    resumePreview.value = r || { candidates: [] }
  } catch (e: any) {
    toast.error('恢复预览失败: ' + e)
    resumeTarget.value = null
  }
}

async function doResume() {
  const p = resumeTarget.value
  if (!p) return
  resumeBusy.value = true
  try {
    const r = await request('board', 'project.resume', { project_id: p.id, dry_run: false })
    if (r?.frozen && r?.resumed === false) {
      // 补合并再冲突 → 重新冻结（后端语义）；提示回人工。
      toast.warn('补合并再次冲突，项目已重新冻结回人工——请解决冲突后再次恢复发车')
    } else if (r?.conflict_replay) {
      const rp = r.conflict_replay
      toast.success(
        `解冻完成：补合并 ${rp.merged ?? 0} 单、跳过 ${rp.superseded ?? 0}、停车 ${rp.parked ?? 0}；恢复发车已派出 ${r?.dispatched ?? 0} 单`
      )
    } else {
      toast.success(`恢复发车：已派出 ${r?.dispatched ?? 0} 单`)
    }
    resumePreview.value = null
    resumeTarget.value = null
    await load()
  } catch (e: any) {
    toast.error('恢复发车失败: ' + e)
  } finally {
    resumeBusy.value = false
  }
}

const activeProjects = computed(() => projects.value.filter((p) => p.status !== 'archived'))
const archivedProjects = computed(() => projects.value.filter((p) => p.status === 'archived'))

// P5/F3：冻结原因卡里的待补合并单数（project.list 全量序列化带上队列）。
const frozenPendingCount = computed(() => resumeTarget.value?.pending_merges?.length ?? 0)

const showCreate = ref(false)
const busy = ref(false)
const createForm = ref({ name: '', description: '', icon: '', acceptance_criteria: '', auto_start: false, directory: '' })

// 编辑弹窗（改名/描述/图标）。
const editing = ref<Project | null>(null)
const editForm = ref({ name: '', description: '', icon: '' })

async function load(silent = false) {
  if (!silent) loading.value = true
  try {
    // C1/C3：项目列表 + 进度摘要并行拉取；board-changed 推送时静默刷新
    // 实现「进度实时跳变」（拍板②=事件推送）。
    const [r, pr] = await Promise.all([
      request('board', 'project.list', {}),
      request('board', 'project.progress', {}).catch(() => null),
    ])
    projects.value = r?.projects || []
    const map: Record<number, ProjectProgress> = {}
    for (const row of pr?.projects || []) map[row.project_id] = row
    progressMap.value = map
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
    // 看板项目档案 P2/B1：目录留空 = 后端自动分配 <workspace>/board-projects/；
    // 前端不预校验（绝对路径/重叠/8.3 权威校验都在后端，错误诚实回报）。
    const dir = createForm.value.directory.trim()
    const r = await request('board', 'project.create', {
      name: createForm.value.name.trim(),
      description: createForm.value.description,
      icon: createForm.value.icon.trim(),
      acceptance_criteria: createForm.value.acceptance_criteria,
      auto_start: createForm.value.auto_start,
      ...(dir ? { directory: dir } : {}),
    })
    const started = r?.auto_start?.issue_number
    const dirNote = r?.directory ? `\n档案目录：${r.directory}` : ''
    toast.success(started ? `已创建项目并自动启动父单 ${started}${dirNote}` : `已创建项目${dirNote}`)
    showCreate.value = false
    createForm.value = { name: '', description: '', icon: '', acceptance_criteria: '', auto_start: false, directory: '' }
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

    <div v-else>
      <!-- 进行中（非归档） -->
      <div v-if="activeProjects.length" class="table-wrap">
        <table>
          <thead>
            <tr><th>项目</th><th>进度</th><th>描述</th><th>状态</th><th>创建时间</th><th style="width: 200px;">操作</th></tr>
          </thead>
          <tbody>
            <tr v-for="p in activeProjects" :key="p.id">
              <td style="font-weight: 500;">
                {{ p.icon }} {{ p.name }}
                <div v-if="p.directory" class="muted" :title="p.directory" style="font-weight: 400; font-size: var(--text-xs); max-width: 260px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">📁 {{ p.directory }}</div>
              </td>
              <td style="font-size: var(--text-sm);" :class="{ muted: !progressLine(p) }">
                {{ progressLine(p) || '—' }}
                <span
                  v-if="missingBlocks(p).length"
                  class="badge badge-error"
                  style="margin-left: var(--space-1);"
                  :title="'档案缺失块：\n' + missingBlocks(p).join('\n')"
                >⚠ 缺 {{ missingBlocks(p).length }} 块</span>
              </td>
              <td style="font-size: var(--text-sm); color: var(--text-muted);">{{ p.description || '—' }}</td>
              <td>
                <span class="badge" :class="statusBadge(p.status).cls">{{ statusBadge(p.status).label }}</span>
                <span v-if="p.conflict_frozen" class="badge badge-error" style="margin-left: var(--space-1);" title="项目冲突冻结中：派发被闸、交付只登记不合入；恢复发车 = 唯一解冻出口">⛔ 冲突冻结</span>
              </td>
              <td style="font-size: var(--text-sm); color: var(--text-muted);">{{ fmtTime(p.created_at) }}</td>
              <td>
                <button class="btn btn-sm" @click="openEdit(p)">编辑</button>
                <button v-if="p.directory" class="btn btn-sm" style="margin-left: var(--space-2);" title="在系统文件管理器中打开项目档案目录" @click="openArchiveDir(p)">📂 档案</button>
                <button class="btn btn-sm" style="margin-left: var(--space-2);" @click="setStatus(p, 'archived')">归档</button>
                <button v-if="canResume(p)" class="btn btn-sm btn-primary" style="margin-left: var(--space-2);" @click="previewResume(p)">
                  {{ p.conflict_frozen ? '解冻/恢复发车' : '恢复发车' }}
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else class="empty-state">
        <h3>暂无进行中的项目</h3>
        <p>已归档项目在下方「已归档」分组中，可展开恢复</p>
      </div>

      <!-- C4：已归档分组（默认折叠；软删除数据保留可恢复） -->
      <div v-if="archivedProjects.length" style="margin-top: var(--space-4);">
        <button class="btn btn-sm archived-toggle" @click="showArchived = !showArchived">
          {{ showArchived ? '▾' : '▸' }} 已归档（{{ archivedProjects.length }}）
        </button>
        <div v-if="showArchived" class="table-wrap" style="margin-top: var(--space-2);">
          <table>
            <thead>
              <tr><th>项目</th><th>进度</th><th>描述</th><th>状态</th><th>创建时间</th><th style="width: 200px;">操作</th></tr>
            </thead>
            <tbody>
              <tr v-for="p in archivedProjects" :key="p.id" style="opacity: 0.75;">
                <td style="font-weight: 500;">
                  {{ p.icon }} {{ p.name }}
                  <div v-if="p.directory" class="muted" :title="p.directory" style="font-weight: 400; font-size: var(--text-xs); max-width: 260px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">📁 {{ p.directory }}</div>
                </td>
                <td style="font-size: var(--text-sm);" :class="{ muted: !progressLine(p) }">
                  {{ progressLine(p) || '—' }}
                  <span
                    v-if="missingBlocks(p).length"
                    class="badge badge-error"
                    style="margin-left: var(--space-1);"
                    :title="'档案缺失块：\n' + missingBlocks(p).join('\n')"
                  >⚠ 缺 {{ missingBlocks(p).length }} 块</span>
                </td>
                <td style="font-size: var(--text-sm); color: var(--text-muted);">{{ p.description || '—' }}</td>
                <td>
                  <span class="badge" :class="statusBadge(p.status).cls">{{ statusBadge(p.status).label }}</span>
                </td>
                <td style="font-size: var(--text-sm); color: var(--text-muted);">{{ fmtTime(p.created_at) }}</td>
                <td>
                  <button class="btn btn-sm" @click="openEdit(p)">编辑</button>
                  <button v-if="p.directory" class="btn btn-sm" style="margin-left: var(--space-2);" title="在系统文件管理器中打开项目档案目录" @click="openArchiveDir(p)">📂 档案</button>
                  <button class="btn btn-sm" style="margin-left: var(--space-2);" @click="setStatus(p, 'active')">恢复</button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
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
          <div class="form-group">
            <label class="form-label">验收标准（可选）</label>
            <textarea class="form-textarea" v-model="createForm.acceptance_criteria" style="min-height: 60px;" placeholder="验收标准（一行一条；AI 拆解与完成判定会参考）"></textarea>
          </div>
          <div class="form-group">
            <label class="form-label">档案目录（可选）</label>
            <input class="form-input" v-model="createForm.directory" placeholder="留空自动分配到工作区 board-projects/ 下" />
            <div class="muted" style="margin-top: var(--space-1); font-size: var(--text-xs);">
              项目档案落盘目录（docs/ 计划与评审、records/ 派发交付、timeline 时间线）。须为绝对路径，不能与工作区或其他项目目录重叠；创建后不可修改。
            </div>
          </div>
          <label class="muted" style="display: flex; align-items: center; gap: var(--space-1); cursor: pointer; margin-bottom: var(--space-2);">
            <input type="checkbox" v-model="createForm.auto_start" />
            创建后自动启动（建父单 + AI 拆解）
          </label>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="showCreate = false">取消</button>
          <button class="btn btn-primary" :disabled="busy" @click="submitCreate">创建</button>
        </div>
      </div>
    </div>

    <!-- D：恢复发车预览确认弹窗（先看将派哪些，再确认执行；P5 冻结项目
         显示冻结原因卡 + 解除指引） -->
    <div v-if="resumeTarget" class="modal-backdrop" @click.self="resumeTarget = null">
      <div class="modal" style="max-width: 560px;">
        <div class="modal-header"><h3>恢复发车 — {{ resumeTarget.name }}</h3></div>
        <div class="modal-body">
          <!-- P5/F2：冻结原因卡（解除指引） -->
          <div v-if="resumePreview?.frozen" class="frozen-card">
            <strong>⛔ 项目冲突冻结中</strong>
            <p style="margin: var(--space-1) 0 0;">
              确认后将依次执行：<br />
              ① 请先在档案目录解决合并冲突并确认工作副本内容正确（<code>board-projects/…</code>，冲突文件见决策流/系统评论）；<br />
              ② 后端自动 commit 人工落定结果；<br />
              ③ 串行补合并冻结期收到的交付（{{ frozenPendingCount }} 单待补）；<br />
              ④ 补合并成功即恢复派发；如补合并再冲突会重新冻结回人工。
            </p>
          </div>
          <template v-else>
            <p class="muted">将对以下可派子单重新发起派发（派发顺序由依赖闸/AI 决定）：</p>
            <ul style="margin: var(--space-2) 0;">
              <li v-for="c in resumePreview?.candidates || []" :key="c.issue_id">
                <strong>{{ c.number }}</strong> {{ c.title }}
                <span class="badge badge-info">{{ c.target }}</span>
              </li>
            </ul>
            <p v-if="!resumePreview?.candidates?.length" class="muted">没有可派发的子单。</p>
            <p v-if="resumePreview?.candidates?.length" class="muted" style="font-size: var(--text-xs);">
              确认后立即执行；无匹配节点时将按兜底策略派发（若已开启）。
            </p>
          </template>
        </div>
        <div class="modal-footer">
          <button class="btn" @click="resumeTarget = null">取消</button>
          <button
            class="btn btn-primary"
            :disabled="resumeBusy || (!resumePreview?.frozen && !resumePreview?.candidates?.length)"
            @click="doResume"
          >{{ resumePreview?.frozen ? '确认解冻并补合并' : '确认恢复发车' }}</button>
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
.archived-toggle {
  color: var(--text-muted);
}
/* P5/F2：冻结原因卡（警示条——冻结语义 + 解除四步指引）。 */
.frozen-card {
  background: var(--bg-secondary, rgba(255, 100, 100, 0.08));
  border: 1px solid var(--error-color, #e5484d);
  border-radius: var(--radius-md, 6px);
  padding: var(--space-3);
  margin-bottom: var(--space-3);
  color: var(--text-primary);
  font-size: var(--text-sm);
}
</style>
