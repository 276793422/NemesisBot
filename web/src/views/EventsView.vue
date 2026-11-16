<script setup lang="ts">
// 事件管理面（项1 事件驱动唤醒 P2，2026-11-16）。
// 三族事件源统一视图：
// ① fs 事件规则（events.json 热重载，events.* WSAPI：规则表 + JSON 编辑 +
//    泵统计）；
// ② 时间触发源（tasks.cron.list 只读收编——管理走任务页，此处只展示）；
// ③ 进程完成唤醒（background_start / 后台子代理，管理走执行单元页，此处
//    只放链接）。
// 三闸语义（冷却/小时窗/estop 冻结）由后端 nemesis-events 执行，页面只展示。
import { ref, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(true)
const available = ref(true)
const stats = ref<any>({})
const rules = ref<any[]>([])
const pending = ref<any[]>([])
const rulesJson = ref('[]')
const saving = ref(false)
// 用户编辑器有未保存修改时，5s 轮询不得覆写 rulesJson（否则编辑被冲掉）。
const rulesDirty = ref(false)

// 时间触发源（cron 收编只读）
const cronJobs = ref<any[]>([])

let pollTimer: ReturnType<typeof setInterval> | null = null

async function load() {
  try {
    const st = await request('events', 'status')
    available.value = true
    stats.value = st?.stats || {}
    rules.value = st?.rules || []
    pending.value = st?.pending || []
    if (!saving.value && !rulesDirty.value) {
      const rd = await request('events', 'rules.get')
      rulesJson.value = JSON.stringify(rd?.rules ?? [], null, 2)
    }
  } catch {
    // events 泵未装配（feature 裁剪 / 旧网关）→ 诚实降级
    available.value = false
  }
  try {
    const cl = await request('tasks', 'cron.list')
    cronJobs.value = cl?.jobs || cl?.items || []
  } catch { /* cron 服务缺席不阻塞本页 */ }
  loading.value = false
}

async function saveRules() {
  let parsed: any
  try {
    parsed = JSON.parse(rulesJson.value)
  } catch (e: any) {
    toast.error('JSON 解析失败: ' + e.message)
    return
  }
  if (!Array.isArray(parsed)) {
    toast.error('规则必须是数组（[ {...}, ... ]）')
    return
  }
  saving.value = true
  try {
    const res = await request('events', 'rules.save', { rules: parsed })
    const warns: string[] = res?.warnings || []
    if (warns.length > 0) {
      toast.warn('已保存，校验提示：' + warns.join('；'))
    } else {
      toast.success('规则已保存（泵 ≤250ms 热重载）')
    }
    rulesDirty.value = false
    await load()
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  } finally {
    saving.value = false
  }
}

async function toggleRule(r: any) {
  try {
    const parsed = JSON.parse(rulesJson.value)
    if (Array.isArray(parsed)) {
      const hit = parsed.find((x: any) => x && x.id === r.id)
      if (hit) {
        hit.deliver = hit.deliver || {}
        hit.deliver.enabled = !r.enabled
        rulesJson.value = JSON.stringify(parsed, null, 2)
      }
    }
    await saveRules()
  } catch (e: any) {
    toast.error('启停失败: ' + e)
  }
}

function fmtNum(n: any): string {
  return typeof n === 'number' ? String(n) : '0'
}

const STAT_CARDS = [
  { key: 'delivered_messages', label: '投递消息', cls: 'badge-success' },
  { key: 'delivered_events', label: '投递事件', cls: 'badge-success' },
  { key: 'deduped', label: '防抖折叠', cls: 'badge-info' },
  { key: 'dropped_cooldown', label: '冷却丢弃', cls: 'badge-warning' },
  { key: 'dropped_hourly', label: '小时窗丢弃', cls: 'badge-warning' },
  { key: 'dropped_estop', label: '急停丢弃', cls: 'badge-error' },
  { key: 'dropped_delivery', label: '投递失败', cls: 'badge-error' },
  { key: 'dropped_reload', label: '重载丢弃', cls: 'badge-neutral' },
  { key: 'reloads', label: '规则重载', cls: 'badge-info' },
] as const

onMounted(() => {
  load()
  pollTimer = setInterval(load, 5000)
})
onUnmounted(() => {
  if (pollTimer) clearInterval(pollTimer)
})
</script>

<template>
  <div class="page-events">
    <div class="page-header">
      <h2>事件</h2>
      <div class="page-header-actions">
        <button class="btn" data-testid="refresh-btn" @click="load">刷新</button>
      </div>
    </div>

    <div class="page-body">
      <div class="card" style="margin-bottom: var(--space-4);">
        <div class="card-body">
          <p style="font-size: var(--text-sm); color: var(--text-secondary); margin: 0;">
            出事主动叫人：文件系统事件按规则唤醒指定会话（<code>workspace/config/events.json</code> 热重载）。
            防失控三闸：per-rule 冷却（冷却内爆发丢弃不排队）+ 滑动小时窗（per-rule 与全局 60/h）+
            急停冻结（estop 期间全部丢弃计数，release 恢复积累）。目标会话缺席时诚实失败不排队。
          </p>
        </div>
      </div>

      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <template v-else-if="!available">
        <div class="card">
          <div class="card-body">
            <p class="empty-state" data-testid="events-unavailable" style="margin: 0;">
              事件泵未装配（events feature 未编译或网关版本较旧）——规则与统计面不可用。
            </p>
          </div>
        </div>
      </template>

      <template v-else>
        <!-- 泵统计 -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>泵统计</h3>
            <span style="font-size: var(--text-sm); color: var(--text-secondary);" data-testid="pending-summary">
              待投 buffer：{{ pending.length }} 条规则
            </span>
          </div>
          <div class="card-body">
            <div style="display: flex; flex-wrap: wrap; gap: var(--space-2);">
              <span v-for="c in STAT_CARDS" :key="c.key" class="badge" :class="c.cls" :data-testid="'stat-' + c.key">
                {{ c.label }} {{ fmtNum(stats[c.key]) }}
              </span>
            </div>
            <div v-if="pending.length > 0" style="margin-top: var(--space-2); font-size: var(--text-xs); color: var(--text-muted);">
              <span v-for="p in pending" :key="p.rule_id" style="margin-right: var(--space-3);">
                {{ p.rule_id }}: 攒 {{ p.count }} 个事件（静默窗中）
              </span>
            </div>
          </div>
        </div>

        <!-- 规则表 -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>事件规则</h3>
            <span style="font-size: var(--text-sm); color: var(--text-secondary);">{{ rules.length }} 条</span>
          </div>
          <div class="card-body">
            <div v-if="rules.length === 0" class="empty-state" data-testid="rules-empty">
              <p>暂无规则——在下方编辑器写入或手工创建 events.json</p>
            </div>
            <div v-else class="table-wrap">
              <table>
                <thead>
                  <tr><th>ID</th><th>监视</th><th>目标会话</th><th>三闸</th><th>状态</th><th>操作</th></tr>
                </thead>
                <tbody>
                  <tr v-for="(r, idx) in rules" :key="r.id" :data-testid="'rule-row-' + idx">
                    <td style="font-family: monospace;">{{ r.id }}</td>
                    <td style="max-width: 320px; font-family: monospace; font-size: var(--text-xs); word-break: break-all;">
                      {{ r.kind }}: {{ r.path }}<template v-if="r.glob"> ({{ r.glob }})</template>
                    </td>
                    <td style="max-width: 260px; font-family: monospace; font-size: var(--text-xs); word-break: break-all;">{{ r.session_key }}</td>
                    <td style="font-size: var(--text-xs); color: var(--text-secondary); white-space: nowrap;">
                      冷却 {{ r.cooldown_secs }}s · 合并 {{ r.merge_window_secs }}s · ≤{{ r.max_per_hour === 0 ? '∞' : r.max_per_hour }}/h
                    </td>
                    <td>
                      <span class="badge" :class="r.enabled ? 'badge-success' : 'badge-neutral'" :data-testid="'rule-status-' + idx">
                        {{ r.enabled ? '启用' : '停用' }}
                      </span>
                    </td>
                    <td style="white-space: nowrap;">
                      <button class="btn" style="font-size: var(--text-xs);" :data-testid="'toggle-' + idx" @click="toggleRule(r)">
                        {{ r.enabled ? '停用' : '启用' }}
                      </button>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- 规则编辑器 -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>规则编辑（events.json）</h3>
            <button class="btn btn-primary" data-testid="save-rules-btn" :disabled="saving" @click="saveRules">保存规则</button>
          </div>
          <div class="card-body">
            <p style="font-size: var(--text-xs); color: var(--text-muted); margin: 0 0 var(--space-2);">
              prompt 模板占位符：<code v-pre>{paths}</code> 路径列表（≤8 条）、<code v-pre>{count}</code> 合并计数、<code v-pre>{path}</code> 单路径；
              session_key 必须是 <code>agent:…</code> / <code>agents:…</code> 预设形态。0 值语义：cooldown/merge=立即、max_per_hour=不限、enabled=false 停用。
            </p>
            <textarea
              v-model="rulesJson"
              data-testid="rules-editor"
              spellcheck="false"
              style="width: 100%; min-height: 220px; font-family: monospace; font-size: var(--text-xs);"
              @input="rulesDirty = true"
            ></textarea>
          </div>
        </div>

        <!-- 收编：时间触发源（只读） -->
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header">
            <h3>时间触发源（cron，只读）</h3>
            <span style="font-size: var(--text-sm); color: var(--text-secondary);">管理走 <router-link to="/tasks">任务页</router-link></span>
          </div>
          <div class="card-body">
            <div v-if="cronJobs.length === 0" class="empty-state"><p>暂无 cron 作业</p></div>
            <div v-else class="table-wrap">
              <table>
                <thead><tr><th>名称</th><th>排程</th><th>状态</th></tr></thead>
                <tbody>
                  <tr v-for="(j, idx) in cronJobs" :key="j.id || idx">
                    <td>{{ j.name || j.id }}</td>
                    <td style="font-family: monospace;">{{ j.cron || j.schedule || '-' }}</td>
                    <td><span class="badge" :class="j.enabled === false ? 'badge-neutral' : 'badge-success'">{{ j.enabled === false ? '停用' : '启用' }}</span></td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- 收编：进程完成唤醒（链接） -->
        <div class="card">
          <div class="card-header">
            <h3>进程完成唤醒</h3>
          </div>
          <div class="card-body">
            <p style="font-size: var(--text-sm); color: var(--text-secondary); margin: 0;">
              后台进程与后台子代理完成时自动回灌发起会话（CompletionEvent 先例）。
              台账与管理走 <router-link to="/executors">执行单元页</router-link>。
            </p>
          </div>
        </div>
      </template>
    </div>
  </div>
</template>
