<script setup lang="ts">
// 行动连接器页（AI 原生行动代理，2026-10-14；三页签改造 2026-10-19）：
// ① 概览：connector 是什么 + 安全模型一句话 + 「＋ 新建连接器」入口
//    （模态粘 connector.toml → connectors.create，后端权威校验、拒绝原文回显）；
// ② 注册表：已装 connector 卡片（trust / 绑定目标徽标 / ops 表 / 损坏红字），
//    每卡「启用/停用」（connectors.set_enabled）与「删除」（两段确认 →
//    connectors.remove，凭条历史保留）；下方回执流水；
// ③ 商城：占位（未来远程源浏览 + 一键安装；现在可走 CLI connector add-source）。
// 数据全部来自 WSAPI（connectors.list / connectors.receipts.list +
// 管理面命令）。挂载即拉全量，页签只门控显示；
// registry 无缓存扫描——增/删/改下一次 list 即见，零重启。
// 领域数据不进框架页：纸面券商账本等 connector 专属数据走各自出口
// （CLI `connector ledger`），框架页面只保留通用审计面（回执流水）。
import { ref, watch, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import {
  connectorRiskClass, connectorOutcomeClass, connectorOutcomeLabel,
  type ConnectorReceipt,
} from '../components/logs/mockData'

interface ConnectorOp {
  id: string
  intent: string[]
  risk: string
  class: string
  confirm_required: boolean
  finance_budget: boolean
}

/** B2 独占接管：per-tool 策略行（mirrors[].policy 三态，Dashboard 选择器数据面）。
 *  B3：基线档位（risk/class/confirm_required）由 MCP annotations 推导随行——
 *  自述只读 = low，自述破坏性 = high+确认卡，未声明 = medium。 */
interface MirrorRow {
  id: string
  args_schema?: unknown
  policy: 'default' | 'exempt' | 'blocked'
  risk?: string
  class?: string
  confirm_required?: boolean
  description?: string
  /** G1 隐藏标记（agents.hidden_tools 命中）——绝对闸：选择器放行也调不动，先去解除隐藏 */
  hidden_by_config?: boolean
}

interface ConnectorInfo {
  id: string
  /** 注册表目录名（WSAPI 管理面命令的定位键；损坏条目 id 即目录名） */
  dir?: string
  name: string
  version: string
  kind: string
  enabled: boolean
  /** 签名信任徽标：verified | community | unsigned；null/缺省 = 损坏条目（无有效清单可评估） */
  trust?: string | null
  /** M4b tool 绑定在位探测：绑定的 MCP server 是否注册；null/缺省 = 无 tool 绑定（不适用）。
   *  false 只是「绑定目标还没装」的指路信息——连接器不拦截任何工具，MCP 状态真相源在 MCP 页。 */
  mcp_ready?: boolean | null
  description: string
  ops: ConnectorOp[]
  /** B2：独占接管的 MCP server 清单（空/缺省 = 非独占 connector） */
  exclusive_mcp?: string[]
  /** B2：独占接管 per-tool 策略行（含 sidecar 记录但 server 离线的例外） */
  mirrors?: MirrorRow[]
  /** 在场 = 该 connector 损坏（加载/解析失败），诚实展示错误原文 */
  error?: string
}

type TabId = 'overview' | 'registry' | 'shop'

const { request } = useWSAPI()
const toast = useToast()

const activeTab = ref<TabId>('overview')
const loading = ref(true)
// connectors.list 是本页主数据源：失败整页错误态（照 SandboxView 错误卡）；
// receipts 失败只降级回执区块的诚实错误文案。
const loadError = ref('')
const connectors = ref<ConnectorInfo[]>([])
const receipts = ref<ConnectorReceipt[]>([])
const receiptsError = ref('')
// B4 总开关（config.json connectors.enabled；list 顶层带出）。true 缺省 =
// 后端缺省语义（缺键/读失败 = 开）。
const masterEnabled = ref(true)
const masterBusy = ref(false)

async function toggleMaster() {
  if (masterBusy.value) return
  masterBusy.value = true
  try {
    await request('connectors', 'set_master', { enabled: !masterEnabled.value })
    toast.success(masterEnabled.value ? '连接器已全关（总开关）' : '连接器总开关已开启')
    await refreshAll()
  } catch (e) {
    toast.error(errMsg(e))
  } finally {
    masterBusy.value = false
  }
}

function switchTab(t: TabId) {
  activeTab.value = t
  confirmDelete.value = '' // 换页撤销未完成的删除确认
}

// ── 新建连接器（模态） ────────────────────────────────────────────────
const showCreate = ref(false)
const newDir = ref('')
const newToml = ref('')
const createError = ref('')
const creating = ref(false)

// 最小模板：只读 local 型 demo（builtin.noop），粘单即成最简合法清单。
const DEFAULT_TOML_TEMPLATE = `[connector]
id = "my-connector"
name = "我的连接器"
version = "0.1.0"
kind = "local"
description = "最小只读示例"

[[op]]
id = "noop"
intent = ["示例操作"]
risk = "low"
class = "read"
action = "builtin.noop"
`

function openCreate() {
  createError.value = ''
  if (!newToml.value.trim()) newToml.value = DEFAULT_TOML_TEMPLATE
  showCreate.value = true
}

// 模板 id 跟随目录名（后端强制目录名==id，边界补强 2026-10-20）：只在
// 模板占位 id 未被手改时自动替换——用户已编辑过 id 则不动，交给预检提示。
watch(newDir, (dir) => {
  const placeholder = 'id = "my-connector"'
  if (dir && newToml.value.includes(placeholder)) {
    newToml.value = newToml.value.replace(placeholder, `id = "${dir}"`)
  }
})

// 清单 [connector] 段 id 提取（首个 `id = "..."` 行即段 id——段首必现）。
function manifestIdOf(toml: string): string | null {
  const m = toml.match(/^id\s*=\s*"([^"]+)"/m)
  return m ? m[1] : null
}

