<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { usePanels, type PanelSlotState, type PanelViewDecl } from '../composables/usePanels'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import PanelTable from '../components/panel/PanelTable.vue'
import PanelForm from '../components/panel/PanelForm.vue'
import PanelTimeline from '../components/panel/PanelTimeline.vue'
import PanelDetail from '../components/panel/PanelDetail.vue'
import PanelActions from '../components/panel/PanelActions.vue'
import PanelChart from '../components/panel/PanelChart.vue'

// 面板容器（P2）：/panel/:route → 注册表查页 → 五词汇分块渲染。
// 取数/动作统一走 plugins.ui.invoke（estop 闸在代理层）；业务失败是
// 200 内 `{"error":...}` 形态——toast + 槽位错误面，不混入 WS 错误。
// 绑定两种形态：cmd:// 前缀剥掉发 guest；flow:// 全串透传——宿主 flow
// 引擎在 plugins.ui.invoke 侧代答（P3），前端不做任何拆解。
const route = useRoute()
const { panels, fetchPanels, pageByRoute, invokePanel } = usePanels()
const { request } = useWSAPI()
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

// ---- 插件配置卡（config-schema 参数化）：面板页内联编辑 {{config.*}} 键。 ----
// schema 明细来自 wasm.config.get（旧网关无 schema 字段 → 回落 schema_keys）；
// 值留空 = 回落声明缺省；凭据（x-*）不经此通道，卡内指路 connectors/vault 页。
interface WasmConfigEntry {
  key: string
  desc: string
  default: string | null
}
interface WasmConfigData {
  slug: string
  enabled: boolean
  entries: Record<string, string>
  schema_keys?: string[]
  schema?: WasmConfigEntry[]
  secret_keys?: string[]
}

const configOpen = ref(false)
const configLoading = ref(false)
const configData = ref<WasmConfigData | null>(null)

const configRows = computed<WasmConfigEntry[]>(() => {
  const d = configData.value
  if (!d) return []
  if (d.schema?.length) return d.schema
  return (d.schema_keys ?? []).map((k) => ({ key: k, desc: '', default: null }))
})

async function toggleConfig() {
  configOpen.value = !configOpen.value
  if (configOpen.value) await loadConfig()
}

async function loadConfig() {
  const slug = entry.value?.plugin.slug
  if (!slug) return
  configLoading.value = true
  try {
    configData.value = await request('plugins', 'wasm.config.get', { slug })
  } catch (e: any) {
    toast.error(`配置读取失败：${String(e?.message ?? e)}`)
  } finally {
    configLoading.value = false
  }
}

async function configSave(key: string, value: string) {
  const slug = entry.value?.plugin.slug
  if (!slug) return
  const trimmed = value.trim()
  try {
    if (!trimmed) {
      // 空值 = 清除该键覆盖，回落声明缺省。
      await request('plugins', 'wasm.config.set', { slug, key, remove: true })
      toast.success(`${key}：已清除（回落声明缺省）`)
    } else {
      await request('plugins', 'wasm.config.set', { slug, key, value: trimmed })
      toast.success(`${key}：已保存（热生效）`)
    }
    await loadConfig()
  } catch (e: any) {
    toast.error(`${key}：${String(e?.message ?? e)}`)
  }
}

