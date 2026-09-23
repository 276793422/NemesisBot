<script setup lang="ts">
/**
 * WorkflowAgentGen — 对话生成 TAB（AI workflow generator 的 UI 入口）。
 *
 * 左侧：ChatPanel（普通 chat 模块 + `moduleData.workflow_edit` 透传）。
 *   每条消息都带 `workflow_edit: { workflow_name }` 元数据 → 后端
 *   AgentLoop 在 system 区注入「工作流编辑会话」摘要（当前 YAML 或
 *   capabilities 引导），AI 用 workflow_create 工具产草稿。
 *   会话本体就是普通会话——ChatPanel 跟随全局 sessionStore.currentId，
 *   因此这里用「全局切换 + 卸载恢复」方案（工作流页无 KeepAlive，
 *   卸载即恢复，不影响其他视图）。
 * 右侧：WorkflowDraftPanel（草稿列表 + 画布预览/应用/放弃）。
 *
 * 会话绑定（localStorage 持久）：每个目标（新建 / 各工作流名）映射到
 * 一条专用后端会话；「＋新建工作流」目标全局共用一条会话。
 * 草稿列表刷新：TAB 进入 / 目标切换 / tool_event(workflow_create 完成)。
 */
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { useWorkflowStore } from '../../stores/workflow'
import { useSessionStore } from '../../stores/session'
import { useWfEditSessions } from '../../composables/wfEditSessions'
import { addMessageHandler, removeMessageHandler } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import WorkflowDraftPanel from './WorkflowDraftPanel.vue'

/** 「＋新建工作流」目标的 select 哨兵值。 */
const NEW_TARGET = '__new__'
const TARGET_KEY = 'nemesisbot_wf_agentGen_target'
const SID_MAP_KEY = 'nemesisbot_wf_agentGen_sids'

const store = useWorkflowStore()
const sessionStore = useSessionStore()
const wfSessions = useWfEditSessions()

// --- 目标选择（localStorage 持久，跨刷新保留） ---

function loadTarget(): string {
  try {
    const v = localStorage.getItem(TARGET_KEY)
    return v ? v : NEW_TARGET
  } catch {
    return NEW_TARGET
  }
}
const selectedTarget = ref<string>(loadTarget())

watch(selectedTarget, (t) => {
  try {
    localStorage.setItem(TARGET_KEY, t)
  } catch {
    /* 持久化失败不影响会话内状态 */
  }
  void ensureSessionFor(t)
  void store.fetchDrafts(true)
})

/** workflow_edit 透传目标：null = 新建引导会话。 */
const targetName = computed<string | null>(() =>
  selectedTarget.value === NEW_TARGET ? null : selectedTarget.value,
)

const moduleData = computed<Record<string, unknown>>(() => ({
  workflow_edit: { workflow_name: targetName.value },
}))

const placeholder = computed(() =>
  targetName.value
    ? `描述要怎么改「${targetName.value}」（AI 会生成草稿，不会直接覆盖）…`
    : '描述你想要的工作流（触发器、步骤、分支…），AI 生成草稿后可在右侧应用…',
)

// --- 目标 → 后端会话映射（localStorage 持久） ---

type SidMap = Record<string, string>
function loadSidMap(): SidMap {
  try {
    const raw = localStorage.getItem(SID_MAP_KEY)
    const parsed = raw ? JSON.parse(raw) : null
    return parsed && typeof parsed === 'object' ? (parsed as SidMap) : {}
  } catch {
    return {}
  }
}
function saveSid(map: SidMap): void {
  try {
    localStorage.setItem(SID_MAP_KEY, JSON.stringify(map))
  } catch {
    /* 降级为内存态 */
  }
}

/** 确保目标有绑定的会话并切过去；返回是否成功。 */
async function ensureSessionFor(target: string): Promise<boolean> {
  await sessionStore.fetchList()
  const map = loadSidMap()
  let sid: string | undefined = map[target]
  // 会话可能已在别处被删除：登记失效就重建，不报错
  if (sid && !sessionStore.sessions.some(s => s.id === sid)) {
    sid = undefined
  }
  if (!sid) {
    const title = target === NEW_TARGET ? '对话生成：新建工作流' : `对话生成：${target}`
    sid = (await sessionStore.create(title)) ?? undefined
    if (!sid) return false
    map[target] = sid
    saveSid(map)
  }
  wfSessions.register(sid, target === NEW_TARGET ? null : target)
  sessionStore.switchTo(sid)
  return true
}

// --- 会话恢复（离开 TAB 时回到进入前的会话） ---

let prevSessionId: string | null = null

// --- tool_event → 草稿列表自动刷新 ---

function onWsMessage(frame: { type?: string; cmd?: string; data?: any }) {
  if (frame.type !== 'push' || frame.cmd !== 'tool_event') return
  const payload = frame.data
  if (payload?.kind !== 'ToolFinished') return
  if (payload.data?.tool !== 'workflow_create') return
  void store.fetchDrafts(true)
}

onMounted(async () => {
  prevSessionId = sessionStore.currentId
  addMessageHandler(onWsMessage)
  // 先切到目标会话（存在时同步已切，避免闪现其他会话历史），再拉列表
  await ensureSessionFor(selectedTarget.value)
  void store.fetchList()
})

onUnmounted(() => {
  removeMessageHandler(onWsMessage)
  // 回到进入前的会话（若仍存在）；没恢复成功也不炸——侧栏可手动切换
  if (prevSessionId && sessionStore.sessions.some(s => s.id === prevSessionId)) {
    sessionStore.switchTo(prevSessionId)
  }
})

// setup 阶段同步预切换：已绑定的会话直接切过去，ChatPanel 首次
// loadHistory 就加载正确会话（否则会先闪现当前活跃会话的历史）。
{
  const sid = loadSidMap()[selectedTarget.value]
  if (sid) sessionStore.switchTo(sid)
}
</script>

<template>
  <div class="agent-gen">
    <div class="gen-toolbar">
      <label class="tb-label" for="wf-gen-target">编辑目标</label>
      <select id="wf-gen-target" v-model="selectedTarget" class="tb-select">
        <option :value="NEW_TARGET">＋ 新建工作流</option>
        <option v-for="w in store.workflows" :key="w.name" :value="w.name">{{ w.name }}</option>
      </select>
      <span class="tb-hint">AI 生成的定义先保存为草稿，在右侧确认应用后才会生效</span>
    </div>

    <div class="gen-body">
      <div class="gen-chat">
        <ChatPanel
          title-override="工作流对话生成"
          :placeholder-override="placeholder"
          :module-data="moduleData"
        />
      </div>
      <aside class="gen-drafts">
        <WorkflowDraftPanel />
      </aside>
    </div>
  </div>
</template>

<style scoped>
.agent-gen {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}

.gen-toolbar {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}

.tb-label {
  font-size: var(--text-sm);
  color: var(--text-secondary);
}

.tb-select {
  max-width: 260px;
  padding: 4px 8px;
  font-size: var(--text-sm);
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg-secondary, transparent);
  color: var(--text-primary);
}

.tb-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.gen-body {
  flex: 1;
  display: flex;
  min-height: 0;
}

.gen-chat {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}

.gen-drafts {
  width: 320px;
  flex-shrink: 0;
  border-left: 1px solid var(--border);
  padding: var(--space-2) var(--space-3);
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

@media (max-width: 960px) {
  .gen-body {
    flex-direction: column;
  }
  .gen-drafts {
    width: auto;
    max-height: 40%;
    border-left: none;
    border-top: 1px solid var(--border);
  }
}
</style>