async function submitCreate() {
  createError.value = ''
  const dir = newDir.value.trim()
  if (!dir) {
    createError.value = '请填写目录名（小写字母/数字/-/_）'
    return
  }
  // 客户端预检（服务端仍权威校验）：目录名与清单 id 必须一致——
  // 目录即注册表，find/invoke 与 sidecar 都按 id 寻址。
  const id = manifestIdOf(newToml.value)
  if (id !== null && id !== dir) {
    createError.value = `目录名与清单 id 不一致（目录 "${dir}" vs id "${id}"）——两者必须相同`
    return
  }
  if (creating.value) return
  creating.value = true
  try {
    // 后端权威校验（目录名字符集 / 清单 schema / dir==id / 目录与 id 重复），拒绝原文回显。
    await request('connectors', 'create', { dir_name: dir, toml: newToml.value })
    toast.success(`连接器 ${dir} 已创建`)
    showCreate.value = false
    newDir.value = ''
    newToml.value = ''
    activeTab.value = 'registry'
    await refreshAll()
  } catch (e) {
    createError.value = errMsg(e)
  } finally {
    creating.value = false
  }
}

// ── 启用/停用 + 删除（注册表页卡片操作） ─────────────────────────────
const busyDir = ref('')
const confirmDelete = ref('')

function dirOf(c: ConnectorInfo): string {
  return c.dir || c.id
}

async function toggleEnabled(c: ConnectorInfo) {
  const dir = dirOf(c)
  if (!dir || busyDir.value) return
  busyDir.value = dir
  try {
    await request('connectors', 'set_enabled', { dir, enabled: !c.enabled })
    toast.success(c.enabled ? `已停用 ${dir}` : `已启用 ${dir}`)
    await refreshAll()
  } catch (e) {
    // 启停恒写 sidecar（清单不动，签名条目也可启停）——失败原文弹错。
    toast.error(errMsg(e))
  } finally {
    busyDir.value = ''
  }
}

