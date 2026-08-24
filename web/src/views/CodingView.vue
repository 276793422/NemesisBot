<script setup lang="ts">
/**
 * P2-1 (2026-08-24 UI entry gap): 「代码开发」页。
 *
 * 三张卡：
 *  1. 语义代码工具（LSP）—— agents.lsp_tool 开关 + 五语言服务器实时
 *     PATH 探测（coding.lsp_status，禁止前端硬编码——§九.6）。
 *  2. Claude Code 委派 —— agents.claude_code_tool 开关 + 固定危险档
 *     （--permission-mode：default/accept_edits/plan/bypass_permissions）。
 *  3. Codex 委派 —— agents.codex_tool 开关 + 沙盒档
 *     （--sandbox：read_only/workspace_write/danger_full_access）。
 *
 * 写入走通用 config.set_field（ConfigStore 落盘）；三个开关都是
 * AgentLoop 启动时 PATH 探测注册——保存后需重启 Agent（一键
 * agent.stop → agent.start，无需重启进程）。
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
}
const lspLangs = ref<LspLang[]>([])
const lspAvailableCount = ref(0)
const lspToolWouldRegister = ref(false)

// --- Config state -----------------------------------------------------------
const lspEnabled = ref(false)
const ccEnabled = ref(false)
const ccPermissionMode = ref('accept_edits')
const codexEnabled = ref(false)
const codexSandbox = ref('read_only')

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
    ccEnabled.value = cfg?.claude_code?.enabled ?? false
    ccPermissionMode.value = cfg?.claude_code?.permission_mode || 'accept_edits'
    codexEnabled.value = cfg?.codex?.enabled ?? false
    codexSandbox.value = cfg?.codex?.sandbox || 'read_only'
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
        setField('agents.claude_code_tool.enabled', ccEnabled.value),
        setField('agents.claude_code_tool.permission_mode', ccPermissionMode.value),
        setField('agents.codex_tool.enabled', codexEnabled.value),
        setField('agents.codex_tool.sandbox', codexSandbox.value),
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

watch([lspEnabled, ccEnabled, ccPermissionMode, codexEnabled, codexSandbox], () => {
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
              开启后 Agent 获得 <code>lsp</code> 工具：定义跳转 / 引用查找 / 实现查找 / 悬停信息，
              由真实语言服务器驱动（只读查询，不改动代码）。工具在 Agent 启动时按 PATH 探测注册——
              至少一个服务器可用才会注册。
            </p>
            <div class="settings-grid">
              <div class="settings-label">启用 LSP 工具</div>
              <label class="toggle-switch">
                <input type="checkbox" v-model="lspEnabled">
                <span class="toggle-slider"></span>
              </label>
            </div>
            <div>
              <div class="settings-label" style="margin-bottom: var(--space-2);">
                语言服务器探测（本机 PATH 实测）
                <span v-if="lspToolWouldRegister" class="badge badge-success" style="margin-left: 6px;">{{ lspAvailableCount }}/5 可用</span>
                <span v-else class="badge badge-error" style="margin-left: 6px;">0/5 可用</span>
              </div>
              <div style="display: flex; flex-direction: column; gap: var(--space-1);">
                <div v-for="l in lspLangs" :key="l.lang"
                  style="display: flex; align-items: center; gap: var(--space-3); font-size: var(--text-sm);">
                  <span class="badge" :class="l.available ? 'badge-success' : 'badge-neutral'">
                    {{ l.available ? '已安装' : '未安装' }}
                  </span>
                  <span style="min-width: 150px;">{{ l.label }}</span>
                  <code class="muted">{{ l.command }}</code>
                </div>
              </div>
              <p class="form-hint">未安装的语言不会出现在工具能力里；安装对应服务器（如 <code>rustup component add rust-analyzer</code>）后重启 Agent 重新探测。</p>
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
