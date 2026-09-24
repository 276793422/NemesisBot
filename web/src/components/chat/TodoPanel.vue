<script setup lang="ts">
/**
 * TodoPanel — H2（2026-09-05，devtool-upgrade 阶段 2）。
 *
 * 渲染当前会话的 todo 清单（H1 `todowrite` 工具写入）：
 * - 实时：监听 WS push 帧 `{type:"push", cmd:"tool_event", data:{kind:"TodoUpdated", data}}`
 *   （M1a AgentEvent 通道），按当前会话过滤后刷新（2026-09-20 BUG-A：帧
 *   data 内的 chat_id 是连接级 id、与会话 id 不同域，过滤恒不等——现优先
 *   用 web pump 注入的 `session_id`（会话 id，末段与 currentId 同域），
 *   旧帧无该字段时回退 chat_id 前缀匹配兜底）。
 * - 进入会话 / 断线重连：WSAPI `chat.todo_get {session_id}` 拉一次同路径
 *   json（后端读 sessions/todo_{safe}.json，与 TodoWriteTool 写路径同构）。
 * - 状态列 checkbox 样式：pending ○ / in_progress 高亮旋转 / completed ✓。
 *
 * 挂载面：ChatPanel（默认 chat 模块；workflow_chat 等模块不挂载）。
 * 会话寻址：锚定宿主传入的 `sessionId`（ChatPanel 的 effectiveSid），
 * **绝不直引全局 currentId**——2026-09-24 串扰修复：曾直引全局选中，
 * 工作流「对话生成」嵌入面板（D-3 钉死绑定会话、module 仍是 chat）因此
 * 把主聊天会话的清单拉来渲染在对话顶上。
 * 折叠态记忆在 localStorage（每会话维度不持久化，全局开合偏好）。
 */
import { computed, onUnmounted, ref, watch } from 'vue'
import { addMessageHandler, removeMessageHandler, wsStatus } from '../../composables/useWebSocket'
import { useWSAPI } from '../../composables/useWSAPI'
import { useChatStore, type TodoItem } from '../../stores/chat'

const props = defineProps<{
  /** 宿主面板的 effectiveSid（D-3：嵌入宿主钉死的会话 id；null/undefined =
   *  尚未就绪，不拉取不监听渲染）。本面板所有取数/过滤的唯一会话域。 */
  sessionId?: string | null
}>()

const chatStore = useChatStore()
const { request } = useWSAPI()

const collapsed = ref(localStorage.getItem('nb_todo_panel_collapsed') === '1')
const justRefreshed = ref(false)

const todos = computed<TodoItem[]>(() => chatStore.todos)
const completedCount = computed(() => todos.value.filter(t => t.status === 'completed').length)
const inProgressIdx = computed(() => todos.value.findIndex(t => t.status === 'in_progress'))

// R2（2026-09-21）全完成自动收起 + 2026-09-24 闪现修复（先判断再展示）：
// 全完成清单按更新来源分流——
// - 「实时勾完」（live WS 帧，且此前正显示着未完成清单）：保留 3s 让用户
//   看到全勾瞬间再收起（R2 原义）；
// - 「历史拉取」（挂载/进会话/重连的 todo_get 回包）与「重复全完成帧」
//   （此前无未完成项在显示）：直接收起，一次渲染都不发生——修「打开项目
//   对话框，历史 4/4 面板闪现 3 秒」（旧逻辑先渲染再定时收起，错误）。
// - 有未完成项的清单：一律恢复显示。
const dismissed = ref(false)
let dismissTimer: ReturnType<typeof setTimeout> | null = null

function clearDismissTimer() {
  if (dismissTimer) {
    clearTimeout(dismissTimer)
    dismissTimer = null
  }
}

/** 清单状态更新的唯一入口（fetch 与 WS push 都走这里）。 */
function applyTodos(next: TodoItem[], live: boolean) {
  const nextAllDone = next.length > 0 && next.every(t => t.status === 'completed')
  const prevOpen
    = chatStore.todos.length > 0 && !chatStore.todos.every(t => t.status === 'completed')
  chatStore.setTodos(next)
  clearDismissTimer()
  if (!nextAllDone) {
    dismissed.value = false
  } else if (live && prevOpen) {
    dismissTimer = setTimeout(() => {
      dismissed.value = true
      dismissTimer = null
    }, 3000)
  } else {
    dismissed.value = true
  }
}

function toggleCollapsed() {
  collapsed.value = !collapsed.value
  localStorage.setItem('nb_todo_panel_collapsed', collapsed.value ? '1' : '0')
}

