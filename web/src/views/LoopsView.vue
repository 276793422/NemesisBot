<script setup lang="ts">
// 循环管理面（项5 自配速 durable loop，2026-11-16）。
// 状态真相源 <workspace>/loops/{id}.json；本页是**人**的管理面：
// 创建（绑定已有会话）/ 暂停 / 恢复（停车场解除入口）/ 调预算 / 删除。
// paused/parked 只能由人置/解（agent 工具面只有 wakeup/status/finish——防自锁）。
// 驱动器语义：预算三维保险丝 → 停车场留痕；estop 全冻结；唤醒注入绑定会话。
import { ref, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const available = ref(true)
const loops = ref<any[]>([])
const detail = ref<any | null>(null)

// 创建表单
const creating = ref(false)
const form = ref({ goal: '', session_id: '', delay_secs: 0, cron: '', max_turns: 0, wall_clock_secs: 0, max_tokens: 0 })
const creatingBusy = ref(false)

let pollTimer: ReturnType<typeof setInterval> | null = null

async function load() {
  try {
    const res = await request('loops', 'list')
    available.value = true
    loops.value = res?.loops || []
    if (detail.value) {
      const hit = loops.value.find((l: any) => l.id === detail.value?.id)
      if (hit) {
        detail.value = await request('loops', 'get', { id: hit.id })
      } else {
        detail.value = null
      }
    }
  } catch {
    // loops 模块缺席（feature 裁剪 / 旧网关）→ 诚实降级
    available.value = false
  }
  loading.value = false
}

async function openDetail(id: string) {
  try {
    detail.value = await request('loops', 'get', { id })
  } catch (e: any) {
    toast.error('读取详情失败: ' + e)
  }
}

async function createLoop() {
  if (!form.value.goal.trim()) { toast.error('目标不能为空'); return }
  if (!form.value.session_id.trim()) { toast.error('会话 ID 不能为空'); return }
  if (form.value.delay_secs > 0 && form.value.cron.trim()) { toast.error('延迟秒与 cron 互斥，只能给一个'); return }
  creatingBusy.value = true
  try {
    const data: any = { goal: form.value.goal.trim(), session_id: form.value.session_id.trim() }
    if (form.value.delay_secs > 0) data.delay_secs = form.value.delay_secs
    if (form.value.cron.trim()) data.cron = form.value.cron.trim()
    const budget: any = {}
    if (form.value.max_turns > 0) budget.max_turns_total = form.value.max_turns
    if (form.value.wall_clock_secs > 0) budget.wall_clock_budget_secs = form.value.wall_clock_secs
    if (form.value.max_tokens > 0) budget.max_tokens = form.value.max_tokens
    if (Object.keys(budget).length > 0) data.budget = budget
    const res = await request('loops', 'create', data)
    toast.success(`Loop ${res?.id} 已创建`)
    creating.value = false
    form.value = { goal: '', session_id: '', delay_secs: 0, cron: '', max_turns: 0, wall_clock_secs: 0, max_tokens: 0 }
    await load()
  } catch (e: any) {
    toast.error('创建失败: ' + e)
  } finally {
    creatingBusy.value = false
  }
}

async function act(id: string, cmd: 'pause' | 'resume' | 'remove') {
  try {
    if (cmd === 'remove') {
      await request('loops', 'remove', { id })
      toast.success(`Loop ${id} 已删除`)
      if (detail.value?.id === id) detail.value = null
    } else {
      await request('loops', cmd, { id })
      toast.success(`Loop ${id} ${cmd === 'pause' ? '已暂停' : '已恢复'}`)
      if (detail.value?.id === id) await openDetail(id)
    }
    await load()
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

async function widenBudget(id: string, maxTurns: number) {
  const v = prompt(`调整 loop ${id} 的唤醒次数预算（当前 ${maxTurns}，0=不限）：`, String(maxTurns))
  if (v === null) return
  const n = Number(v)
  if (!Number.isFinite(n) || n < 0) { toast.error('预算必须是非负数'); return }
  try {
    await request('loops', 'update', { id, budget: { max_turns_total: n } })
    toast.success(`Loop ${id} 预算已调整`)
    if (detail.value?.id === id) await openDetail(id)
    await load()
  } catch (e: any) {
    toast.error('调整失败: ' + e)
  }
}

function statusCls(s: string): string {
  switch (s) {
    case 'active': return 'badge-success'
    case 'paused': return 'badge-warning'
    case 'parked': return 'badge-error'
    case 'done': return 'badge-info'
    default: return 'badge-neutral'
  }
}

function statusLabel(s: string): string {
  switch (s) {
    case 'active': return '运行中'
    case 'paused': return '已暂停'
    case 'parked': return '停车场'
    case 'done': return '已收尾'
    case 'failed': return '失败'
    default: return s
  }
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
  <div class="page-loops">
    <div class="page-header">
      <h2>循环</h2>
      <div class="page-header-actions">
        <button class="btn" data-testid="refresh-btn" @click="load">刷新</button>
        <button class="btn btn-primary" data-testid="create-btn" @click="creating = !creating">新建循环</button>
      </div>
    </div>

    <div class="page-body">
      <div class="card" style="margin-bottom: var(--space-4);">
        <div class="card-body">
          <p style="font-size: var(--text-sm); color: var(--text-secondary); margin: 0;">
            自配速 durable loop：agent 绑定会话自主排程唤醒（<code>loop_wakeup</code> 排下次、<code>loop_finish</code> 收尾），
            gateway 驱动器按排程注入「继续你的目标」。防失控：预算三维保险丝（次数/墙钟/token）→ 耗尽自动进停车场；
            暂停/恢复只有人能操作；急停期间全部冻结。状态真相源 <code>workspace/loops/*.json</code>。
          </p>
        </div>
      </div>

      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <template v-else-if="!available">
        <div class="card">
          <div class="card-body">
            <p class="empty-state" data-testid="loops-unavailable" style="margin: 0;">
              loop 子系统未装配（loop feature 未编译或网关版本较旧）——管理面不可用。
            </p>
          </div>
        </div>
      </template>

      <template v-else>
        <!-- 创建表单 -->
        <div v-if="creating" class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header"><h3>新建循环</h3></div>
          <div class="card-body">
            <div style="display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-3);">
              <label style="grid-column: 1 / -1;">目标
                <input v-model="form.goal" data-testid="create-goal" placeholder="例如：每天检查 CI 并修复失败项" style="width: 100%;" />
              </label>
              <label>会话 ID（必须已存在）
                <input v-model="form.session_id" data-testid="create-session" placeholder="例如：main" style="width: 100%;" />
              </label>
              <label>首次唤醒延迟秒（0 = 等 agent 自排程）
                <input v-model.number="form.delay_secs" type="number" min="0" style="width: 100%;" />
              </label>
              <label>cron 表达式（可选，与延迟互斥）
                <input v-model="form.cron" placeholder="0 9 * * *" style="width: 100%;" />
              </label>
              <label>唤醒次数预算（0 = 不限）
                <input v-model.number="form.max_turns" type="number" min="0" style="width: 100%;" />
              </label>
              <label>墙钟预算秒（0 = 不限）
                <input v-model.number="form.wall_clock_secs" type="number" min="0" style="width: 100%;" />
              </label>
              <label>token 预算（0 = 不限）
                <input v-model.number="form.max_tokens" type="number" min="0" style="width: 100%;" />
              </label>
            </div>
            <div style="margin-top: var(--space-3); display: flex; gap: var(--space-2);">
              <button class="btn btn-primary" data-testid="create-submit" :disabled="creatingBusy" @click="createLoop">创建</button>
              <button class="btn" @click="creating = false">取消</button>
            </div>
          </div>
        </div>

        <!-- 循环列表 -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>循环</h3>
            <span style="font-size: var(--text-sm); color: var(--text-secondary);">{{ loops.length }} 条</span>
          </div>
          <div class="card-body">
            <div v-if="loops.length === 0" class="empty-state" data-testid="loops-empty">
              <p>暂无循环——用「新建循环」绑定一个已有会话，或让 agent 用 loop_wakeup 工具自排程</p>
            </div>
            <div v-else class="table-wrap">
              <table>
                <thead>
                  <tr><th>ID</th><th>目标</th><th>状态</th><th>下次唤醒</th><th>预算（次数/墙钟/token）</th><th>绑定会话</th><th>操作</th></tr>
                </thead>
                <tbody>
                  <tr v-for="(l, idx) in loops" :key="l.id" :data-testid="'loop-row-' + idx" style="cursor: pointer;" @click="openDetail(l.id)">
                    <td style="font-family: monospace;">{{ l.id }}</td>
                    <td style="max-width: 260px; word-break: break-all;">{{ l.goal }}</td>
                    <td>
                      <span class="badge" :class="statusCls(l.status)" :data-testid="'loop-status-' + idx">{{ statusLabel(l.status) }}</span>
                    </td>
                    <td style="font-size: var(--text-xs); white-space: nowrap;">{{ l.next_wake || '-' }}<template v-if="l.cron"> (cron {{ l.cron }})</template></td>
                    <td style="font-size: var(--text-xs); white-space: nowrap;">
                      {{ l.spent?.turns ?? 0 }}/{{ l.budget?.max_turns_total || 0 }} · {{ l.spent?.wall_clock_secs ?? 0 }}/{{ l.budget?.wall_clock_budget_secs || 0 }}s · {{ l.spent?.tokens ?? 0 }}/{{ l.budget?.max_tokens || 0 }}
                      <button v-if="l.status === 'parked'" class="btn" style="font-size: var(--text-xs); margin-left: var(--space-1);" :data-testid="'widen-' + idx" @click.stop="widenBudget(l.id, l.budget?.max_turns_total || 0)">调预算</button>
                    </td>
                    <td style="max-width: 200px; font-family: monospace; font-size: var(--text-xs); word-break: break-all;">{{ l.session_key }}</td>
                    <td style="white-space: nowrap;" @click.stop>
                      <button v-if="l.status === 'active'" class="btn" style="font-size: var(--text-xs);" :data-testid="'pause-' + idx" @click="act(l.id, 'pause')">暂停</button>
                      <button v-if="l.status === 'paused' || l.status === 'parked'" class="btn" style="font-size: var(--text-xs);" :data-testid="'resume-' + idx" @click="act(l.id, 'resume')">恢复</button>
                      <button class="btn" style="font-size: var(--text-xs); color: var(--color-danger, #c0392b);" :data-testid="'remove-' + idx" @click="act(l.id, 'remove')">删除</button>
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
            <p style="font-size: var(--text-sm); margin: 0 0 var(--space-2);"><strong>目标：</strong>{{ detail.goal }}</p>
            <p v-if="detail.plan" style="font-size: var(--text-sm); margin: 0 0 var(--space-2);"><strong>计划：</strong>{{ detail.plan }}</p>
            <p v-if="detail.finish_summary" style="font-size: var(--text-sm); margin: 0 0 var(--space-2);"><strong>收尾总结：</strong>{{ detail.finish_summary }}</p>
            <p style="font-size: var(--text-xs); color: var(--text-secondary); margin: 0 0 var(--space-2);">
              会话 <code>{{ detail.session_key }}</code> · 创建 {{ detail.started_at }} · 下次唤醒 {{ detail.next_wake || '-' }}
            </p>
            <div v-if="(detail.progress_log || []).length > 0" style="font-size: var(--text-xs); font-family: monospace; max-height: 220px; overflow-y: auto; border-top: 1px solid var(--border-color, #ddd); padding-top: var(--space-2);">
              <div v-for="(e, i) in detail.progress_log" :key="i" style="margin-bottom: 2px;">
                [{{ e.at }}] {{ e.text }}
              </div>
            </div>
          </div>
        </div>
      </template>
    </div>
  </div>
</template>
