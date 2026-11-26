<script setup lang="ts">
// 目标管理面（目标系统，2026-11-17）。
// 状态真相源 <workspace>/config/goals.json + goals/{id}.goal.md；本页是
// **人**的管理面：创建（绑定已有会话）/ 编辑目标 / 暂停 / 解锁 / 删除 /
// 查看评估台账。done 只能由评估闸判定（agent 自称完成不生效）；paused
// 只能由人置/解；blocked（预算耗尽/评估失败）走 enable 解锁。
import { ref, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const available = ref(true)
const goals = ref<any[]>([])
const detail = ref<any | null>(null)

// 创建表单
const creating = ref(false)
const form = ref({ content: '', session_id: '', max_rounds: 256, eval_model: '' })
const creatingBusy = ref(false)

// 编辑表单（详情内）
const editing = ref(false)
const editContent = ref('')
const editBusy = ref(false)

let pollTimer: ReturnType<typeof setInterval> | null = null

async function load() {
  try {
    const res = await request('goals', 'list')
    available.value = true
    goals.value = res?.goals || []
    if (detail.value) {
      const hit = goals.value.find((g: any) => g.id === detail.value?.id)
      if (hit) {
        detail.value = await request('goals', 'get', { id: hit.id })
      } else {
        detail.value = null
      }
    }
  } catch {
    // goals 模块缺席（feature 裁剪 / 旧网关）→ 诚实降级
    available.value = false
  }
  loading.value = false
}

async function openDetail(id: string) {
  try {
    detail.value = await request('goals', 'get', { id })
    editing.value = false
  } catch (e: any) {
    toast.error('读取详情失败: ' + e)
  }
}

async function createGoal() {
  if (!form.value.content.trim()) { toast.error('目标内容不能为空'); return }
  if (!form.value.session_id.trim()) { toast.error('会话 ID 不能为空'); return }
  creatingBusy.value = true
  try {
    const data: any = { content: form.value.content.trim(), session_id: form.value.session_id.trim() }
    if (form.value.max_rounds > 0) data.max_rounds = form.value.max_rounds
    if (form.value.eval_model.trim()) data.eval_model = form.value.eval_model.trim()
    const res = await request('goals', 'create', data)
    toast.success(`目标 ${res?.id} 已创建`)
    creating.value = false
    form.value = { content: '', session_id: '', max_rounds: 256, eval_model: '' }
    await load()
  } catch (e: any) {
    toast.error('创建失败: ' + e)
  } finally {
    creatingBusy.value = false
  }
}

async function act(id: string, cmd: 'enable' | 'disable' | 'delete') {
  try {
    if (cmd === 'delete') {
      await request('goals', 'delete', { id })
      toast.success(`目标 ${id} 已删除`)
      if (detail.value?.id === id) detail.value = null
    } else {
      await request('goals', cmd, { id })
      toast.success(`目标 ${id} ${cmd === 'disable' ? '已暂停' : '已启用'}`)
      if (detail.value?.id === id) await openDetail(id)
    }
    await load()
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

function startEdit() {
  if (!detail.value) return
  editContent.value = detail.value.content || ''
  editing.value = true
}

async function saveEdit() {
  // 函数内 busy 闸（BUG-20261127-014 同族横向排查补修）：输入框 @keyup.enter
  // 直调绕过按钮 :disabled——在飞时连按 Enter 并发双发请求。
  if (editBusy.value) return
  if (!detail.value) return
  if (!editContent.value.trim()) { toast.error('目标内容不能为空'); return }
  editBusy.value = true
  try {
    await request('goals', 'update', { id: detail.value.id, content: editContent.value.trim() })
    toast.success(`目标 ${detail.value.id} 已更新`)
    editing.value = false
    await openDetail(detail.value.id)
    await load()
  } catch (e: any) {
    toast.error('更新失败: ' + e)
  } finally {
    editBusy.value = false
  }
}

function statusCls(s: string): string {
  switch (s) {
    case 'active': return 'badge-success'
    case 'paused': return 'badge-warning'
    case 'blocked': return 'badge-error'
    case 'done': return 'badge-info'
    default: return 'badge-neutral'
  }
}

function statusLabel(s: string): string {
  switch (s) {
    case 'active': return '运行中'
    case 'paused': return '已暂停'
    case 'blocked': return '已停车'
    case 'done': return '已完成'
    default: return s
  }
}

function fmtTime(ms: any): string {
  if (!ms) return '-'
  const d = new Date(Number(ms))
  if (isNaN(d.getTime())) return '-'
  return d.toLocaleString()
}

onMounted(() => {
  load()
  pollTimer = setInterval(load, 5000)
})
onUnmounted(() => {
  if (pollTimer) clearInterval(pollTimer)
})
</script>

<template>
  <div class="page-goals">
    <div class="page-header">
      <h2>目标</h2>
      <div class="page-header-actions">
        <button class="btn" data-testid="refresh-btn" @click="load">刷新</button>
        <button class="btn btn-primary" data-testid="create-btn" @click="creating = !creating">新建目标</button>
      </div>
    </div>

    <div class="page-body">
      <div class="card" style="margin-bottom: var(--space-4);">
        <div class="card-body">
          <p style="font-size: var(--text-sm); color: var(--text-secondary); margin: 0;">
            目标文件驱动的自监督循环：agent 绑定会话的每个回合结束时，评估闸独立重读目标文件判定——
            <strong>DONE</strong>（唯一完成出口，agent 自称完成不生效）或 <strong>CONTINUE</strong>（注入缺口继续循环）。
            预算耗尽或评估失败自动停车，人工 enable 解锁；急停期间评估冻结。
            状态真相源 <code>workspace/config/goals.json</code> + <code>goals/*.goal.md</code>。
          </p>
        </div>
      </div>

      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <template v-else-if="!available">
        <div class="card">
          <div class="card-body">
            <p class="empty-state" data-testid="goals-unavailable" style="margin: 0;">
              goals 子系统未装配（goals feature 未编译或网关版本较旧）——管理面不可用。
            </p>
          </div>
        </div>
      </template>

      <template v-else>
        <!-- 创建表单 -->
        <div v-if="creating" class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>新建目标</h3></div>
          <div class="card-body">
            <div style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-3);">
              <label style="grid-column: 1 / -1;">目标内容（验收标准写具体，评估闸逐条核验）
                <textarea v-model="form.content" data-testid="create-content" rows="5" placeholder="例如：修复登录 bug&#10;验收：1) 复现步骤不再失败 2) 相关测试全绿" style="width: 100%;"></textarea>
              </label>
              <label>会话 ID（必须已存在）
                <input v-model="form.session_id" data-testid="create-session" placeholder="例如：main" style="width: 100%;" />
              </label>
              <label>轮数预算（CONTINUE 注入轮上限，0 = 不限）
                <input v-model.number="form.max_rounds" data-testid="create-max-rounds" type="number" min="0" style="width: 100%;" />
              </label>
              <label>评估模型（留空 = 主模型；填 small = 杂务模型）
                <input v-model="form.eval_model" data-testid="create-eval-model" placeholder="small 或留空" style="width: 100%;" />
              </label>
            </div>
            <div style="margin-top: var(--space-3); display: flex; gap: var(--space-2);">
              <button class="btn btn-primary" data-testid="create-submit" :disabled="creatingBusy" @click="createGoal">创建</button>
              <button class="btn" @click="creating = false">取消</button>
            </div>
          </div>
        </div>

        <!-- 目标列表 -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>目标</h3>
            <span style="font-size: var(--text-sm); color: var(--text-secondary);">{{ goals.length }} 条</span>
          </div>
          <div class="card-body">
            <div v-if="goals.length === 0" class="empty-state" data-testid="goals-empty">
              <p>暂无目标——用「新建目标」绑定一个已有会话，agent 每回合结束会被评估闸监督推进</p>
            </div>
            <div v-else class="table-wrap">
              <table>
                <thead>
                  <tr><th>ID</th><th>状态</th><th>进度</th><th>创建者</th><th>停车原因</th><th>绑定会话</th><th>更新时间</th><th>操作</th></tr>
                </thead>
                <tbody>
                  <tr v-for="(g, idx) in goals" :key="g.id" :data-testid="'goal-row-' + idx" style="cursor: pointer;" @click="openDetail(g.id)">
                    <td style="font-family: monospace;">{{ g.id }}</td>
                    <td>
                      <span class="badge" :class="statusCls(g.status)" :data-testid="'goal-status-' + idx">{{ statusLabel(g.status) }}</span>
                    </td>
                    <td style="font-size: var(--text-xs); white-space: nowrap;">{{ g.rounds_used }}/{{ g.max_rounds === 0 ? '∞' : g.max_rounds }} 轮</td>
                    <td style="font-size: var(--text-xs);">{{ g.created_by === 'human' ? '人' : 'agent' }}</td>
                    <td style="font-size: var(--text-xs); max-width: 180px; word-break: break-all;">
                      <template v-if="g.blocked_message">{{ g.blocked_message }}</template>
                      <template v-else>-</template>
                    </td>
                    <td style="max-width: 200px; font-family: monospace; font-size: var(--text-xs); word-break: break-all;">{{ g.session_key }}</td>
                    <td style="font-size: var(--text-xs); white-space: nowrap;">{{ fmtTime(g.updated_at) }}</td>
                    <td style="white-space: nowrap;" @click.stop>
                      <button v-if="g.status === 'active'" class="btn" style="font-size: var(--text-xs);" :data-testid="'disable-' + idx" @click="act(g.id, 'disable')">暂停</button>
                      <button v-if="g.status === 'paused' || g.status === 'blocked'" class="btn" style="font-size: var(--text-xs);" :data-testid="'enable-' + idx" @click="act(g.id, 'enable')">启用</button>
                      <button class="btn" style="font-size: var(--text-xs); color: var(--color-danger, #c0392b);" :data-testid="'delete-' + idx" @click="act(g.id, 'delete')">删除</button>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- 详情 -->
        <div v-if="detail" class="card">
          <div class="card-header">
            <h3>详情 {{ detail.id }}</h3>
            <span class="badge" :class="statusCls(detail.status)">{{ statusLabel(detail.status) }}</span>
          </div>
          <div class="card-body">
            <p style="font-size: var(--text-xs); color: var(--text-secondary); margin: 0 0 var(--space-2);">
              会话 <code>{{ detail.session_key }}</code> · 创建者 {{ detail.created_by === 'human' ? '人' : 'agent' }} ·
              进度 {{ detail.rounds_used }}/{{ detail.max_rounds === 0 ? '∞' : detail.max_rounds }} 轮 ·
              评估模型 {{ detail.eval_model || '主模型' }} · 更新 {{ fmtTime(detail.updated_at) }}
            </p>
            <div v-if="detail.blocked_message" class="card" style="margin-bottom: var(--space-2); background: var(--color-danger-bg, rgba(192,57,43,.08)); border-color: var(--color-danger, #c0392b);">
              <div class="card-body" style="padding: var(--space-2);">
                <strong style="color: var(--color-danger, #c0392b);">停车（{{ detail.blocked_code }}）：</strong>{{ detail.blocked_message }}
              </div>
            </div>

            <!-- 目标内容（查看/编辑） -->
            <template v-if="!editing">
              <pre v-if="detail.content" data-testid="detail-content" style="white-space: pre-wrap; font-size: var(--text-sm); background: var(--bg-secondary, rgba(127,127,127,.06)); padding: var(--space-3); border-radius: var(--radius-md, 6px); margin: 0 0 var(--space-2);">{{ detail.content }}</pre>
              <p v-else style="font-size: var(--text-sm); color: var(--text-secondary); margin: 0 0 var(--space-2);">
                ⚠ 目标文件缺失（goals/{{ detail.id }}.goal.md 被手删）——评估闸将缺料，请编辑补写或删除重建。
              </p>
              <div style="display: flex; gap: var(--space-2); margin-bottom: var(--space-3);">
                <button v-if="detail.status !== 'done'" class="btn" data-testid="edit-btn" @click="startEdit">编辑目标</button>
              </div>
            </template>
            <template v-else>
              <textarea v-model="editContent" data-testid="edit-content" rows="6" style="width: 100%; margin-bottom: var(--space-2);"></textarea>
              <div style="display: flex; gap: var(--space-2); margin-bottom: var(--space-3);">
                <button class="btn btn-primary" data-testid="edit-save" :disabled="editBusy" @click="saveEdit">保存</button>
                <button class="btn" @click="editing = false">取消</button>
              </div>
            </template>

            <!-- 评估台账 -->
            <div v-if="(detail.evals || []).length > 0" style="font-size: var(--text-xs); border-top: 1px solid var(--border-color, #ddd); padding-top: var(--space-2); max-height: 260px; overflow-y: auto;">
              <div v-for="(e, i) in detail.evals" :key="i" data-testid="eval-record" style="margin-bottom: var(--space-1);">
                <span class="badge" :class="e.verdict === 'done' ? 'badge-info' : (e.verdict === 'error' ? 'badge-error' : 'badge-neutral')" style="margin-right: var(--space-1);">{{ e.verdict }}</span>
                第 {{ e.round }} 轮 · {{ fmtTime(e.at) }} · {{ e.model }} · {{ e.duration_ms }}ms
                <div v-if="e.gap" style="color: var(--text-secondary); margin-left: var(--space-4);">缺口：{{ e.gap }}</div>
              </div>
            </div>
            <p v-else style="font-size: var(--text-xs); color: var(--text-secondary); margin: 0;">
              暂无评估记录——绑定会话每个回合结束后评估闸会留下台账。
            </p>
          </div>
        </div>
      </template>
    </div>
  </div>
</template>
