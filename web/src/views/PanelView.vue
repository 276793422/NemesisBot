<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { usePanels, type PanelSlotState, type PanelViewDecl } from '../composables/usePanels'
import { useToast } from '../composables/useToast'
import PanelTable from '../components/panel/PanelTable.vue'
import PanelForm from '../components/panel/PanelForm.vue'
import PanelTimeline from '../components/panel/PanelTimeline.vue'
import PanelDetail from '../components/panel/PanelDetail.vue'
import PanelActions from '../components/panel/PanelActions.vue'

// 面板容器（P2）：/panel/:route → 注册表查页 → 五词汇分块渲染。
// 取数/动作统一走 plugins.ui.invoke（estop 闸在代理层）；业务失败是
// 200 内 `{"error":...}` 形态——toast + 槽位错误面，不混入 WS 错误。
// 绑定两种形态：cmd:// 前缀剥掉发 guest；flow:// 全串透传——宿主 flow
// 引擎在 plugins.ui.invoke 侧代答（P3），前端不做任何拆解。
const route = useRoute()
const { panels, fetchPanels, pageByRoute, invokePanel } = usePanels()
const toast = useToast()

const routeSeg = computed(() => String(route.params.route ?? ''))
const entry = computed(() => pageByRoute(routeSeg.value))

// 载荷缺失提示只在「确有 guest 依赖」时出现（P3）：cmd:// 绑定与 actions
// 按钮命令都落 guest 执行；纯 flow:// 声明式面板由宿主引擎代答，无载荷是
// 设计形态不是缺失（actions 按钮的 flow:// 命令同属宿主代答，P4 样板批补）。
const needsGuest = computed(
  () =>
    entry.value?.page.views.some(
      (v) =>
        v.bind.startsWith('cmd://') ||
        (v.kind === 'actions' && v.buttons.some((b) => b.command && !b.command.startsWith('flow://'))),
    ) ?? false,
)

const slots = ref<PanelSlotState[]>([])

// 取数种类门控：table/detail/timeline 的 cmd:// 与 flow:// 绑定都会在
// 挂载/刷新时自动取数（flow 侧宿主引擎代答同语义）；form 的绑定是提交
// 目标（空参自动执行=真 bug），actions 无取数面。
function slotFrom(v: PanelViewDecl): PanelSlotState['from'] {
  if (v.kind === 'form' || v.kind === 'actions') return 'none'
  if (v.bind.startsWith('cmd://')) return 'cmd'
  if (v.bind.startsWith('flow://')) return 'flow'
  return 'none'
}

// 发送命令整形：cmd:// 剥前缀落 guest；flow:// 全串透传（宿主引擎按
// `<template>/<view>` 路由，拆解权在服务端）。
function invokeCommandOf(bind: string): string {
  return bind.startsWith('cmd://') ? bind.slice('cmd://'.length) : bind
}

function buildSlots(views: PanelViewDecl[]): PanelSlotState[] {
  return views.map((v) => ({ loading: false, error: '', data: null, from: slotFrom(v) }))
}

async function loadView(idx: number) {
  const ent = entry.value
  const v = ent?.page.views[idx]
  const slot = slots.value[idx]
  if (!ent || !v || !slot || (slot.from !== 'cmd' && slot.from !== 'flow')) return
  slot.loading = true
  slot.error = ''
  try {
    const reply: any = await invokePanel(ent.plugin.slug, invokeCommandOf(v.bind), {})
    if (reply && typeof reply.error === 'string' && reply.error) {
      slot.error = reply.error
      slot.data = null
    } else {
      slot.data = reply
    }
  } catch (e: any) {
    slot.error = String(e?.message ?? e)
    slot.data = null
  } finally {
    slot.loading = false
  }
}

async function loadAll() {
  const page = entry.value?.page
  slots.value = page ? buildSlots(page.views) : []
  for (let i = 0; i < slots.value.length; i++) void loadView(i)
}

// 动作执行单点：actions 按钮 / form 提交都走这里。成功后整页重取
//（表格/时间线反映最新状态——guest 侧状态变更的最简同步方式）。
const busyCommand = ref('')
async function runCommand(command: string, args: Record<string, unknown> = {}, label = '') {
  const ent = entry.value
  if (!ent || !command) return
  busyCommand.value = command
  try {
    const reply: any = await invokePanel(ent.plugin.slug, command, args)
    if (reply && typeof reply.error === 'string' && reply.error) {
      toast.error(`${label || command}：${reply.error}`)
    } else {
      const msg =
        typeof reply?.message === 'string' && reply.message ? reply.message : '命令已执行'
      toast.success(`${label || command}：${msg}`)
      await loadAll()
    }
  } catch (e: any) {
    toast.error(`${label || command}：${String(e?.message ?? e)}`)
  } finally {
    busyCommand.value = ''
  }
}