// 两段确认（SkillsView 同款）：首击变「确认删除？」，再击执行。
async function deleteConnector(c: ConnectorInfo) {
  const dir = dirOf(c)
  if (!dir || busyDir.value) return
  if (confirmDelete.value !== dir) {
    confirmDelete.value = dir
    return
  }
  confirmDelete.value = ''
  busyDir.value = dir
  try {
    await request('connectors', 'remove', { dir })
    toast.success(`已删除 ${dir}（凭条历史保留）`)
    await refreshAll()
  } catch (e) {
    toast.error(errMsg(e))
  } finally {
    busyDir.value = ''
  }
}

// ── B2 独占接管 per-tool 策略（三态选择器） ──────────────────────────
const busyPolicy = ref('')

const POLICY_LABELS: Record<string, string> = {
  default: '默认镜像',
  exempt: '豁免（原生直调）',
  blocked: '封禁',
}

async function setPolicy(c: ConnectorInfo, tool: string, ev: Event) {
  const policy = (ev.target as HTMLSelectElement).value
  if (busyPolicy.value) return
  busyPolicy.value = `${c.id}/${tool}`
  try {
    await request('connectors', 'tool_policy', { connector: c.id, tool, policy })
    toast.success(`已${POLICY_LABELS[policy] ?? policy}：${tool}`)
    await refreshAll()
  } catch (e) {
    toast.error(errMsg(e))
    await refreshAll() // 回滚显示为后端真相
  } finally {
    busyPolicy.value = ''
  }
}

function intentFirst(op: ConnectorOp): string {
  return Array.isArray(op.intent) && op.intent.length > 0 ? op.intent[0] : ''
}

// M1 信任徽标（与远程源区同款色义：verified 绿 / community 蓝 / unsigned 灰）。
// null/缺省 = 损坏条目，不显示徽标（错误行已诚实展示原因）。
function trustBadge(t: string | null | undefined): { cls: string; text: string } | null {
  if (t === 'verified') return { cls: 'badge-success', text: '✔ 已验证' }
  if (t === 'community') return { cls: 'badge-info', text: '社区' }
  if (t === 'unsigned') return { cls: 'badge-neutral', text: '未签名' }
  return null
}

// RFC3339 → 本地时间展示；解析失败原样回显
function formatTs(ts: string): string {
  const d = new Date(ts)
  return isNaN(d.getTime()) ? ts : d.toLocaleString()
}

function errMsg(reason: unknown): string {
  if (reason == null) return ''
  if (typeof reason === 'string') return reason
  const e = reason as { message?: string }
  return e.message ?? String(reason)
}

async function refreshAll() {
  loading.value = true
  loadError.value = ''
  receiptsError.value = ''
  try {
    const [listRes, rcptRes] = await Promise.allSettled([
      request('connectors', 'list', {}),
      request('connectors', 'receipts.list', { limit: 50 }),
    ])
    if (listRes.status === 'rejected') {
      loadError.value = errMsg(listRes.reason)
      return
    }
    connectors.value = Array.isArray(listRes.value?.connectors) ? listRes.value.connectors : []
    // B4：顶层 enabled = 总开关当下态（缺键 = 后端缺省 true）。
    masterEnabled.value = listRes.value?.enabled !== false
    if (rcptRes.status === 'fulfilled') {
      receipts.value = Array.isArray(rcptRes.value?.receipts) ? rcptRes.value.receipts : []
    } else {
      receiptsError.value = errMsg(rcptRes.reason)
    }
  } finally {
    loading.value = false
  }
}

onMounted(refreshAll)
</script>

