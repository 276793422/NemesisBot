<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

// Hooks 钩子页（2026-08-29 自设置页迁移）：hooks.json CC 方言编辑器。
// 原实现 = SettingsView P4（设置页左侧 Hooks 分类取消，功能整体迁至本页）。

const { request } = useWSAPI()
const toast = useToast()

const hooksContent = ref('')
const hooksExists = ref(false)
const hooksValid = ref(true)
const hooksError = ref<string | null>(null)
const hooksSummary = ref<Record<string, number> | null>(null)
const hooksSaving = ref(false)
const hooksRestarting = ref(false)

async function loadHooks() {
  try {
    const data = await request('hooks', 'get')
    hooksContent.value = data?.content ?? ''
    hooksExists.value = !!data?.exists
    hooksValid.value = data?.valid !== false
    hooksError.value = data?.error || null
    hooksSummary.value = data?.summary || null
  } catch (e: any) {
    toast.error('加载 hooks.json 失败: ' + e)
  }
}

/** 保存：前端先做 JSON 语法自检（语义校验由后端 parse_cc_hooks 把关，
 * 校验失败不落盘）。 */
async function saveHooks() {
  try {
    JSON.parse(hooksContent.value)
  } catch (e: any) {
    toast.error('JSON 语法错误，未保存: ' + e?.message || e)
    return
  }
  hooksSaving.value = true
  try {
    const data = await request('hooks', 'set', { content: hooksContent.value })
    hooksSummary.value = data?.summary || null
    hooksExists.value = true
    hooksValid.value = true
    hooksError.value = null
    toast.success(`已保存（${data?.summary?.total ?? 0} 个脚本）。重启 Agent 后生效`)
  } catch (e: any) {
    // 后端语义校验拒绝 —— 错误串就是 parse 详情，文件未动。
    toast.error('保存被拒（文件未写入）: ' + e)
  }
  hooksSaving.value = false
}

/** hooks.json 只在 Agent 启动时加载（agent_factory），保存后一键重启生效。 */
async function restartAgentForHooks() {
  hooksRestarting.value = true
  try {
    await request('agent', 'stop')
    await new Promise(r => setTimeout(r, 1000))
    await request('agent', 'start')
    toast.success('Agent 已重启，hooks 配置已生效')
  } catch (e: any) {
    toast.error('重启 Agent 失败: ' + (e?.message || e))
  }
  hooksRestarting.value = false
}

onMounted(loadHooks)
</script>

<template>
  <div class="page-hooks">
    <div class="page-header"><h2>Hooks 钩子</h2></div>
    <div class="page-body">
      <div class="card" style="margin-bottom: var(--space-4);">
        <div class="card-header"><h3>Hooks 钩子说明</h3></div>
        <div class="card-body">
          <p style="font-size: var(--text-sm); margin: 0 0 var(--space-3);">
            hooks.json 采用 Claude Code 方言：每个 hook 是一条子进程脚本（stdin 收单行 JSON 事件、
            env <code>CLAUDE_PROJECT_DIR</code>、cwd 为 workspace），五个事件映射到 Agent 内核钩点：
          </p>
          <table class="hooks-table">
            <thead><tr><th>CC 事件</th><th>触发时机</th><th>阻断语义（exit 2）</th></tr></thead>
            <tbody>
              <tr><td>SessionStart</td><td>每会话首条 prompt 到达时</td><td>观察型（不阻断）</td></tr>
              <tr><td>UserPromptSubmit</td><td>用户消息进入历史之前</td><td>拦下 prompt，模型永远看不到</td></tr>
              <tr><td>PreToolUse</td><td>工具执行前（安全 8 层闸之后）</td><td>拦截工具调用，stderr 作理由回灌模型</td></tr>
              <tr><td>PostToolUse</td><td>工具执行后（Forge 记录前）</td><td>stderr 追加到结果反馈模型（不撤销已执行操作）</td></tr>
              <tr><td>Stop</td><td>最终答案被接受后、轮次结束前</td><td>阻止收尾，stderr 作反馈再答一轮（每轮封顶 2 次）</td></tr>
            </tbody>
          </table>
          <p style="font-size: var(--text-sm); margin: var(--space-3) 0 0; color: var(--text-muted);">
            脚本协议：退出码 0 = 放行（PreToolUse/Stop 还认 stdout JSON <code>{"decision":"block","reason":…}</code>）；
            2 = 阻断；其他/超时/启动失败 = 非阻断错误（fail-open）。matcher 用 CC 工具名
            （Bash/Edit/Write/Read/Grep，本系统工具名自动映射别名），省略 = 全命中。每脚本可设
            <code>timeout</code> 秒（默认 60）。LLM 调用级钩子（K1b）无 CC 对应事件，不在此文件配置。
          </p>
        </div>
      </div>
      <div class="card">
        <div class="card-header">
          <h3>hooks.json<span v-if="hooksSummary" style="font-weight: 400; font-size: var(--text-sm); color: var(--text-muted);">　当前 {{ hooksSummary.total }} 个脚本</span></h3>
          <div style="display: flex; gap: var(--space-2);">
            <button class="btn btn-sm" @click="loadHooks" :disabled="hooksSaving">重载</button>
            <button class="btn btn-sm btn-primary" @click="saveHooks" :disabled="hooksSaving">
              {{ hooksSaving ? '保存中…' : '校验并保存' }}
            </button>
            <button class="btn btn-sm" @click="restartAgentForHooks" :disabled="hooksRestarting">
              {{ hooksRestarting ? '重启中…' : '重启 Agent 生效' }}
            </button>
          </div>
        </div>
        <div class="card-body">
          <div v-if="!hooksValid" style="padding: var(--space-3); margin-bottom: var(--space-3); background: var(--danger-bg, #fdecea); border: 1px solid var(--danger, #dc3545); border-radius: var(--radius-md); font-size: var(--text-sm);">
            磁盘上的文件解析失败（Agent 启动时已跳过加载，fail-open）：{{ hooksError }}<br>
            在下方修正后保存即可恢复。
          </div>
          <div v-else-if="!hooksExists" style="padding: var(--space-3); margin-bottom: var(--space-3); background: var(--accent-muted, #e8f0fe); border: 1px solid var(--border-light); border-radius: var(--radius-md); font-size: var(--text-sm);">
            尚未配置 hooks.json —— 下方为空模板（五个事件全空，可直接编辑）。保存后创建文件。
          </div>
          <textarea class="form-textarea" style="min-height: 50vh; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="hooksContent"></textarea>
          <p class="form-hint" style="margin-top: var(--space-2);">
            保存时后端做 CC 方言语义校验，校验失败不落盘。hooks.json 在 Agent 启动时加载——保存后点「重启 Agent 生效」。
          </p>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* CC 5-event mapping table */
.hooks-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-sm);
}
.hooks-table th,
.hooks-table td {
  padding: 6px 10px;
  border: 1px solid var(--border-light);
  text-align: left;
  vertical-align: top;
}
.hooks-table th {
  background: var(--bg-secondary);
  font-weight: 600;
}
.hooks-table td:first-child {
  white-space: nowrap;
  font-family: var(--font-mono);
}
</style>
