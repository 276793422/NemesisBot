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
 * 挂载面：ChatPanel（默认 chat 模块；workflow_chat 等模块会话路由不同，
 * todo 不适用）。折叠态记忆在 localStorage（每会话维度不持久化，全局开合偏好）。
 */
import { computed, onUnmounted, ref, watch } from 'vue'
import { addMessageHandler, removeMessageHandler, wsStatus } from '../../composables/useWebSocket'
import { useWSAPI } from '../../composables/useWSAPI'
import { useChatStore, type TodoItem } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

const props = defineProps<{
  /** 是否为默认 chat 模块（非默认模块不拉取不监听）。 */
  isDefaultChat: boolean
}>()

const chatStore = useChatStore()
const sessionStore = useSessionStore()
const { request } = useWSAPI()

const collapsed = ref(localStorage.getItem('nb_todo_panel_collapsed') === '1')
const justRefreshed = ref(false)

const todos = computed<TodoItem[]>(() => chatStore.todos)
const completedCount = computed(() => todos.value.filter(t => t.status === 'completed').length)
const inProgressIdx = computed(() => todos.value.findIndex(t => t.status === 'in_progress'))

// R2（2026-09-21）：全部完成后 3s 自动收起——清单的使命是跟踪进行中的
// 多步流程，全勾后长期驻留只占屏。留 3s 让用户看到全勾瞬间；新清单
// 到达（含未完成项）立即恢复。挂载/进会话时 fetchTodos 拉到历史全完成
// 清单同样走 3s 收起（immediate 覆盖「挂载即全完成」形态）。
const allDone = computed(() => todos.value.length > 0 && todos.value.every(t => t.status === 'completed'))
const dismissed = ref(false)
let dismissTimer: ReturnType<typeof setTimeout> | null = null
watch(allDone, (v) => {
  if (dismissTimer) {
    clearTimeout(dismissTimer)
    dismissTimer = null
  }
  if (v) {
    dismissTimer = setTimeout(() => {
      dismissed.value = true
      dismissTimer = null
    }, 3000)
  } else {
    dismissed.value = false
  }
}, { immediate: true })

function toggleCollapsed() {
  collapsed.value = !collapsed.value
  localStorage.setItem('nb_todo_panel_collapsed', collapsed.value ? '1' : '0')
}

/** 拉一次当前会话的 todo（进会话 / 重连）。 */
function fetchTodos() {
  if (!props.isDefaultChat) return
  const sessionId = sessionStore.currentId
  if (!sessionId) return
  request('chat', 'todo_get', { session_id: sessionId })
    .then((data) => {
      // 响应可能晚于会话切换——回包时校验还是当前会话。
      if (data && sessionStore.currentId === sessionId) {
        chatStore.setTodos(data.todos ?? [])
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
    if (payload.session_id !== sessionStore.currentId) return
  } else if (payload.chat_id !== `web:${sessionStore.currentId}`) {
    return
  }
  chatStore.setTodos(payload.todos ?? [])
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

// 进入会话 / 断线重连拉一次。
watch(() => sessionStore.currentId, fetchTodos, { immediate: true })
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