async function configRemove(key: string) {
  const slug = entry.value?.plugin.slug
  if (!slug) return
  try {
    await request('plugins', 'wasm.config.set', { slug, key, remove: true })
    toast.success(`${key}：已清除（回落声明缺省）`)
    await loadConfig()
  } catch (e: any) {
    toast.error(`${key}：${String(e?.message ?? e)}`)
  }
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
  <!-- 布局契约：根类必须 page- 前缀开头（.main-content > [class^="page-"] 给
       height:100% + flex 列，main-content 自身 overflow:hidden 永不滚动），
       内容滚动唯一区域 = page-body。违约形态 = 长内容被裁切且无滚动条。 -->
  <div class="page-panel panel-view" data-test="panel-view">
    <template v-if="entry">
      <header class="page-header">
        <h2>{{ entry.page.title }}</h2>
        <div class="page-header-actions">
          <span class="panel-sub">
            {{ entry.plugin.name }}
            <template v-if="!entry.plugin.payload && needsGuest">（载荷缺失——插件页检查安装状态）</template>
          </span>
          <button class="btn btn-sm" data-test="panel-config-toggle" @click="toggleConfig">
            {{ configOpen ? '收起配置' : '⚙ 配置' }}
          </button>
        </div>
      </header>
      <div class="page-body">
        <!-- 配置卡：面板声明 config-schema 的键在此内联编辑（热生效）；
             凭据不在此通道——指路 connectors / vault 页。 -->
        <section v-if="configOpen" class="panel-config-card" data-test="panel-config-card">
          <div class="panel-config-title">
            插件配置
            <span class="panel-config-hint">值留空 = 回落声明缺省；改动热生效（下轮装载取新值）</span>
          </div>
          <div v-if="configLoading" class="panel-config-empty">读取中…</div>
          <template v-else-if="configData">
            <div v-if="configData.secret_keys?.length" class="panel-config-secret">
              凭据键（{{ configData.secret_keys.join(', ') }}）经 vault 注入：
              <router-link to="/vault">保险库</router-link> 页管理原文；
              connector 地址/令牌在 <router-link to="/connectors">连接器</router-link> 页配置。
            </div>
            <div v-if="!configRows.length" class="panel-config-empty">该插件未声明配置键。</div>
            <div v-for="s in configRows" :key="s.key" class="panel-config-row">
              <span class="panel-config-key" :title="s.desc">{{ s.key }}</span>
              <span v-if="s.desc" class="panel-config-desc">{{ s.desc }}</span>
              <input
                class="form-input panel-config-input"
                :value="configData.entries[s.key] ?? ''"
                :placeholder="s.default ? '缺省 ' + s.default : '未设置'"
                @change="configSave(s.key, ($event.target as HTMLInputElement).value)"
              />
              <button v-if="configData.entries[s.key]" class="btn btn-sm" title="清除后回落声明缺省" @click="configRemove(s.key)">清除</button>
            </div>
            <div v-if="configRows.length" class="panel-config-secret">
              连接器凭据（如 Gitea PAT，vault 别名 <code>connector/&lt;id&gt;/&lt;槽位&gt;</code>）在
              <router-link to="/vault">保险库</router-link> 页录入；
              连接器地址/启用状态在 <router-link to="/connectors">连接器</router-link> 页查看。
            </div>
          </template>
        </section>
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
          <PanelChart
            v-else-if="v.kind === 'chart'"
            :decl="v"
            :state="slots[i] ?? { loading: false, error: '', data: null, from: 'none' }"
            @refresh="loadView(i)"
          />
          <!-- 未知 kind：诚实占位（旧 dashboard 配新插件声明的降级面）。 -->
          <section v-else class="panel-unknown-kind">未知视图类型：{{ v.kind }}</section>
        </template>
      </div>
    </template>
    <div v-else class="page-body">
      <div class="panel-missing" data-test="panel-missing">
        <h2>面板未找到</h2>
        <p>
          路由 <code>/panel/{{ routeSeg }}</code> 不在当前面板注册表中——插件可能未安装、未启用，
          或注册表尚未加载（稍候自动重试）。
        </p>
        <p>可在「插件」页确认 WASM 插件的安装与启用状态。</p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.panel-view {
  /* 全站 page-* 契约：根不加 max-width/padding——头部带通栏、正文由 page-body 统一内边距 */
}
.panel-sub {
  font-size: var(--text-xs);
  color: var(--text-muted);
}
.panel-config-card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: var(--space-3) var(--space-4);
  margin-bottom: var(--space-3);
}
.panel-config-title {
  font-size: var(--text-sm);
  font-weight: 600;
  margin-bottom: var(--space-2);
}
.panel-config-hint {
  font-size: var(--text-xs);
  font-weight: 400;
  color: var(--text-muted);
  margin-left: var(--space-2);
}
.panel-config-empty {
  color: var(--text-muted);
  font-size: var(--text-xs);
  padding: var(--space-1) 0;
}
.panel-config-secret {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-bottom: var(--space-2);
}
.panel-config-secret a {
  color: var(--primary);
}
.panel-config-secret code {
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 0 4px;
  font-size: var(--text-xs);
}
.panel-config-row {
  display: flex;
  gap: var(--space-2);
  align-items: center;
  margin-bottom: var(--space-2);
}
.panel-config-key {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  min-width: 120px;
}
.panel-config-desc {
  font-size: var(--text-xs);
  color: var(--text-muted);
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.panel-config-input {
  width: 220px;
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