<template>
  <div class="page-connectors">
    <div class="page-header">
      <h2>Connectors</h2>
      <span v-if="!loadError" class="conn-count-badge">{{ connectors.length }} 个连接器</span>
    </div>
    <div class="page-body">

      <!-- 加载失败错误态（connectors.list 失败；照 SandboxView 错误卡风格） -->
      <div v-if="loadError" class="card">
        <div class="card-body conn-error" data-test="load-error">
          无法获取 Connectors 列表（connectors.list）— {{ loadError }}
          <button class="btn btn-sm" style="margin-left: var(--space-2);" @click="refreshAll" :disabled="loading">重试</button>
        </div>
      </div>

      <template v-else>
        <div class="tabs" data-test="conn-tabs">
          <button class="tab" :class="{ active: activeTab === 'overview' }" data-test="tab-overview" @click="switchTab('overview')">概览</button>
          <button class="tab" :class="{ active: activeTab === 'registry' }" data-test="tab-registry" @click="switchTab('registry')">注册表</button>
          <button class="tab" :class="{ active: activeTab === 'shop' }" data-test="tab-shop" @click="switchTab('shop')">商城</button>
        </div>

        <!-- ════════ Tab① 概览：介绍 + 新建入口 ════════ -->
        <div v-if="activeTab === 'overview'" class="card" data-test="overview-card">
          <div class="card-header"><h3 style="margin: 0;">什么是 Connectors</h3></div>
          <div class="card-body conn-intro">
            <p>
              Connector 是<strong>声明型</strong>的 agent 行动扩展：只写一份 <code>connector.toml</code>
              清单，声明「能做什么操作（op）、每个操作多危险（风险分级 low→critical）、要不要人确认」，
              网关负责其余一切——把 op 供给给 agent 按意图调用，并接上完整安全管线。
            </p>
            <p>
              安全模型：每个操作走安全管线（高危弹审批卡、拒绝 = 零副作用）；资金类操作受资金限额约束；
              每笔操作（无论成败）都在「注册表」页的回执流水留底；带签名的连接器显示信任徽标
              （✔ 已验证 / 社区 / 未签名）。
            </p>
            <p style="color: var(--text-secondary);">
              安装方式二选一：点击下方按钮在 Dashboard 直接新建（粘贴清单）；或把 connector 目录
              放进 <code>&lt;workspace&gt;/connectors/&lt;目录名&gt;/</code>，网关实时扫描注册、无需重启。
              开发细节见三方开发指南（docs/INFO/2026-10-14_connector三方开发指南.md）。
            </p>
            <!-- B4 总开关：只裁 connector_find / connector_invoke 两把治理工具；
                 原生 MCP / 文件等工具不受影响（连接器≠闸门）。热生效，无需重启。 -->
            <div
              class="master-switch"
              data-test="master-switch"
              style="margin-top: var(--space-3); padding: var(--space-3); border: 1px solid var(--border); border-radius: var(--radius-md); display: flex; align-items: center; gap: var(--space-3);"
            >
              <button
                class="btn btn-sm"
                :class="masterEnabled ? 'primary' : ''"
                data-test="master-toggle"
                :disabled="masterBusy"
                @click="toggleMaster"
              >{{ masterEnabled ? '总开关：开（点击全关）' : '总开关：关（点击开启）' }}</button>
              <span style="color: var(--text-muted); font-size: var(--text-sm);">
                {{ masterEnabled
                  ? '连接器治理面生效中——注册表子开关与 per-tool 例外可细调。'
                  : '已全关：connector_find / connector_invoke 对 agent 不可用（诚实报错），注册表子开关置灰；原生工具不受影响。' }}
              </span>
            </div>
            <div style="margin-top: var(--space-3); display: flex; align-items: center; gap: var(--space-3);">
              <button class="btn primary" data-test="open-create" @click="openCreate">＋ 新建连接器</button>
              <span style="color: var(--text-muted); font-size: var(--text-sm);">启用/停用与删除在「注册表」页各卡片上操作。</span>
            </div>
          </div>
        </div>

        <!-- ════════ Tab② 注册表：卡片（启停/删除）+ 回执 ════════ -->
        <template v-else-if="activeTab === 'registry'">
          <div class="card" data-test="connectors-card">
            <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
              <h3 style="margin: 0;">Connectors</h3>
              <div style="display: flex; gap: var(--space-2);">
                <button class="btn btn-sm" data-test="registry-open-create" @click="openCreate">＋ 新建</button>
                <button class="btn btn-sm" data-test="connectors-refresh" @click="refreshAll" :disabled="loading">刷新</button>
              </div>
            </div>
            <div class="card-body">
              <div v-if="loading" style="color: var(--text-secondary); font-size: var(--text-sm);">加载中…</div>
              <div v-else-if="connectors.length === 0" style="color: var(--text-secondary); font-size: var(--text-sm);">
                暂无已注册 connector。点「＋ 新建」粘贴清单创建，或把 connector 目录放进
                &lt;workspace&gt;/connectors/ 由网关扫描注册。
              </div>
              <div v-else style="display: flex; flex-direction: column; gap: var(--space-3);">
                <div
                  v-for="c in connectors"
                  :key="c.id"
                  class="connector-card"
                  :class="{ 'connector-disabled': !c.enabled }"
                  data-test="connector-card"
                >
                  <div class="connector-head">
                    <span class="connector-name">{{ c.name || c.id }}</span>
                    <code class="connector-id">{{ c.id }}</code>
                    <span v-if="c.version" class="conn-meta">v{{ c.version }}</span>
                    <span v-if="c.kind" class="badge badge-info">{{ c.kind }}</span>
                    <span v-if="trustBadge(c.trust)" class="badge" :class="trustBadge(c.trust)!.cls" data-test="connector-trust">{{ trustBadge(c.trust)!.text }}</span>
                    <span class="badge" :class="c.enabled ? 'badge-success' : 'badge-neutral'" data-test="connector-enabled">{{ c.enabled ? '已启用' : '已停用' }}</span>
                    <span
                      v-if="c.mcp_ready === false"
                      class="badge badge-neutral"
                      data-test="connector-mcp-missing"
                      title="绑定目标 MCP server 尚未安装/注册。连接器只是治理壳，不拦截任何工具；MCP 状态的唯一真相源在 MCP 页。"
                    >绑定目标未配置 · 去 MCP 页</span>
                    <span style="flex: 1;"></span>
                    <!-- 管理面操作：损坏条目不提供启停（清单坏了启停无意义），
                         但保留删除（清理损坏目录的主通道）。
                         B4：总开关关闭时子开关置灰（层级闸——先开总开关）。 -->
                    <span
                      v-if="!c.error && !masterEnabled"
                      class="badge badge-neutral"
                      data-test="sub-toggle-greyed"
                      title="连接器总开关已关闭（概览页）——先开启总开关才能启用单个连接器"
                    >总开关已关</span>
                    <button
                      v-if="!c.error"
                      class="btn btn-sm"
                      data-test="connector-toggle"
                      :disabled="busyDir === dirOf(c) || !masterEnabled"
                      :title="masterEnabled
                        ? (c.exclusive_mcp?.length ? '停用将释放独占接管：该 server 的原生工具恢复对 agent 直调（监管叠加整体下线）' : '')
                        : '连接器总开关已关闭（概览页）'"
                      @click="toggleEnabled(c)"
                    >{{ c.enabled ? '停用' : '启用' }}</button>
                    <button
                      class="btn btn-sm"
                      :class="confirmDelete === dirOf(c) ? 'btn-danger' : ''"
                      data-test="connector-delete"
                      :disabled="busyDir === dirOf(c)"
                      @click="deleteConnector(c)"
                    >{{ confirmDelete === dirOf(c) ? '确认删除？' : '删除' }}</button>
                  </div>
                  <div v-if="c.description" class="connector-desc">{{ c.description }}</div>
                  <!-- 损坏条目：红字诚实展示错误原文，不隐藏 -->
                  <div v-if="c.error" class="connector-error" data-test="connector-error">⚠ {{ c.error }}</div>
                  <div v-else-if="c.ops && c.ops.length" class="table-wrap connector-ops">
                    <table>
                      <thead>
                        <tr><th>op</th><th>意图</th><th>风险</th><th>约束</th></tr>
                      </thead>
                      <tbody>
                        <tr v-for="op in c.ops" :key="op.id">
                          <td><code>{{ op.id }}</code></td>
                          <td>{{ intentFirst(op) }}</td>
                          <td>
                            <span class="risk-badge" :class="connectorRiskClass(op.risk)" :data-risk="op.risk">{{ op.risk }}</span>
                          </td>
                          <td>
                            <span v-if="op.confirm_required" class="badge badge-warning">需确认</span>
                            <span v-if="op.finance_budget" class="badge badge-info" style="margin-left: var(--space-1);">资金限额</span>
                            <span v-if="!op.confirm_required && !op.finance_budget" style="color: var(--text-muted);">—</span>
                          </td>
                        </tr>
                      </tbody>
                    </table>
                  </div>
                  <!-- B2 独占接管：per-tool 三态策略（默认镜像 / 豁免 / 封禁）。
                       豁免 = 恢复原生直调（放弃这一个接口的监管）；封禁 = 原生
                       照隐 + 镜像不提供（能力消失）；默认 = 镜像基线（凭条恒
                       记；档位按工具自述 hints 定，见「基线」列）。 -->
                  <div
                    v-if="!c.error && c.exclusive_mcp && c.exclusive_mcp.length"
                    class="connector-exclusive"
                    data-test="exclusive-block"
                  >
                    <div class="conn-exclusive-head">
                      独占接管：<code>{{ c.exclusive_mcp.join('、') }}</code>
                      <span style="color: var(--text-muted); font-size: var(--text-xs);">
                        — 这些原生工具对 agent 隐藏，经连接器镜像调用（凭条恒记；基线按工具自述 hints 定档，见「基线」列）；逐工具可豁免/封禁。
                      </span>
                    </div>
                    <div class="table-wrap connector-ops">
                      <table data-test="mirrors-table">
                        <thead>
                          <tr><th>原生工具</th><th>基线</th><th>策略</th></tr>
                        </thead>
                        <tbody>
                          <tr v-for="m in c.mirrors || []" :key="m.id" data-test="mirror-row">
                            <td>
                              <code>{{ m.id }}</code>
                              <span
                                v-if="m.hidden_by_config"
                                class="risk-badge"
                                style="margin-left: 6px;"
                                title="该工具被 agents.hidden_tools 隐藏（绝对闸）——选择器放行也调不动，需先在配置里解除隐藏"
                              >已隐藏</span>
                            </td>
                            <td>
                              <span
                                v-if="m.risk"
                                class="risk-badge"
                                :class="connectorRiskClass(m.risk)"
                                :data-test="`mirror-risk-${m.id}`"
                              >{{ m.risk }}<template v-if="m.confirm_required"> ⚑</template></span>
                              <span v-else style="color: var(--text-muted); font-size: var(--text-xs);">—</span>
                            </td>
                            <td>
                              <select
                                class="form-input conn-policy-select"
                                :data-test="`policy-select-${m.id}`"
                                :value="m.policy"
                                :disabled="busyPolicy === `${c.id}/${m.id}`"
                                @change="setPolicy(c, m.id, $event)"
                              >
                                <option value="default">默认镜像</option>
                                <option value="exempt">豁免（原生直调）</option>
                                <option value="blocked">封禁</option>
                              </select>
                            </td>
                          </tr>
                        </tbody>
                      </table>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>

          <!-- 回执（行动账本） -->
          <div class="card" data-test="receipts-card">
            <div class="card-header" style="display: flex; justify-content: space-between; align-items: center;">
              <h3 style="margin: 0;">
                回执（行动账本）
                <span style="font-size: var(--text-sm); font-weight: 400; color: var(--text-secondary);">最近 {{ receipts.length }} 条</span>
              </h3>
              <button class="btn btn-sm" data-test="receipts-refresh" @click="refreshAll" :disabled="loading">刷新</button>
            </div>
            <div class="card-body">
              <div v-if="receiptsError" class="connector-error">回执加载失败：{{ receiptsError }}</div>
              <div v-else-if="receipts.length === 0" data-test="receipts-empty" style="color: var(--text-secondary); font-size: var(--text-sm);">
                暂无回执。connector 操作执行（或被拒绝/限额拦截）后，每笔都会在这里留底。
              </div>
              <div v-else class="table-wrap">
                <table data-test="receipts-table">
                  <thead>
                    <tr>
                      <th>回执 ID</th><th>时间</th><th>connector.op</th><th>风险</th><th>结果</th><th>摘要</th><th>主键</th>
                    </tr>
                  </thead>
                  <tbody>
                    <tr v-for="r in receipts" :key="r.receipt_id" data-test="receipt-row">
                      <td><code>{{ r.receipt_id }}</code></td>
                      <td class="conn-ts">{{ formatTs(r.ts) }}</td>
                      <td><code>{{ r.connector }}.{{ r.op }}</code></td>
                      <td><span class="risk-badge" :class="connectorRiskClass(r.risk)">{{ r.risk }}</span></td>
                      <td><span class="badge" :class="connectorOutcomeClass(r.outcome)">{{ connectorOutcomeLabel(r.outcome) }}</span></td>
                      <td class="conn-summary" :title="r.summary">{{ r.summary }}</td>
                      <td class="conn-receipt-value">
                        <span v-if="r.receipt_value"><code>{{ r.receipt_key ? r.receipt_key + ': ' : '' }}{{ r.receipt_value }}</code></span>
                        <span v-if="r.undoable" class="badge badge-info undo-badge" data-test="undo-badge">可撤销</span>
                      </td>
                    </tr>
                  </tbody>
                </table>
              </div>
            </div>
          </div>
        </template>

        <!-- ════════ Tab③ 商城：占位（未来远程源浏览 + 一键安装） ════════ -->
        <div v-else class="card" data-test="shop-card">
          <div class="card-header"><h3 style="margin: 0;">connector 商城（规划中）</h3></div>
          <div class="card-body conn-intro">
            <p style="color: var(--text-secondary);">
              商城将支持从远程源浏览社区 connector 并一键安装（信任徽标 + 签名验签把门）。
              当前版本可先用 CLI 注册远程源：
            </p>
            <p><code>nemesisbot connector add-source &lt;仓库地址&gt;</code></p>
            <p style="color: var(--text-muted); font-size: var(--text-sm);">
              已注册的远程源可用 <code>connector search / install</code> 检索安装——商城页是这套
              能力的可视化形态，后续批次落地。
            </p>
          </div>
        </div>

        <!-- 新建连接器模态（后端权威校验，拒绝原文回显、modal 不关） -->
        <div v-if="showCreate" class="modal-backdrop" data-test="create-modal" @click.self="showCreate = false">
          <div class="modal conn-create-modal">
            <div class="modal-header">
              <h3>新建连接器</h3>
              <button class="close-btn" @click="showCreate = false">×</button>
            </div>
            <div class="modal-body">
              <div class="form-group">
                <label class="form-label">目录名 *</label>
                <input
                  v-model="newDir"
                  class="form-input"
                  data-test="create-dir"
                  placeholder="小写字母/数字/-/_，如 my-connector"
                  @keyup.enter="submitCreate"
                />
              </div>
              <div class="form-group">
                <label class="form-label">connector.toml 全文 *</label>
                <textarea
                  v-model="newToml"
                  class="form-input conn-toml-input"
                  data-test="create-toml"
                  rows="16"
                  spellcheck="false"
                ></textarea>
              </div>
              <div v-if="createError" class="connector-error" data-test="create-error">{{ createError }}</div>
            </div>
            <div class="modal-footer">
              <button class="btn" @click="showCreate = false">取消</button>
              <button class="btn primary" data-test="create-submit" :disabled="creating" @click="submitCreate">
                {{ creating ? '创建中…' : '创建' }}
              </button>
            </div>
          </div>
        </div>
      </template>

    </div>
  </div>
