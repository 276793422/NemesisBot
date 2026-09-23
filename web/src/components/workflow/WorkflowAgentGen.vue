<script setup lang="ts">
/**
 * WorkflowAgentGen — 对话生成 TAB（AI workflow generator 的 UI 入口）。
 *
 * 左侧：ChatPanel（普通 chat 模块 + `moduleData.workflow_edit` 透传）。
 *   每条消息都带 `workflow_edit: { workflow_name }` 元数据 → 后端
 *   AgentLoop 在 system 区注入「工作流编辑会话」摘要（当前 YAML 或
 *   capabilities 引导），AI 用 workflow_create 工具产草稿。
 *   会话本体就是普通会话——D-3（2026-09-23 多会话并行清账）起经
 *   `:session-id` prop 钉死给 ChatPanel（effectiveSid），**不再抢写全局
 *   currentId**：主聊天页的选中/视图/busy 态与本面板彻底隔离，「保存工作
 *   流后对话仍绑【新建工作流】」的跨会话串扰在视图层终结。
 * 右侧：WorkflowDraftPanel（草稿列表 + 画布预览/应用/放弃；应用成功回抛
 *   applied 事件 → __new__ 引导会话原地重绑到正式工作流名）。
 *
 * 会话绑定（B，2026-09-23）：真相源上移服务端绑定注册表
 * （workspace/data/session_bindings.json，WSAPI sessions.create 带
 * binding_key 原子 get-or-create / set_binding / remove_binding /
 * list 回带 bindings）。每个目标（新建 / 各工作流名）映射一条专用会话；
 * 「＋新建工作流」目标全局共用一条会话。旧 localStorage 映射
 * （nemesisbot_wf_agentGen_sids）只作一次性迁移源：目标会话仍存活则
 * setBinding 收编进服务端，陈旧则弃用——零本地状态。
 * 草稿列表刷新：TAB 进入 / 目标切换 / tool_event(workflow_create 完成)。
 */
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { useWorkflowStore } from '../../stores/workflow'
import { useSessionStore } from '../../stores/session'
import { useChatApi } from '../../composables/useChatApi'
import { useWfEditSessions } from '../../composables/wfEditSessions'
import { addMessageHandler, removeMessageHandler } from '../../composables/useWebSocket'
import ChatPanel from '../ChatPanel.vue'
import WorkflowDraftPanel from './WorkflowDraftPanel.vue'

/** 「＋新建工作流」目标的 select 哨兵值。 */
const NEW_TARGET = '__new__'
const TARGET_KEY = 'nemesisbot_wf_agentGen_target'
/** 旧 localStorage 映射键——只读迁移源，不再写入（真相源上移服务端）。 */
const LEGACY_SID_MAP_KEY = 'nemesisbot_wf_agentGen_sids'

const store = useWorkflowStore()
const sessionStore = useSessionStore()
const api = useChatApi()
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

// --- B：目标 → 后端会话绑定（服务端注册表真相源） ---

/** 本面板当前绑定的会话 id；null = 尚未就绪（ChatPanel 不挂载，
 *  避免首拉历史打到无 session_id 的默认会话）。 */
const boundSid = ref<string | null>(null)
const bindingError = ref<string | null>(null)

function bindingKeyFor(target: string): string {
  return `wf_agentgen:${target}`
}

function loadLegacySidMap(): Record<string, string> {
  try {
    const raw = localStorage.getItem(LEGACY_SID_MAP_KEY)
    const parsed = raw ? JSON.parse(raw) : null
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, string>) : {}
  } catch {
    return {}
  }
}

/** 迁移后清掉该目标的旧条目（陈旧映射永不复用；localStorage 是降级写）。 */
function migrateOutLegacy(target: string): void {
  try {
    const map = loadLegacySidMap()
    if (!(target in map)) return
    delete map[target]
    localStorage.setItem(LEGACY_SID_MAP_KEY, JSON.stringify(map))
  } catch {
    /* 清理失败无害——迁移幂等 */
  }
}

/** 确保目标有绑定的会话并锚定本面板；返回是否成功。
 *  服务端 get-or-create：key 已绑且会话存活 → 原样复用（零新建，复制
 *  机器终结）；旧 localStorage 条目仍存活 → 先收编进服务端（历史对话
 *  不丢）；未绑/陈旧 → 服务端新建并落绑定。 */