function onFormSubmit(idx: number, values: Record<string, unknown>) {
  const v = entry.value?.page.views[idx]
  if (!v) return
  void runCommand(invokeCommandOf(v.bind), values, '提交')
}

onMounted(async () => {
  if (!entry.value) await fetchPanels()
  await loadAll()
})

// 注册表晚到（深链先于 fetch / 插件刚启用）与页内切换：重解析当前页。
// 首次 fire 与 onMounted 自己的 loadAll 重叠（fetch 到达触发）——跳过防双发。
let firstRegistryFire = true
watch(panels, () => {
  if (firstRegistryFire) {
    firstRegistryFire = false
    return
  }
  if (entry.value) void loadAll()
})
watch(routeSeg, () => {
  if (entry.value) void loadAll()
})
</script>

<template>
  <div class="panel-view" data-test="panel-view">
    <template v-if="entry">
      <header class="panel-head">
        <h2>{{ entry.page.title }}</h2>
        <span class="panel-sub">
          {{ entry.plugin.name }}
          <template v-if="!entry.plugin.payload && needsGuest">（载荷缺失——插件页检查安装状态）</template>
        </span>
      </header>
      <template v-for="(v, i) in entry.page.views" :key="i">
        <PanelTable
          v-if="v.kind === 'table'"
          :decl="v"
          :state="slots[i] ?? { loading: false, error: '', data: null, from: 'none' }"
          @refresh="loadView(i)"
        />
        <PanelDetail
          v-else-if="v.kind === 'detail'"
          :decl="v"
          :state="slots[i] ?? { loading: false, error: '', data: null, from: 'none' }"
          @refresh="loadView(i)"
        />
        <PanelTimeline
          v-else-if="v.kind === 'timeline'"
          :decl="v"
          :state="slots[i] ?? { loading: false, error: '', data: null, from: 'none' }"
          @refresh="loadView(i)"
        />
        <PanelForm
          v-else-if="v.kind === 'form'"
          :decl="v"
          :busy="busyCommand !== ''"
          @submit="(vals: Record<string, unknown>) => onFormSubmit(i, vals)"
        />
        <PanelActions
          v-else-if="v.kind === 'actions'"
          :buttons="v.buttons"
          :busy="busyCommand !== ''"
          @run="(b) => runCommand(b.command, {}, b.label)"
        />
        <!-- 未知 kind：诚实占位（旧 dashboard 配新插件声明的降级面）。 -->
        <section v-else class="panel-unknown-kind">未知视图类型：{{ v.kind }}</section>
      </template>
    </template>
    <div v-else class="panel-missing" data-test="panel-missing">
      <h2>面板未找到</h2>
      <p>
        路由 <code>/panel/{{ routeSeg }}</code> 不在当前面板注册表中——插件可能未安装、未启用，
        或注册表尚未加载（稍候自动重试）。
      </p>
      <p>可在「插件」页确认 WASM 插件的安装与启用状态。</p>
    </div>
  </div>
</template>

<style scoped>
.panel-view {
  padding: var(--space-3);
  max-width: 960px;
}
.panel-head {
  margin-bottom: var(--space-3);
}
.panel-head h2 {
  margin: 0 0 4px 0;
  font-size: var(--text-xl);
  color: var(--text-primary);
}
.panel-sub {
  font-size: var(--text-xs);
  color: var(--text-muted);
}
.panel-missing {
  color: var(--text-muted);
  font-size: var(--text-sm);
  padding: var(--space-4) 0;
}
.panel-missing h2 {
  color: var(--text-primary);
  font-size: var(--text-lg);
  margin: 0 0 var(--space-2) 0;
}
.panel-missing code {
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 1px 6px;
  font-size: var(--text-xs);
}
.panel-unknown-kind {
  background: var(--surface);
  border: 1px dashed var(--border);
  border-radius: 8px;
  padding: var(--space-3);
  margin-bottom: var(--space-3);
  color: var(--text-muted);
  font-size: var(--text-xs);
}
</style>