</template>

<style scoped>
code { background: var(--bg-secondary, rgba(0,0,0,0.05)); padding: 1px 4px; border-radius: 3px; font-size: var(--text-xs); }

.conn-count-badge {
  font-size: var(--text-xs);
  color: var(--text-secondary);
  border: 1px solid var(--border);
  border-radius: 999px;
  padding: 2px 10px;
  margin-left: var(--space-3);
}

.conn-error {
  color: var(--error, #ef4444);
  font-size: var(--text-sm);
}

/* 概览/商城介绍段落节奏 */
.conn-intro p { margin: 0 0 var(--space-2); font-size: var(--text-sm); line-height: 1.7; }
.conn-intro p:last-child { margin-bottom: 0; }

/* 新建模态（骨架同 ProjectCreateModal；加宽适配 TOML 编辑） */
.modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}
.conn-create-modal { width: 640px; max-width: 92vw; }
.conn-toml-input {
  font-family: var(--font-mono, monospace);
  font-size: var(--text-xs);
  resize: vertical;
}

/* 损坏条目红字（诚实展示，不隐藏） */
.connector-error {
  margin-top: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border: 1px solid rgba(239, 68, 68, 0.4);
  background: rgba(239, 68, 68, 0.08);
  border-radius: var(--radius-md);
  color: var(--error, #ef4444);
  font-size: var(--text-sm);
  word-break: break-all;
}

.connector-card {
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-3) var(--space-4);
}

