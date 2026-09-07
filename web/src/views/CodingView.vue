<script setup lang="ts">
/**
 * P2-1 (2026-08-24 UI entry gap): 「代码开发」页。
 *
 * 四张卡：
 *  1. 语义代码工具（LSP）—— agents.lsp_tool 开关 + 五语言服务器实时
 *     PATH 探测（coding.lsp_status，禁止前端硬编码——§九.6）。
 *  2. Claude Code 委派 —— agents.claude_code_tool 开关 + 固定危险档
 *     （--permission-mode：default/accept_edits/plan/bypass_permissions）。
 *  3. Codex 委派 —— agents.codex_tool 开关 + 沙盒档
 *     （--sandbox：read_only/workspace_write/danger_full_access）。
 *  4. 诊断闭环 —— agents.defaults.diagnostics_loop（C4，编辑→诊断回灌
 *     闭环的独立开关；与 lsp_tool.enabled 解耦，闭环落地前先配）。
 *  5. 并发请求模式 —— agents.defaults.concurrent_request_mode（E2）：
 *     reject（会话忙时直接拒绝）/ queue（忙时排队，本轮结束自动继续，
 *     E1 默认）/ steer（queue + `!` 前缀紧急插话注入下一步思考前）。
 *
 * 写入走通用 config.set_field（ConfigStore 落盘）；前三个开关都是
 * AgentLoop 启动时 PATH 探测注册——保存后需重启 Agent（一键
 * agent.stop → agent.start，无需重启进程）。并发模式同样是启动时读取
 * （check_config_reload 只重解析 tier，不重解析并发模式）。
 */