async function ensureSessionFor(target: string): Promise<boolean> {
  const key = bindingKeyFor(target)
  const title = target === NEW_TARGET ? '对话生成：新建工作流' : `对话生成：${target}`
  try {
    const legacy = loadLegacySidMap()[target]
    if (legacy) {
      await sessionStore.fetchList(true).catch(() => {})
      if (sessionStore.sessions.some(s => s.id === legacy)) {
        await api.setBinding(key, legacy).catch(() => {})
      }
      migrateOutLegacy(target)
    }
    const res = await api.createBound(key, title)
    wfSessions.register(res.session_id, target === NEW_TARGET ? null : target)
    boundSid.value = res.session_id
    bindingError.value = null
    // 侧栏可见性：列表强制刷新（session.ts 在飞共享 Promise 去重并发拉取；
    // 未入列期间 receive 帧的 maybeRefreshSessionsFor 兜底）。
    void sessionStore.fetchList(true)
    return true
  } catch (e) {
    bindingError.value = `会话准备失败：${typeof e === 'string' ? e : ((e as Error)?.message ?? '未知错误')}`
    return false
  }
}

// --- C：草稿应用后的原地重绑（draft_apply → 正式工作流名） ---

/** __new__ 引导会话里应用草稿「name」→ 该会话重绑到 name 的专用键
 *  （对话连续性保留：AI 的上下文就在这条会话里），并释放 __new__ 键
 *  （下次「新建工作流」从干净会话开始）；选中目标随之切到 name。 */
async function onDraftApplied(name: string) {
  if (selectedTarget.value !== NEW_TARGET || !boundSid.value) return
  const sid = boundSid.value
  try {
    await api.setBinding(bindingKeyFor(name), sid)
    await api.removeBinding(bindingKeyFor(NEW_TARGET))
    await sessionStore.fetchList(true)
    // watch(selectedTarget) 会跟进 ensureSessionFor(name)（createBound
    // 幂等命中刚重绑的会话，boundSid 不变 → ChatPanel 不重载，仅
    // moduleData 的 workflow_edit 切到 name）+ fetchDrafts。
    selectedTarget.value = name
  } catch {
    // 重绑失败不阻断应用流程——下次进入该目标 get-or-create 兜底。
  }
}

// --- tool_event → 草稿列表自动刷新 ---

function onWsMessage(frame: { type?: string; cmd?: string; data?: any }) {
  if (frame.type !== 'push' || frame.cmd !== 'tool_event') return
  const payload = frame.data
  if (payload?.kind !== 'ToolFinished') return
  if (payload.data?.tool !== 'workflow_create') return
  void store.fetchDrafts(true)
}

onMounted(async () => {
  addMessageHandler(onWsMessage)
  await ensureSessionFor(selectedTarget.value)
  void store.fetchList()
})

onUnmounted(() => {
  removeMessageHandler(onWsMessage)
  // D-3：不再保存/恢复全局选中——本面板从未动过 sessionStore.currentId，
  // 主聊天页的视图在离开/回来时按它自己的重挂载补偿链自愈。
})
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
        <div v-if="bindingError" class="gen-binding-state error">⚠ {{ bindingError }}</div>
        <div v-else-if="!boundSid" class="gen-binding-state">⟳ 正在准备对话会话...</div>
        <ChatPanel
          v-else
          :session-id="boundSid"
          title-override="工作流对话生成"
          :placeholder-override="placeholder"
          :module-data="moduleData"
        />
      </div>
      <aside class="gen-drafts">
        <WorkflowDraftPanel @applied="onDraftApplied" />
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
  /* E（2026-09-23 滚动条丢失根因之一）：flex 子项缺 min-height:0 时不许
     收缩，内部滚动区失去上界。 */
  min-height: 0;
  display: flex;
  flex-direction: column;
}

/* E：宿主契约——.page-chat 是 ChatPanel 根。聊天页（ChatView.vue）与独立
   工作流聊天页（WorkflowChatStandalone.vue）都由宿主补「有界高度 + 纵向
   flex」约束，否则 .chat-messages 的 flex:1 + overflow-y:auto 不产生滚动
   条、内容溢出丢失。dashboard 的 `.main-content > [class^="page-"]` 规则
   够不到嵌在 .workflow-content 里的本实例。 */
.gen-chat > :deep(.page-chat) {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  height: 100%;
}

.gen-binding-state {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: var(--text-sm);
  color: var(--text-muted);
}
.gen-binding-state.error {
  color: var(--danger, #e74c3c);
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