.connector-card.connector-disabled { opacity: 0.65; }

.connector-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.connector-name { font-weight: 600; }

.connector-id {
  color: var(--text-secondary);
  font-family: var(--font-mono, monospace);
}

.conn-meta { font-size: var(--text-xs); color: var(--text-secondary); }

.connector-desc {
  margin-top: var(--space-1);
  font-size: var(--text-sm);
  color: var(--text-secondary);
}

.connector-ops { margin-top: var(--space-2); font-size: var(--text-sm); }

/* B2 独占接管块（与 ops 表区分的左描边） */
.connector-exclusive {
  margin-top: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border-left: 3px solid var(--accent, #6366f1);
  background: var(--bg-secondary, rgba(99, 102, 241, 0.05));
  border-radius: var(--radius-md);
}
.conn-exclusive-head { font-size: var(--text-sm); margin-bottom: var(--space-1); }
.conn-policy-select { padding: 2px 6px; font-size: var(--text-xs); width: auto; }

/* risk 徽标四色（low=绿 medium=黄 high=橙 critical=红；未知灰） */
.risk-badge {
  display: inline-flex;
  align-items: center;
  padding: 2px 8px;
  border-radius: var(--radius-full, 999px);
  font-size: var(--text-xs);
  font-weight: 600;
}
.risk-low      { background: rgba(16,185,129,0.15); color: #10b981; }
.risk-medium   { background: rgba(234,179,8,0.15); color: #eab308; }
.risk-high     { background: rgba(249,115,22,0.15); color: #f97316; }
.risk-critical { background: rgba(239,68,68,0.15); color: #ef4444; }
.risk-unknown  { background: var(--surface, rgba(0,0,0,0.05)); color: var(--text-muted); }

.conn-ts {
  font-family: var(--font-mono, monospace);
  font-size: var(--text-xs);
  color: var(--text-secondary);
  white-space: nowrap;
}

.conn-summary {
  max-width: 260px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.conn-receipt-value { white-space: nowrap; }
.undo-badge { margin-left: var(--space-1); }
</style>