import { ref, watch, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

// --- LSP probe state (from backend, never hard-coded) ----------------------
interface LspLang {
  lang: string
  label: string
  command: string
  available: boolean
  install_command: string
  needs_interactive: boolean
}
const lspLangs = ref<LspLang[]>([])
const lspAvailableCount = ref(0)
const lspToolWouldRegister = ref(false)
// C6: 静默自举开关 + 一键安装进行中标记（防重复点击）。
const lspAutoInstall = ref(false)
const installingLang = ref('')

// --- Config state -----------------------------------------------------------
const lspEnabled = ref(false)
const ccEnabled = ref(false)
const ccPermissionMode = ref('accept_edits')
const codexEnabled = ref(false)
const codexSandbox = ref('read_only')
// C4: 诊断闭环（agents.defaults.diagnostics_loop）
const diagEnabled = ref(false)
const diagMaxErrors = ref(20)
const diagWaitMaxMs = ref(2000)
// E2: 并发请求模式（agents.defaults.concurrent_request_mode + queue_size）
const concurrentMode = ref('queue')
const concurrentQueueSize = ref(8)
// N2: 小模型杂务通道（agents.small_model；唯一消费点 = 手动 /compact 摘要）
const smallModel = ref('')
const modelNameOptions = ref<string[]>([])

const loading = ref(true)
const restartingAgent = ref(false)
let configInitialized = false
let saveTimer: ReturnType<typeof setTimeout> | undefined

async function loadAll() {
  try {
    const [cfg, lsp] = await Promise.all([
      request('coding', 'config'),
      request('coding', 'lsp_status'),
    ])
    lspEnabled.value = cfg?.lsp?.enabled ?? false
    lspAutoInstall.value = cfg?.lsp?.auto_install ?? false
    ccEnabled.value = cfg?.claude_code?.enabled ?? false
    ccPermissionMode.value = cfg?.claude_code?.permission_mode || 'accept_edits'
    codexEnabled.value = cfg?.codex?.enabled ?? false
    codexSandbox.value = cfg?.codex?.sandbox || 'read_only'
    diagEnabled.value = cfg?.diagnostics?.enabled ?? false
    diagMaxErrors.value = cfg?.diagnostics?.max_errors ?? 20
    diagWaitMaxMs.value = cfg?.diagnostics?.wait_max_ms ?? 2000
    concurrentMode.value = cfg?.concurrent?.mode || 'queue'
    concurrentQueueSize.value = cfg?.concurrent?.queue_size ?? 8
    smallModel.value = cfg?.small_model?.model || ''
    modelNameOptions.value = cfg?.small_model?.model_names ?? []
    lspLangs.value = lsp?.languages ?? []
    lspAvailableCount.value = lsp?.available_count ?? 0
    lspToolWouldRegister.value = !!lsp?.tool_would_register
  } catch (e: any) {
    toast.error('加载代码开发配置失败: ' + (e?.message || e))
  }
}

// 写入统一走 config.set_field —— 模式枚举由下拉框约束（后端 spawn 对
// 未知值 fail-safe 回默认档，两层防御）。
async function setField(path: string, value: unknown) {
  await request('config', 'set_field', { path, value })
}

function saveConfigDebounced() {
  if (!configInitialized) return
  if (saveTimer) clearTimeout(saveTimer)
  saveTimer = setTimeout(async () => {
    try {
      await Promise.all([
        setField('agents.lsp_tool.enabled', lspEnabled.value),
        setField('agents.lsp_tool.auto_install', lspAutoInstall.value),
        setField('agents.claude_code_tool.enabled', ccEnabled.value),
        setField('agents.claude_code_tool.permission_mode', ccPermissionMode.value),
        setField('agents.codex_tool.enabled', codexEnabled.value),
        setField('agents.codex_tool.sandbox', codexSandbox.value),
        setField('agents.defaults.diagnostics_loop.enabled', diagEnabled.value),
        setField('agents.defaults.diagnostics_loop.max_errors', diagMaxErrors.value),
        setField('agents.defaults.diagnostics_loop.wait_max_ms', diagWaitMaxMs.value),
        setField('agents.defaults.concurrent_request_mode', concurrentMode.value),
        setField('agents.defaults.queue_size', concurrentQueueSize.value),
        // 清空 = 写 null（serde 反序列化为 None），恢复「未配置、走主模型」。
        setField('agents.small_model', smallModel.value.trim() || null),
      ])
      toast.success('配置已保存，重启 Agent 后生效')
    } catch (e: any) {
      toast.error('保存失败: ' + (e?.message || e))
    }
  }, 500)
}

// 三个开关/档位都是启动时读取——保存后一键重启 Agent。
async function restartAgent() {
  restartingAgent.value = true
  try {
    await request('agent', 'stop')
    await new Promise(r => setTimeout(r, 1000))
    await request('agent', 'start')
    toast.success('Agent 已重启，代码开发工具设置已生效')
  } catch (e: any) {
    toast.error('重启 Agent 失败: ' + (e?.message || e))
  }
  restartingAgent.value = false
}

// --- C6: LSP 服务器一键安装 ------------------------------------------------
// 走后端 coding.lsp_install → agent exec dispatch（安全 8 层 + 审批卡）。
// exec timeout 600s，WSAPI 侧给 660s 等待窗（默认 30s 必超时假红）。
async function installServer(l: LspLang) {
  if (installingLang.value) return
  installingLang.value = l.lang
  try {
    const r = await request('coding', 'lsp_install', { lang: l.lang }, 660000)
    toast.success(`「${l.label}」安装命令已执行，重启 Agent 后生效`)
    if (r?.result) console.log('[lsp_install]', r.result)
    await refreshLspStatus()
  } catch (e: any) {
    toast.error(`安装失败: ${e?.message || e}`)
  } finally {
    installingLang.value = ''
  }
}

async function refreshLspStatus() {
  try {
    const lsp = await request('coding', 'lsp_status')
    lspLangs.value = lsp?.languages ?? []
    lspAvailableCount.value = lsp?.available_count ?? 0
    lspToolWouldRegister.value = !!lsp?.tool_would_register
  } catch (e: any) {
    toast.error('刷新探测状态失败: ' + (e?.message || e))
  }
}

async function copyInstallCommand(l: LspLang) {
  try {
    await navigator.clipboard.writeText(l.install_command)
    toast.success('安装命令已复制')
  } catch {
    toast.error('复制失败，请手动选择命令文本复制')
  }
}

watch([lspEnabled, lspAutoInstall, ccEnabled, ccPermissionMode, codexEnabled, codexSandbox,
       diagEnabled, diagMaxErrors, diagWaitMaxMs,
       concurrentMode, concurrentQueueSize, smallModel], () => {
  saveConfigDebounced()
})

onMounted(async () => {
  await loadAll()
  loading.value = false
  configInitialized = true
})
</script>

<template>
  <div class="page-coding">
    <div class="page-header">
      <h2>代码开发</h2>
      <button class="btn btn-sm" :disabled="restartingAgent" @click="restartAgent">
        {{ restartingAgent ? '重启中…' : '重启 Agent 生效' }}
      </button>
    </div>
    <div class="page-body">
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-if="!loading" style="display: flex; flex-direction: column; gap: var(--space-4);">

        <!-- 卡 1：LSP 语义代码工具 -->
        <div class="card">
          <div class="card-header"><h3>语义代码工具（LSP）</h3></div>
          <div style="padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3);">
            <p class="muted">
              开启后 Agent 获得 <code>lsp</code> 工具：定义跳转 / 引用查找 / 实现查找 / 悬停信息 / 语义重命名（跨文件，改写经安全审批后落盘） /
              快速修复列表，由真实语言服务器驱动。工具在 Agent 启动时按 PATH 探测注册——至少一个服务器可用才会注册。
            </p>
            <div class="settings-grid">
              <div class="settings-label">启用 LSP 工具</div>
              <label class="toggle-switch">
                <input type="checkbox" v-model="lspEnabled">
                <span class="toggle-slider"></span>
              </label>
              <div class="settings-label">自动安装缺失服务器<span class="muted" style="font-size: var(--text-xs); display: block;">网关启动时后台安装（默认关）</span></div>
              <label class="toggle-switch">
                <input type="checkbox" v-model="lspAutoInstall">
                <span class="toggle-slider"></span>
              </label>
            </div>
            <div>
              <div class="settings-label" style="margin-bottom: var(--space-2);">
                语言服务器探测（本机 PATH 实测）
                <span v-if="lspToolWouldRegister" class="badge badge-success" style="margin-left: 6px;">{{ lspAvailableCount }}/{{ lspLangs.length }} 可用</span>
                <span v-else class="badge badge-error" style="margin-left: 6px;">0/{{ lspLangs.length }} 可用</span>
              </div>
              <div style="display: flex; flex-direction: column; gap: var(--space-1);">
                <div v-for="l in lspLangs" :key="l.lang"
                  style="display: flex; align-items: center; gap: var(--space-3); font-size: var(--text-sm); flex-wrap: wrap;">
                  <span class="badge" :class="l.available ? 'badge-success' : 'badge-neutral'">
                    {{ l.available ? '已安装' : '未安装' }}
                  </span>
                  <span style="min-width: 150px;">{{ l.label }}</span>
                  <code class="muted">{{ l.command }}</code>
                  <template v-if="!l.available && l.install_command">
                    <code class="muted" style="font-size: var(--text-xs);">{{ l.install_command }}</code>
                    <button v-if="!l.needs_interactive" class="btn btn-sm btn-secondary"
                      :disabled="!!installingLang" @click="installServer(l)"
                      style="padding: 2px 10px; font-size: var(--text-xs);">
                      {{ installingLang === l.lang ? '安装中…' : '一键安装' }}
                    </button>
                    <button class="btn btn-sm btn-secondary" @click="copyInstallCommand(l)"
                      style="padding: 2px 10px; font-size: var(--text-xs);">复制命令</button>
                  </template>
                </div>
              </div>
              <p class="form-hint">未安装的语言不会出现在工具能力里。一键安装经 Agent 的 exec 通路（安全审批 + 审计）；
              需要交互终端的命令（如 sudo apt）只能复制手动运行。安装完成后重启 Agent 重新探测。</p>
            </div>
          </div>
        </div>

        <!-- 卡 2：Claude Code 委派 -->
        <div class="card">
          <div class="card-header"><h3>Claude Code 委派</h3></div>
          <div style="padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3);">
            <p class="muted">
              开启后 Agent 可把子任务委派给本机的 <code>claude</code> CLI（需在 PATH）。
              危险档固定在配置层——模型不可自选（spawn 时传 <code>--permission-mode</code>，未知值 fail-safe 回默认档）。
            </p>
            <div class="settings-grid">
              <div class="settings-label">启用 Claude Code 委派</div>
              <label class="toggle-switch">
                <input type="checkbox" v-model="ccEnabled">
                <span class="toggle-slider"></span>
              </label>
              <div class="settings-label">危险档（permission-mode）</div>
              <select class="form-select" v-model="ccPermissionMode" :disabled="!ccEnabled">
                <option value="default">default（每次都要确认）</option>
                <option value="accept_edits">accept_edits（自动接受编辑，默认）</option>
                <option value="plan">plan（只读规划，不改文件）</option>
                <option value="bypass_permissions">bypass_permissions（跳过全部确认，最危险）</option>
              </select>
            </div>
          </div>
        </div>

        <!-- 卡 3：Codex 委派 -->
        <div class="card">
          <div class="card-header"><h3>Codex 委派</h3></div>
          <div style="padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3);">
            <p class="muted">
              开启后 Agent 可把子任务委派给本机的 <code>codex</code> CLI（需在 PATH）。
              沙盒档固定在配置层——模型不可自选（spawn 时传 <code>--sandbox</code>，未知值 fail-safe 回只读档）。
            </p>
            <div class="settings-grid">
              <div class="settings-label">启用 Codex 委派</div>
              <label class="toggle-switch">
                <input type="checkbox" v-model="codexEnabled">
                <span class="toggle-slider"></span>
              </label>
              <div class="settings-label">沙盒档（sandbox）</div>
              <select class="form-select" v-model="codexSandbox" :disabled="!codexEnabled">
                <option value="read_only">read_only（只读，默认）</option>
                <option value="workspace_write">workspace_write（可写工作区）</option>
                <option value="danger_full_access">danger_full_access（完全访问，最危险）</option>
              </select>
            </div>
          </div>
        </div>

        <!-- 卡 4：诊断闭环（C4） -->
        <div class="card">
          <div class="card-header"><h3>诊断闭环</h3></div>
          <div style="padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3);">
            <p class="muted">
              开启后，Agent 每次编辑代码文件会向语言服务器查询诊断（错误/警告），
              超出阈值或等待超时的诊断自动回灌为修复指令，形成「编辑 → 诊断 → 修复」闭环。
              <b>与上方「启用 LSP 工具」相互独立</b>——诊断闭环可以单独开启（推荐组合：
              诊断闭环开 + LSP 工具关，Agent 无需手动调用 lsp 工具也能吃到诊断反馈）。
            </p>
            <div class="settings-grid">
              <div class="settings-label">启用诊断闭环</div>
              <label class="toggle-switch">
                <input type="checkbox" v-model="diagEnabled">
                <span class="toggle-slider"></span>
              </label>
              <div class="settings-label">单次编辑回灌上限（条）</div>
              <input type="number" class="form-input" v-model.number="diagMaxErrors"
                :disabled="!diagEnabled" min="1" max="200" style="max-width: 160px;">
              <div class="settings-label">诊断等待上限（毫秒）</div>
              <input type="number" class="form-input" v-model.number="diagWaitMaxMs"
                :disabled="!diagEnabled" min="0" step="500" style="max-width: 160px;">
            </div>
            <p class="form-hint">闭环按需临时拉起语言服务器，不依赖上方 LSP 工具开关；保存后重启 Agent 生效。</p>
          </div>
        </div>

        <!-- 卡 5：并发请求模式（E2） -->
        <div class="card">
          <div class="card-header"><h3>并发请求模式</h3></div>
          <div style="padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3);">
            <p class="muted">
              同一会话上一条消息还在处理时，新消息怎么办：
              <code>reject</code> 直接拒绝（旧版行为）；
              <code>queue</code> 自动排队，本轮结束后继续处理（默认）；
              <code>steer</code> 在 queue 之上支持 <code>!</code> 前缀紧急插话——长任务跑着时发
              「<code>! 先停一下，改用 X 方案</code>」，消息会在 AI 下一步思考前注入。
            </p>
            <div class="settings-grid">
              <div class="settings-label">模式</div>
              <select class="form-select" v-model="concurrentMode" data-test="concurrent-mode">
                <option value="reject">reject（忙时拒绝，旧版行为）</option>
                <option value="queue">queue（忙时排队，默认）</option>
                <option value="steer">steer（排队 + ! 紧急插话）</option>
              </select>
              <div class="settings-label">排队容量（每会话）</div>
              <input type="number" class="form-input" v-model.number="concurrentQueueSize"
                data-test="concurrent-queue-size"
                :disabled="concurrentMode === 'reject'" min="1" max="64" style="max-width: 160px;">
            </div>
            <p class="form-hint">模式在 Agent 启动时读取——保存后重启 Agent 生效；插话回执在聊天里以 ⏳（已排队）/ ⚡（插话）标记。</p>
          </div>
        </div>

        <!-- 卡 6：小模型杂务通道（N2） -->
        <div class="card">
          <div class="card-header"><h3>小模型杂务通道</h3></div>
          <div style="padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3);">
            <p class="muted">
              指定一个便宜的小模型专门干杂活，当前唯一消费点是手动
              <code>/compact</code> 压缩会话时的摘要生成——大模型上下文又长又贵，摘要这种机械活交给小模型省 token。
              自动压缩（上下文爆窗触发的应急压缩）始终用主模型，不受此项影响。
            </p>
            <div class="settings-grid">
              <div class="settings-label">小模型</div>
              <div>
                <input class="form-input" v-model="smallModel" data-test="small-model"
                  list="small-model-names" placeholder="留空 = 用主模型做摘要"
                  style="max-width: 360px;">
                <datalist id="small-model-names">
                  <option v-for="n in modelNameOptions" :key="n" :value="n"></option>
                </datalist>
              </div>
            </div>
            <p class="form-hint">
              填模型别名（上方下拉候选来自已配置模型列表）；启动时若别名解析失败会回退主模型并在网关日志记 warn。
              Agent 启动时读取——保存后重启 Agent 生效。
            </p>
          </div>
        </div>

      </div>
    </div>
  </div>
</template>

<style scoped>
.settings-grid {
  display: grid;
  grid-template-columns: 220px 1fr;
  gap: var(--space-3) var(--space-4);
  align-items: center;
}
.settings-label {
  font-size: var(--text-sm);
  color: var(--text-secondary, #888);
}
.toggle-switch {
  position: relative;
  display: inline-block;
  width: 40px;
  height: 22px;
}
.toggle-switch input {
  opacity: 0;
  width: 0;
  height: 0;
}
.toggle-slider {
  position: absolute;
  cursor: pointer;
  inset: 0;
  background: var(--bg-inset, #ccc);
  border-radius: 22px;
  transition: 0.2s;
}
.toggle-slider::after {
  content: '';
  position: absolute;
  height: 16px;
  width: 16px;
  left: 3px;
  bottom: 3px;
  background: white;
  border-radius: 50%;
  transition: 0.2s;
}
.toggle-switch input:checked + .toggle-slider {
  background: var(--accent, #4a90e2);
}
.toggle-switch input:checked + .toggle-slider::after {
  transform: translateX(18px);
}
.toggle-switch input:disabled + .toggle-slider {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
