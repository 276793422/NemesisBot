<script setup lang="ts">
import { ref, computed, nextTick, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { useBoardChanged } from '../../composables/useBoardChanged'
import { useBoardActors } from '../../composables/useBoardActors'
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
// goal P1 增强：F1 成员面板（在线/离线）+ F2 @ 补全（@名/@role:/@all）+
// F3 唤醒结果回显 + F4/F6 发言提示 + H1 可读设备名（displayActor）。

const { request } = useWSAPI()
const toast = useToast()
const { nodes: actorNodes, ensureNodes, displayActor, actorName } = useBoardActors()

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
  // H1：可读设备名（agent/Alex）；未知 id 回退短 id。
  return displayActor(m.sender.kind, m.sender.id)
}

// ---------------------------------------------------------------------------
// F1 成员面板 + F2 @ 补全 + F4/F6 发言提示（goal P1）
// ---------------------------------------------------------------------------

// F1：集群成员名单（在线优先排序；离线灰显不隐藏——用户该知道有谁存在）。
const roster = computed(() => {
  const list = [...actorNodes.value.values()]
  list.sort((a, b) => Number(b.online) - Number(a.online) || a.name.localeCompare(b.name))
  return list
})

// F2：@ 提及补全状态（start=@ 在 draft 中的位置；query=@ 后的已输入片段）。
const draftEl = ref<HTMLTextAreaElement | null>(null)
const mention = ref<{ start: number; query: string } | null>(null)

const MENTION_FIXED = [
  { token: '@all', desc: '唤醒全部在线节点' },
  { token: '@role:worker', desc: '唤醒全部在线 worker' },
]

const mentionItems = computed(() => {
  if (!mention.value) return []
  const q = mention.value.query.toLowerCase()
  const nodeItems = roster.value
    .filter((n) => n.online)
    .map((n) => ({ token: `@${n.name}`, desc: `${n.role}/${n.category}` }))
  const all = [...nodeItems, ...MENTION_FIXED]
  if (!q) return all
  return all.filter((x) => x.token.toLowerCase().includes(q))
})

function onDraftInput() {
  const el = draftEl.value
  if (!el) return
  const caret = el.selectionStart ?? 0
  const before = draft.value.slice(0, caret)
  const at = before.lastIndexOf('@')
  if (at >= 0) {
    const token = before.slice(at + 1)
    if (!/\s/.test(token)) {
      mention.value = { start: at, query: token }
      return
    }
  }
  mention.value = null
}

function pickMention(token: string) {
  const m = mention.value
  const el = draftEl.value
  if (!m) return
  const caret = el?.selectionStart ?? m.start + m.query.length
  draft.value = `${draft.value.slice(0, m.start)}${token} ${draft.value.slice(caret)}`
  mention.value = null
  nextTick(() => el?.focus())
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

  // F6（goal P1）：@ 了不认识的设备 → 诚实提示（仍可发送——后端裁决器
  // 会如实记 not_found），附可用候选。
  const tokens = (content.match(/(^|[\s（(])@([^\s，。、]+)/g) || [])
    .map((s) => s.trim().slice(1))
    .filter(Boolean)
  if (tokens.length) {
    const known = new Set([
      'all',
      ...roster.value.map((n) => n.name.toLowerCase()),
      ...roster.value.map((n) => n.id.toLowerCase()),
      ...roster.value.map((n) => `role:${n.role.toLowerCase()}`),
      ...roster.value.map((n) => `role:${n.category.toLowerCase()}`),
    ])
    const unknown = tokens.filter((t) => !known.has(t.toLowerCase()) && !/^role:[a-z]/i.test(t))
    if (unknown.length) {
      toast.warn(`未找到设备: ${unknown.map((u) => `@${u}`).join(' ')}（可用：@all、@role:worker 或成员面板中的名字）`)
      return
    }
  }

  sending.value = true
  try {
    const r = await request('board', 'channel.post', { channel_id: activeChannelId.value, content })
    draft.value = ''
    // F3：后端首响带 wake 摘要（谁被唤醒/仅主持人）——即时反馈。
    const wake = r?.wake
    if (wake) {
      const woke: string[] = wake.woke || []
      if (woke.length) {
        toast.success(`已唤醒: ${woke.map((w: string) => actorName(w) || w).join('、')}`)
      } else if (wake.to_moderator) {
        toast.info('将由主持人回应；输入 @ 可点名设备')
      } else {
        toast.info('无人被唤醒（目标离线或未指派）')
      }
    }
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
  // F1/F2：节点注册表（成员面板/@ 补全）——失败不阻塞消息加载。
  ensureNodes().catch(() => {})
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
        <!-- F1（goal P1）：成员面板——在线徽章/离线灰显，用户由此知道能 @ 谁 -->
        <div class="member-section">
          <div class="muted member-head">成员（{{ roster.filter((n) => n.online).length }} 在线 / {{ roster.length }}）</div>
          <div v-for="n in roster" :key="n.id" class="member-item" :class="{ offline: !n.online }">
            <span class="member-dot" :class="n.online ? 'on' : 'off'"></span>
            <span class="member-name">{{ n.name }}</span>
            <span class="muted member-role">{{ n.role }}/{{ n.category }}</span>
          </div>
        </div>
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
        <div class="composer" style="position: relative;">
          <!-- F2：@ 补全弹层 -->
          <div v-if="mention && mentionItems.length" class="mention-popup">
            <button
              v-for="item in mentionItems"
              :key="item.token"
              class="mention-item"
              @mousedown.prevent="pickMention(item.token)"
            >
              <strong>{{ item.token }}</strong>
              <span class="muted">{{ item.desc }}</span>
            </button>
          </div>
          <textarea
            ref="draftEl"
            v-model="draft"
            class="form-textarea composer-input"
            rows="2"
            :placeholder="`发到 ${activeChannel()?.name || ''}…（输入 @ 点名设备；Enter 发送，Shift+Enter 换行）`"
            @input="onDraftInput"
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
/* F1 成员面板 */
.member-section {
  margin-top: var(--space-3);
  border-top: 1px solid var(--border);
  padding-top: var(--space-2);
}
.member-head {
  font-size: var(--text-xs);
  padding: 0 var(--space-2) var(--space-1);
}
.member-item {
  display: flex;
  align-items: center;
  gap: var(--space-1);
  padding: var(--space-1) var(--space-2);
  font-size: var(--text-xs);
}
.member-item.offline {
  opacity: 0.5;
}
.member-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}
.member-dot.on {
  background: var(--success, #22c55e);
}
.member-dot.off {
  background: var(--text-muted);
}
.member-name {
  font-weight: 600;
}
.member-role {
  margin-left: auto;
}
/* F2 @ 补全弹层 */
.mention-popup {
  position: absolute;
  bottom: 100%;
  left: var(--space-3);
  right: var(--space-3);
  background: var(--bg-primary);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  box-shadow: 0 -4px 16px rgba(0, 0, 0, 0.15);
  max-height: 220px;
  overflow-y: auto;
  z-index: 10;
}
.mention-item {
  display: flex;
  gap: var(--space-2);
  align-items: baseline;
  width: 100%;
  text-align: left;
  padding: var(--space-2) var(--space-3);
  background: transparent;
  border: none;
  cursor: pointer;
  font-size: var(--text-sm);
  color: var(--text-primary);
}
.mention-item:hover {
  background: var(--bg-secondary);
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