/** 拉一次当前会话的 todo（进会话 / 重连）。 */
function fetchTodos() {
  const sid = props.sessionId
  if (!sid) return
  request('chat', 'todo_get', { session_id: sid })
    .then((data) => {
      // 响应可能晚于会话切换——回包时校验还是当前会话。
      if (data && props.sessionId === sid) {
        applyTodos(data.todos ?? [], false) // 历史拉取：全完成不闪现
      }
    })
    .catch(() => {
      // todo_get 失败静默（清单不存在是常态；面板对空态本就隐藏）。
    })
}

/** WS push 帧 → TodoUpdated 事件；按当前会话过滤。 */
function onWsMessage(frame: any) {
  if (frame?.type !== 'push' || frame?.cmd !== 'tool_event') return
  const ev = frame?.data
  if (ev?.kind !== 'TodoUpdated') return
  const payload = ev.data ?? {}
  // 2026-09-20 BUG-A：优先按 pump 注入的 session_id（会话 id 域）过滤；
  // 旧帧无该字段时回退 chat_id（`web:{连接id}`，连接级）前缀匹配兜底。
  if (typeof payload.session_id === 'string' && payload.session_id.length > 0) {
    if (payload.session_id !== props.sessionId) return
  } else if (payload.chat_id !== `web:${props.sessionId}`) {
    return
  }
  applyTodos(payload.todos ?? [], true) // live WS 帧：全完成保留 3s 全勾瞬间
  flashRefreshed()
}

let flashTimer: ReturnType<typeof setTimeout> | null = null
/** 更新闪烁提示（折叠态也能感知「有更新」）。 */
function flashRefreshed() {
  justRefreshed.value = true
  if (flashTimer) clearTimeout(flashTimer)
  flashTimer = setTimeout(() => { justRefreshed.value = false }, 1200)
}

addMessageHandler(onWsMessage)

// 进入会话 / 断线重连拉一次（嵌入宿主换绑会话时随 prop 切换重拉）。
watch(() => props.sessionId, fetchTodos, { immediate: true })
watch(wsStatus, (val) => {
  if (val === 'connected') fetchTodos()
})

onUnmounted(() => {
  removeMessageHandler(onWsMessage)
  if (flashTimer) clearTimeout(flashTimer)
  if (dismissTimer) clearTimeout(dismissTimer)
})
</script>

<template>
  <div
    v-if="todos.length > 0 && !dismissed"
    class="todo-panel"
    :class="{ 'todo-flash': justRefreshed }"
  >
    <button class="todo-header" type="button" @click="toggleCollapsed">
      <span class="todo-title">Todo</span>
      <span class="todo-count">{{ completedCount }}/{{ todos.length }}</span>
      <span v-if="justRefreshed" class="todo-updated-dot" title="刚刚更新" />
      <span class="todo-caret">{{ collapsed ? '▸' : '▾' }}</span>
    </button>
    <ul v-if="!collapsed" class="todo-list">
      <li
        v-for="(todo, i) in todos"
        :key="i"
        class="todo-item"
        :class="`is-${todo.status}`"
      >
        <span class="todo-check" aria-hidden="true">
          <template v-if="todo.status === 'completed'">✓</template>
          <template v-else-if="todo.status === 'in_progress'">◐</template>
          <template v-else>○</template>
        </span>
        <span class="todo-content">{{ todo.content }}</span>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.todo-panel {
  margin: 0 0 8px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg-elev);
  overflow: hidden;
  transition: border-color 0.3s;
}

.todo-panel.todo-flash {
  border-color: var(--accent);
}

.todo-header {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 6px 10px;
  border: none;
  background: transparent;
  color: var(--text);
  font-size: var(--text-sm, 13px);
  cursor: pointer;
  text-align: left;
}

.todo-title {
  font-weight: 600;
}

.todo-count {
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}

.todo-updated-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--accent);
}

.todo-caret {
  margin-left: auto;
  color: var(--text-muted);
}

.todo-list {
  list-style: none;
  margin: 0;
  padding: 2px 10px 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.todo-item {
  display: flex;
  align-items: baseline;
  gap: 8px;
  font-size: var(--text-sm, 13px);
  line-height: 1.45;
}

.todo-check {
  flex: none;
  width: 16px;
  text-align: center;
  color: var(--text-muted);
}

.todo-item.is-completed .todo-check {
  color: var(--accent, #4a9);
}

.todo-item.is-completed .todo-content {
  color: var(--text-muted);
  text-decoration: line-through;
}

.todo-item.is-in_progress .todo-check {
  color: var(--accent);
}

.todo-item.is-in_progress .todo-content {
  font-weight: 600;
}
</style>
