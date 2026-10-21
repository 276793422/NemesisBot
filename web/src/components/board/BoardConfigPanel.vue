<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'

// 配置面板（全自动流转 P1/A4）：看板自动化开关 + 参数。每项改动即时
// board.config.set（后端热生效——消费点每次现读），无统一保存按钮；
// 写失败时重新拉取回滚显示。后端唯一真相源：
// crates/nemesis-web/src/handlers/board.rs（config.get / config.set 白名单）
// + nemesis-config BoardFlagConfig。

const { request } = useWSAPI()
const toast = useToast()

interface BoardFlags {
  auto_review: boolean
  auto_accept: boolean
  auto_close_parent: boolean
  unlimited_mode: boolean
  conflict_auto_resolve: boolean
  /** 项目收口自动导出任务档案（logs/dossiers/ 自包含目录） */
  dossier_auto_on_complete: boolean
  /** 兜底派发档位：off=停车 / role=丢职能保角色（默认） / full=全松弛 */
  dispatch_fallback: 'off' | 'role' | 'full'
  dispatch_fallback_target: string | null
  max_redispatch: number
  dispatch_timeout_secs: number
  plan: { auto_confirm: boolean; model: string | null }
  review: {
    max_turns: number
    max_unsure_streak: number
    selfcheck: boolean
    auto_close_project: boolean
    checkers: number
    /** 必需检查契约：{匹配标签: [必需锚点 id]}；空串键 = 全局兜底规则 */
    required_checks: Record<string, string[]>
  }
  budget: {
    max_subissues_per_parent: number
    max_total_redispatch: number
    wall_clock_budget_secs: number
    max_tokens_per_parent: number
    max_send_retries: number
  }
  discussion: {
    retention_days: number
    max_agent_turns_per_thread: number
    hourly_budget_per_node: number
    rate_limit_per_min: number
  }
}

const loading = ref(true)
const flags = ref<BoardFlags | null>(null)

async function load() {
  loading.value = true
  try {
    const r = await request('board', 'config.get', {})
    flags.value = {
      auto_review: !!r?.auto_review,
      auto_accept: !!r?.auto_accept,
      auto_close_parent: !!r?.auto_close_parent,
      unlimited_mode: !!r?.unlimited_mode,
      conflict_auto_resolve: !!r?.conflict_auto_resolve,
      dossier_auto_on_complete: r?.dossier?.auto_on_complete !== false,
      dispatch_fallback: normalizeFallbackMode(r?.dispatch_fallback),
      dispatch_fallback_target: r?.dispatch_fallback_target ?? null,
      max_redispatch: r?.max_redispatch ?? 2,
      dispatch_timeout_secs: r?.dispatch_timeout_secs ?? 3600,
      plan: {
        auto_confirm: !!r?.plan?.auto_confirm,
        model: r?.plan?.model ?? null,
      },
      review: {
        // 0=不限（取证到能下结论为止）——与后端 BoardReviewConfig::default 同源。
        max_turns: r?.review?.max_turns ?? 0,
        // 连续 UNSURE 停车阈值（0=不限）——与后端默认 2 同源。
        max_unsure_streak: r?.review?.max_unsure_streak ?? 2,
        selfcheck: !!r?.review?.selfcheck,
        auto_close_project: !!r?.review?.auto_close_project,
        checkers: r?.review?.checkers ?? 1,
        required_checks: r?.review?.required_checks ?? {},
      },
      budget: {
        max_subissues_per_parent: r?.budget?.max_subissues_per_parent ?? 20,
        max_total_redispatch: r?.budget?.max_total_redispatch ?? 0,
        wall_clock_budget_secs: r?.budget?.wall_clock_budget_secs ?? 0,
        max_tokens_per_parent: r?.budget?.max_tokens_per_parent ?? 0,
        max_send_retries: r?.budget?.max_send_retries ?? 5,
      },
      discussion: {
        retention_days: r?.discussion?.retention_days ?? 30,
        max_agent_turns_per_thread: r?.discussion?.max_agent_turns_per_thread ?? 12,
        hourly_budget_per_node: r?.discussion?.hourly_budget_per_node ?? 20,
        rate_limit_per_min: r?.discussion?.rate_limit_per_min ?? 6,
      },
    }
  } catch (e: any) {
    toast.error('加载看板配置失败: ' + e)
  } finally {
    syncRequiredRulesFromFlags()
    loading.value = false
  }
}

