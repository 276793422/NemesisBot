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
  dispatch_fallback: boolean
  dispatch_fallback_target: string | null
  max_redispatch: number
  dispatch_timeout_secs: number
  plan: { auto_confirm: boolean; model: string | null }
  review: { max_turns: number; selfcheck: boolean; auto_close_project: boolean; checkers: number }
  budget: {
    max_subissues_per_parent: number
    max_total_redispatch: number
    wall_clock_budget_secs: number
    max_tokens_per_parent: number
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
      dispatch_fallback: !!r?.dispatch_fallback,
      dispatch_fallback_target: r?.dispatch_fallback_target ?? null,
      max_redispatch: r?.max_redispatch ?? 2,
      dispatch_timeout_secs: r?.dispatch_timeout_secs ?? 3600,
      plan: {
        auto_confirm: !!r?.plan?.auto_confirm,
        model: r?.plan?.model ?? null,
      },
      review: {
        max_turns: r?.review?.max_turns ?? 1,
        selfcheck: !!r?.review?.selfcheck,
        auto_close_project: !!r?.review?.auto_close_project,
        checkers: r?.review?.checkers ?? 1,
      },
      budget: {
        max_subissues_per_parent: r?.budget?.max_subissues_per_parent ?? 20,
        max_total_redispatch: r?.budget?.max_total_redispatch ?? 0,
        wall_clock_budget_secs: r?.budget?.wall_clock_budget_secs ?? 0,
        max_tokens_per_parent: r?.budget?.max_tokens_per_parent ?? 0,
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
          key: 'unlimited_mode',
          label: '无限模式',
          desc: '验收 FAIL 无限重派（不看重派上限）、UNSURE 继续重派不转人工。预算护栏失效，急停开关随时可止血',
          value: flags.value.unlimited_mode,
        },
        {
          key: 'dispatch_fallback',
          label: '无人匹配兜底派发',
          desc: '自动派发匹配不到（角色/标签）节点时，为了任务做下去兜底派给在线节点（下方可钉住指定客户端；钉住的不在线则继续等）。派发前会留 ⚠ 评论说明',
          value: flags.value.dispatch_fallback,
        },
      ]
    : [],
)

async function setFlag(key: string, value: boolean | number | string | null) {
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

function onFallbackTarget(ev: Event) {
  const raw = (ev.target as HTMLInputElement).value.trim()
  void setFlag('dispatch_fallback_target', raw === '' ? null : raw)
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

      <!-- 兜底客户端钉住（dispatch_fallback 开时生效） -->
      <div v-if="flags.dispatch_fallback" class="param-list" style="margin-top: calc(var(--space-2) * -1);">
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
            <div class="flag-desc">验收 agent 用只读工具（读文件/搜索等）就地核实的最大轮数；1 = 纯文本验收不开工具（执行节点是本机时才生效）</div>
          </div>
          <input
            class="form-input param-input"
            type="number"
            min="1"
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
</style>
