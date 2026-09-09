<script setup lang="ts">
import { ref, nextTick, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { useBoardChanged } from '../../composables/useBoardChanged'
import { fmtTime } from './boardMeta'

// 讨论（M3 批次 E）：节点间多 agent 组织的沟通频道。左栏频道列表 +
// 右栏消息流。数据面：
// - board.channel.list — 频道列表；
// - board.channel.messages {channel_id, after_id, limit} — after_id 游标
//   增量拉取（幂等：id > after_id 升序，重拉不重不漏）；
// - board.channel.post — dashboard 人工发言（sender=admin/<session>，走
//   master 讨论总线管线：幂等/额度/裁决器唤醒与 worker 上行同源）。
// 实时性：复用 W2.5 board-changed SSE 推送（任何写入方落库后 2s 内广播），
// 收到后按 lastId 游标拉增量；自己发言成功后立即拉一次（不等推送）。

const { request } = useWSAPI()
const toast = useToast()

interface Actor {
  kind: string
  id: string
}

interface Channel {
  id: number
  name: string
  description: string
  created_at: number
}

interface Message {
  id: number
  channel_id: number
  sender: Actor
  content: string
  parent_id: number | null
  mtype: string
  created_at: number
}

// sender 徽标三色：admin=蓝（人工）/ agent=绿（节点 agent）/ system=灰（机械播报）。
const SENDER_BADGE: Record<string, string> = {
  admin: 'badge-info',
  agent: 'badge-success',
  system: 'badge-neutral',
}

// mtype 词表：text（普通发言）/ system（派发播报等）。delivery 等
// issue 线程专属类型不会出现在频道，但徽标仍按类型兜底显示原文。
function mtypeLabel(m: Message): string {
  return m.mtype !== 'text' ? m.mtype : ''
}

const loading = ref(true)
const channels = ref<Channel[]>([])
const activeChannelId = ref<number | null>(null)
const messages = ref<Message[]>([])
// after_id 游标：已拉到的最大消息 id；board-changed 到达时只拉增量。
let lastId = 0
const draft = ref('')
const sending = ref(false)
const messagesEl = ref<HTMLElement | null>(null)

function activeChannel(): Channel | undefined {
  return channels.value.find((c) => c.id === activeChannelId.value)
}

function senderLabel(m: Message): string {
  return `${m.sender.kind}/${m.sender.id}`
}

async function scrollToBottom() {
  await nextTick()
  const el = messagesEl.value
  if (el) el.scrollTop = el.scrollHeight
}

async function loadChannels() {
  const r = await request('board', 'channel.list', {})
  channels.value = r?.channels || []
  if (channels.value.length && activeChannelId.value === null) {
    activeChannelId.value = channels.value[0].id
  }
}

async function pullIncrement(): Promise<Message[]> {
  if (activeChannelId.value === null) return []
  const r = await request('board', 'channel.messages', {
    channel_id: activeChannelId.value,
    after_id: lastId,
    limit: 500,
  })
  const fresh: Message[] = r?.messages || []
  if (fresh.length) {
    // 游标幂等（id > after_id）保证不重；防御式按 id 去重兜底。
    const seen = new Set(messages.value.map((m) => m.id))
    for (const m of fresh) {
      if (!seen.has(m.id)) messages.value.push(m)
    }
    lastId = Math.max(lastId, ...fresh.map((m) => m.id))
  }
  return fresh
}

async function selectChannel(ch: Channel) {
  if (ch.id === activeChannelId.value) return
  activeChannelId.value = ch.id
  messages.value = []
  lastId = 0
  loading.value = true
  try {
    await pullIncrement()
  } catch (e: any) {
    toast.error('加载消息失败: ' + e)
  } finally {
    loading.value = false
  }
  scrollToBottom()
}

async function send() {
  const content = draft.value.trim()
  if (!content || activeChannelId.value === null || sending.value) return
  sending.value = true
  try {
    await request('board', 'channel.post', { channel_id: activeChannelId.value, content })
    draft.value = ''
    // 不等 SSE 推送，立即拉增量让发言可见（游标幂等，重复拉取无害）。
    await pullIncrement()
    scrollToBottom()
  } catch (e: any) {
    // 后端拒绝文案已人读（[quota_exhausted] … / [rate_limited] …），原样透出。
    toast.error(String(e))
  } finally {
    sending.value = false
  }
}

onMounted(async () => {
  try {
    await loadChannels()
    await pullIncrement()
  } catch (e: any) {
    toast.error('加载讨论频道失败: ' + e)
  } finally {
    loading.value = false
  }
  scrollToBottom()
})

// board-changed：任何写入方（worker 上行/裁决器/system 播报/自己 post）落库
// 后静默拉增量。游标保证只追加新行，不打断正在阅读/输入的内容。
useBoardChanged(async () => {
  if (activeChannelId.value === null) return
  try {
    const fresh = await pullIncrement()
    if (fresh.length) scrollToBottom()
  } catch (e) {
    console.warn('[DiscussionPanel] silent refresh failed:', e)
  }
})
</script>

<template>
  <div class="discussion-layout">
    <!-- 频道列表 -->
    <div class="channel-rail">
      <div v-if="loading" style="text-align: center; padding: var(--space-6);">
        <div class="spinner" style="margin: 0 auto;"></div>
      </div>
      <template v-else>
        <div v-if="channels.length === 0" class="muted" style="padding: var(--space-3);">
          暂无频道（master 首启自动建 #dev/#general/#ops）
        </div>
        <button
          v-for="c in channels"
          :key="c.id"
          class="channel-item"
          :class="{ active: c.id === activeChannelId }"
          :title="c.description"
          @click="selectChannel(c)"
        >{{ c.name }}</button>
      </template>
    </div>

    <!-- 消息流 -->
    <div class="thread-pane">
      <template v-if="activeChannelId !== null">
        <div ref="messagesEl" class="message-flow">
          <div v-if="messages.length === 0 && !loading" class="empty-state">
            <h3>还没有消息</h3>
            <p>在下方发言；集群节点的 agent 会收到唤醒并参与讨论</p>
          </div>
          <div
            v-for="m in messages"
            :key="m.id"
            class="message-item"
            :class="{ reply: m.parent_id !== null, system: m.mtype === 'system' }"
          >
            <div class="message-head">
              <span v-if="m.parent_id !== null" class="muted">↳</span>
              <span class="badge" :class="SENDER_BADGE[m.sender.kind] || 'badge-neutral'">{{ m.sender.kind }}</span>
              <strong>{{ senderLabel(m) }}</strong>
              <span v-if="mtypeLabel(m)" class="badge badge-neutral">{{ mtypeLabel(m) }}</span>
              <span class="muted message-time">{{ fmtTime(m.created_at) }}</span>
            </div>
            <div class="message-body">{{ m.content }}</div>
          </div>
        </div>
        <div class="composer">
          <textarea
            v-model="draft"
            class="form-textarea composer-input"
            rows="2"
            :placeholder="`发到 ${activeChannel()?.name || ''}…（Enter 发送，Shift+Enter 换行）`"
            @keydown.enter.exact.prevent="send"
          ></textarea>
          <button
            class="btn btn-primary composer-send"
            :disabled="sending || !draft.trim()"
            @click="send"
          >{{ sending ? '发送中…' : '发送' }}</button>
        </div>
      </template>
      <div v-else-if="!loading" class="empty-state">
        <h3>选择一个频道</h3>
        <p>左侧选择频道查看讨论</p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.muted {
  color: var(--text-muted);
  font-size: var(--text-sm);
}
.discussion-layout {
  display: flex;
  gap: var(--space-3);
  min-height: 480px;
  height: calc(100vh - 260px);
}
.channel-rail {
  width: 180px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  overflow-y: auto;
}
.channel-item {
  text-align: left;
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  padding: var(--space-2) var(--space-3);
  cursor: pointer;
  font-size: var(--text-sm);
  color: var(--text-primary);
}
.channel-item:hover {
  background: var(--bg-secondary);
}
.channel-item.active {
  background: var(--bg-secondary);
  border-color: var(--accent);
  font-weight: 600;
}
.thread-pane {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
}
.message-flow {
  flex: 1;
  overflow-y: auto;
  padding: var(--space-3);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.message-item {
  padding: var(--space-2) var(--space-3);
  border-left: 2px solid var(--border);
}
.message-item.reply {
  margin-left: var(--space-6);
  border-left-color: var(--accent);
}
.message-item.system {
  opacity: 0.75;
  background: var(--bg-primary);
  border-radius: var(--radius-sm);
}
.message-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-1);
  font-size: var(--text-sm);
}
.message-time {
  margin-left: auto;
}
.message-body {
  white-space: pre-wrap;
  word-break: break-word;
  font-size: var(--text-sm);
}
.composer {
  display: flex;
  gap: var(--space-2);
  padding: var(--space-3);
  border-top: 1px solid var(--border);
  align-items: flex-end;
}
.composer-input {
  flex: 1;
  min-height: 44px;
  resize: vertical;
}
.composer-send {
  flex-shrink: 0;
}
@media (max-width: 720px) {
  .discussion-layout {
    height: auto;
  }
  .channel-rail {
    width: 120px;
  }
}
</style>