// 开关组（渲染顺序 = 自动化链路顺序：拆解 → 验收 → 收货 → 父单收口 →
// 取证/项目收口（P4）→ 无限模式）。
const toggles = computed(() =>
  flags.value
    ? [
        {
          key: 'plan.auto_confirm',
          label: '拆解自动发车',
          desc: 'AI 拆解完成后自动确认子单并派发，不再等人工点确认（拆解失败仍会转人工）',
          value: flags.value.plan.auto_confirm,
        },
        {
          key: 'auto_review',
          label: '自动验收',
          desc: '子单报完成后自动跑验收：验收 agent 对照验收标准出 PASS / FAIL / UNSURE 结论',
          value: flags.value.auto_review,
        },
        {
          key: 'auto_accept',
          label: 'PASS 自动收货',
          desc: '验收 PASS 时自动把子单收货为 done（关 = 只把验收意见发到单里，等人工点收货）',
          value: flags.value.auto_accept,
        },
        {
          key: 'auto_close_parent',
          label: '父单自动收口',
          desc: '子单全部落定后 AI 汇总各子单交付自动验收父单：PASS 自动 done；FAIL/UNSURE 仍 @创建人处理；有被取消的子单时不自动收口',
          value: flags.value.auto_close_parent,
        },
        {
          key: 'review.selfcheck',
          label: '验收取证（向执行节点追问）',
          desc: '验收 agent 证据不足时可向执行该单的节点发一轮取证请求，等证据回报后做二次验收再下结论（只追问一轮，不循环）',
          value: flags.value.review.selfcheck,
        },
        {
          key: 'review.auto_close_project',
          label: '项目自动收口',
          desc: '项目下全部顶层任务完成后 AI 汇总验收整个项目：PASS 自动 completed；FAIL/UNSURE 项目回退 in_progress 并 @创建人列缺口（不自动重开任务）',
          value: flags.value.review.auto_close_project,
        },
        {
          key: 'conflict_auto_resolve',
          label: '合并冲突 AI 自动解决',
          desc: '多个交付改动撞车时自动尝试 AI 硬解并继续流转（失败重派/换节点，预算打满冻结转人工）；关闭 = 冲突即冻结项目转人工，人工解决后解冻补合并',
          value: flags.value.conflict_auto_resolve,
        },
        {
          key: 'unlimited_mode',
          label: '无限模式',
          desc: '验收 FAIL 无限重派（不看重派上限）、UNSURE 继续重派不转人工。预算护栏失效，急停开关随时可止血',
          value: flags.value.unlimited_mode,
        },
        {
          key: 'dossier.auto_on_complete',
          label: '收口自动导出任务档案',
          desc: '项目收口后自动把全部任务记录（活动/评论/派发/执行档案/用量）整合到 workspace/logs/dossiers/ 自包含目录，离线可读',
          value: flags.value.dossier_auto_on_complete,
        },
      ]
    : [],
)

/** 服务端档位归一：off/role/full 字符串；防御旧布尔（false→off、true→full）。 */
function normalizeFallbackMode(v: any): 'off' | 'role' | 'full' {
  if (v === 'off' || v === 'role' || v === 'full') return v
  if (v === true) return 'full'
  return 'off'
}

async function setFlag(key: string, value: boolean | number | string | null | Record<string, string[]>) {
  try {
    await request('board', 'config.set', { key, value })
    toast.success('已保存')
  } catch (e: any) {
    toast.error('保存失败: ' + e)
  } finally {
    // 无论成败都回读真实值（失败=回滚显示，成功=对齐服务端归一化值）。
    await load()
  }
}

function onToggle(key: string, ev: Event) {
  void setFlag(key, (ev.target as HTMLInputElement).checked)
}

function onNumber(key: string, ev: Event) {
  const raw = (ev.target as HTMLInputElement).value.trim()
  const n = Number(raw)
  if (raw === '' || !Number.isFinite(n) || n < 0) {
    toast.warn('请输入非负数字')
    void load()
    return
  }
  void setFlag(key, n)
}

function onModel(ev: Event) {
  const raw = (ev.target as HTMLInputElement).value.trim()
  void setFlag('plan.model', raw === '' ? null : raw)
}

function onFallbackMode(ev: Event) {
  const v = (ev.target as HTMLSelectElement).value
  if (v === 'off' || v === 'role' || v === 'full') void setFlag('dispatch_fallback', v)
}

function onFallbackTarget(ev: Event) {
  const raw = (ev.target as HTMLInputElement).value.trim()
  void setFlag('dispatch_fallback_target', raw === '' ? null : raw)
}

// ---- 必需检查规则（项 B「必需的必须跑」，2026-10-22 拍板）----
// 行态编辑（key + 逗号分隔 ids 原文），每次行变更整 map 保存（后端 whole-map
// 键 review.required_checks）。空 key 行填了 ids = 全局兜底规则；key+ids 全
// 空 = 未填完的草稿行（保存时跳过）；key 填了但 ids 空 / 键重复 = 校验失败
// 不保存（toast 提示，行态保留）。

interface RequiredRuleRow {
  key: string
  ids: string
}

const requiredRules = ref<RequiredRuleRow[]>([])

function syncRequiredRulesFromFlags() {
  const map = flags.value?.review.required_checks ?? {}
  requiredRules.value = Object.keys(map)
    .sort()
    .map((k) => ({ key: k, ids: map[k].join(', ') }))
}

/** 逗号/分号/空白切分锚点 id 列表（中英文逗号分号都认）。 */
function parseIds(raw: string): string[] {
  return raw
    .split(/[,;，；\s]+/)
    .map((s) => s.trim())
    .filter((s) => s !== '')
}

function ruleLabel(key: string): string {
  return key === '' ? '全局兜底' : key
}

async function saveRequiredRules() {
  const map: Record<string, string[]> = {}
  for (const row of requiredRules.value) {
    const key = row.key.trim()
    const ids = parseIds(row.ids)
    if (key === '' && ids.length === 0) continue // 未填完的草稿行
    if (ids.length === 0) {
      toast.warn(`规则「${ruleLabel(key)}」缺少锚点 id，未保存`)
      return
    }
    if (key in map) {
      toast.warn(`规则键「${ruleLabel(key)}」重复，未保存`)
      return
    }
    map[key] = ids
  }
  await setFlag('review.required_checks', map)
}

function addRequiredRule() {
  requiredRules.value.push({ key: '', ids: '' })
}

function removeRequiredRule(idx: number) {
  requiredRules.value.splice(idx, 1)
  void saveRequiredRules()
}

function onRequiredRuleInput() {
  void saveRequiredRules()
}

onMounted(load)
</script>

<template>
  <div>
    <div v-if="loading" style="text-align: center; padding: var(--space-8);">
      <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
    </div>

    <template v-else-if="flags">
      <!-- 无限模式警示条 -->
      <div v-if="flags.unlimited_mode" class="warn-banner">
        ⚠️ 无限模式已开启：FAIL 重派不再设上限、UNSURE 不再转人工，费用与时长无护栏。
        <template v-if="!flags.auto_accept">
          PASS 自动收货未开启——验收通过的单子仍会停下等人工收货。
        </template>
        异常时可用侧栏急停按钮或 CLI estop 随时冻结全部 agent 活动。
      </div>

      <!-- 自动化开关 -->
      <h3 class="section-title">自动化开关</h3>
      <div class="flag-list">
        <label v-for="t in toggles" :key="t.key" class="flag-row">
          <div class="flag-text">
            <div class="flag-label">{{ t.label }}</div>
            <div class="flag-desc">{{ t.desc }}</div>
          </div>
          <input type="checkbox" :checked="t.value" @change="onToggle(t.key, $event)" />
        </label>
      </div>

      <!-- 无人匹配兜底派发档位（2026-09-29 三档化） -->
      <div class="param-list">
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">无人匹配兜底派发</div>
            <div class="flag-desc">自动派发匹配不到节点时是否降级推进：诚实停车（等节点上线）/ 保角色降级（默认：职能与门槛放开，worker/coordinator 角色纪律保留）/ 全松弛（角色也放开）。降级派发前会留 ⚠ 评论说明；无任何在线节点时始终诚实停车</div>
          </div>
          <select class="form-input param-input" :value="flags.dispatch_fallback" @change="onFallbackMode($event)">
            <option value="off">诚实停车（off）</option>
            <option value="role">保角色降级（role，默认）</option>
            <option value="full">全松弛（full）</option>
          </select>
        </div>
      </div>

      <!-- 兜底客户端钉住（dispatch_fallback 非 off 时生效） -->
      <div v-if="flags.dispatch_fallback !== 'off'" class="param-list" style="margin-top: calc(var(--space-2) * -1);">
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">兜底客户端</div>
            <div class="flag-desc">钉住兜底派发的目标节点（节点名称或 ID，大小写不敏感）；留空 = 在线节点里自动选负载最低的。钉住的节点不在线时任务继续等待（不悄悄换人）</div>
          </div>
          <input
            class="form-input param-input param-input-wide"
            type="text"
            placeholder="留空 = 自动选在线节点"
            :value="flags.dispatch_fallback_target ?? ''"
            @change="onFallbackTarget($event)"
          />
        </div>
      </div>

      <!-- 必需检查规则（项 B「必需的必须跑」）：命中标签的单子验收时，
           列出的锚点必须全部跑过（声明+执行），否则强制停车转人工 -->
      <h3 class="section-title">必需检查规则</h3>
      <div class="param-list">
        <div v-for="(row, idx) in requiredRules" :key="idx" class="param-row required-rule-row">
          <input
            class="form-input rule-key-input"
            type="text"
            placeholder="匹配标签（留空 = 全局兜底）"
            v-model="row.key"
            @change="onRequiredRuleInput"
          />
          <input
            class="form-input rule-ids-input"
            type="text"
            placeholder="必需锚点 id，逗号分隔"
            v-model="row.ids"
            @change="onRequiredRuleInput"
          />
          <button class="btn btn-ghost btn-sm" title="删除规则" @click="removeRequiredRule(idx)">✕</button>
        </div>
        <div class="required-rule-hint">
          命中规则（任务标签与匹配键交集，大小写不敏感）的单子验收时，列出的锚点必须在验收标准中声明且实际执行（过或不过都算跑过）；任一未满足则验收强制停车转人工，不自动通过。匹配键留空 = 全局兜底（对所有单子生效）。锚点 id 填 [CHECK] 行的文件路径 / 命令 / pattern 文本。
        </div>
        <div>
          <button class="btn btn-secondary btn-sm" @click="addRequiredRule">＋ 添加规则</button>
        </div>
      </div>

      <!-- 参数 -->
      <h3 class="section-title">参数</h3>
      <div class="param-list">
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">验收 FAIL 重派上限</div>
            <div class="flag-desc">同一子单验收 FAIL 后自动重派的最大次数；0 = 关闭自动重派（无限模式下忽略此上限）</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            min="0"
            :value="flags.max_redispatch"
            @change="onNumber('max_redispatch', $event)"
          />
        </div>
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">验收取证轮数</div>
            <div class="flag-desc">验收 agent 用只读工具（读文件/搜索等）就地核实的最大轮数；1 = 纯文本验收不开工具（执行节点是本机时才生效）；0 = 不限轮数（取证到能下结论为止，doom-loop/急停守卫仍在）</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            min="0"
            :value="flags.review.max_turns"
            @change="onNumber('review.max_turns', $event)"
          />
        </div>
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">验收检查员数量</div>
            <div class="flag-desc">每次验收并行请多位检查员独立评审后多数票裁决（平票转人工）；1 = 单检查员。最大 5，每多一位多一倍评审开销</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            min="1"
            max="5"
            :value="flags.review.checkers"
            @change="onNumber('review.checkers', $event)"
          />
        </div>
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">连续 UNSURE 停车阈值</div>
            <div class="flag-desc">同一单连续这么多次评审无法定案（UNSURE）就停止自动重派、转人工裁决（人工处理后自动复评/手动重派会清零计数）；0 = 不限（一直自动重派，配合重派上限兜底）</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            min="0"
            :value="flags.review.max_unsure_streak"
            @change="onNumber('review.max_unsure_streak', $event)"
          />
        </div>
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">派发超时（秒）</div>
            <div class="flag-desc">派发后等待执行节点回报的超时秒数，超时标记派发失败并通知</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            min="0"
            :value="flags.dispatch_timeout_secs"
            @change="onNumber('dispatch_timeout_secs', $event)"
          />
        </div>
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">拆解 / 验收模型</div>
            <div class="flag-desc">拆解与验收 agent 使用的模型别名；留空 = 跟随会话主模型</div>
          </div>
          <input
            class="form-input param-input param-input-wide"
            type="text"
            placeholder="留空 = 跟随主模型"
            :value="flags.plan.model ?? ''"
            @change="onModel($event)"
          />
        </div>
      </div>

      <h3 class="section-title">预算护栏</h3>
      <div class="param-list">
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">单任务子单数上限</div>
            <div class="flag-desc">AI 拆解一个任务最多拆出的子单数，超过即停自动流转转人工（0 = 不设限）</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            min="0"
            :value="flags.budget.max_subissues_per_parent"
            @change="onNumber('budget.max_subissues_per_parent', $event)"
          />
        </div>
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">任务全链累计派发上限</div>
            <div class="flag-desc">一个任务（含全部子单）累计自动派发的最大次数，超过即停转人工（0 = 不设限；无限模式下降级为仅提醒）</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            min="0"
            :value="flags.budget.max_total_redispatch"
            @change="onNumber('budget.max_total_redispatch', $event)"
          />
        </div>
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">任务墙钟时限（秒）</div>
            <div class="flag-desc">任务从创建起算的最长存活秒数，超时即停自动流转转人工（0 = 不设限；无限模式下降级为仅提醒）</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            min="0"
            :value="flags.budget.wall_clock_budget_secs"
            @change="onNumber('budget.wall_clock_budget_secs', $event)"
          />
        </div>
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">任务 token 预算</div>
            <div class="flag-desc">一个任务（含全部子单）在执行节点消耗的 token 累计上限（输入+输出），超时即停转人工（0 = 不设限；需集群节点回报用量）</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            min="0"
            :value="flags.budget.max_tokens_per_parent"
            @change="onNumber('budget.max_tokens_per_parent', $event)"
          />
        </div>
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">送达失败自动重试上限</div>
            <div class="flag-desc">单个任务派发送达失败后的自动重试次数（退避 1 分钟起步、最长间隔 30 分钟），超限自动转人工（0 = 关闭保险丝，无限重试；不推荐）</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            min="0"
            :value="flags.budget.max_send_retries"
            @change="onNumber('budget.max_send_retries', $event)"
          />
        </div>
      </div>

      <h3 class="section-title">讨论频道额度</h3>
      <div class="param-list">
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">消息保留天数</div>
            <div class="flag-desc">讨论频道消息的保留天数，过期自动清理</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            :value="flags.discussion.retention_days"
            @change="onNumber('discussion.retention_days', $event)"
          />
        </div>
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">单线程 agent 发言上限</div>
            <div class="flag-desc">单个讨论线程内 agent 的最大发言轮数，防止 agent 之间无限互聊</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            :value="flags.discussion.max_agent_turns_per_thread"
            @change="onNumber('discussion.max_agent_turns_per_thread', $event)"
          />
        </div>
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">每节点每小时发言额度</div>
            <div class="flag-desc">每个节点每小时在讨论频道可发言的条数上限</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            :value="flags.discussion.hourly_budget_per_node"
            @change="onNumber('discussion.hourly_budget_per_node', $event)"
          />
        </div>
        <div class="param-row">
          <div class="flag-text">
            <div class="flag-label">每分钟发言限速</div>
            <div class="flag-desc">每分钟最大发言条数（平滑限速，与小时额度叠加生效）</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            :value="flags.discussion.rate_limit_per_min"
            @change="onNumber('discussion.rate_limit_per_min', $event)"
          />
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.warn-banner {
  background: var(--bg-warning, rgba(234, 179, 8, 0.12));
  border: 1px solid var(--warning, #eab308);
  border-radius: var(--radius-md);
  padding: var(--space-3) var(--space-4);
  margin-bottom: var(--space-4);
  font-size: var(--text-sm);
  line-height: 1.6;
}
.section-title {
  margin: var(--space-5) 0 var(--space-2);
  font-size: var(--text-base);
}
.flag-list,
.param-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.flag-row,
.param-row {
  display: flex;
  align-items: center;
  gap: var(--space-4);
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-3) var(--space-4);
}
.flag-row {
  cursor: pointer;
}
.flag-text {
  flex: 1;
}
.flag-label {
  font-weight: 500;
}
.flag-desc {
  color: var(--text-muted);
  font-size: var(--text-sm);
  margin-top: var(--space-1);
  line-height: 1.5;
}
.param-input {
  width: 110px;
  flex-shrink: 0;
}
.param-input-wide {
  width: 240px;
}
.required-rule-row {
  align-items: center;
}
.rule-key-input {
  width: 200px;
  flex-shrink: 0;
}
.rule-ids-input {
  flex: 1;
}
.required-rule-hint {
  color: var(--text-muted);
  font-size: var(--text-sm);
  line-height: 1.5;
  padding: 0 var(--space-1);
}
</style>
