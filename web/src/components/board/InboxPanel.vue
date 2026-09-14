<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { useBoardChanged } from '../../composables/useBoardChanged'
import IssueDetailModal from './IssueDetailModal.vue'
import { fmtTime } from './boardMeta'

// 收件箱（W2 P3）：站内通知列表。通知由后端 store 事件钩子产生（评论/
// @提及/指派/状态变化）；MVP 单管理员语义（admin wildcard，后端 inbox.list）。
// 未读徽标 + 单条已读 + 全部已读 + 仅看未读过滤。经通道的站外投递留 P4。
// goal P1/H2+H3：卡片显示所属 issue 上下文（标题解析+点击打开详情弹窗
// 看到评论原上下文）；标题与正文视觉分层（字色/字号区分）。

const { request } = useWSAPI()
const toast = useToast()

interface Notification {
  id: number
  recipient: { kind: string; id: string }
  kind: string
  title: string
  content: string
  issue_id: number | null
  read: boolean
  created_at: number
}

const KIND_BADGE: Record<string, string> = {
  commented: 'badge-info',
  mentioned: 'badge-warning',
  assigned: 'badge-info',
  status_changed: 'badge-neutral',
  dispatch_failed: 'badge-error',
}

const KIND_LABEL: Record<string, string> = {
  commented: '评论',
  mentioned: '提及',
  assigned: '指派',
  status_changed: '状态',
  dispatch_failed: '派发失败',
}

const loading = ref(true)
const notifications = ref<Notification[]>([])
const unread = ref(0)
const unreadOnly = ref(false)
// H2：issue_id → 标题解析缓存（卡片显示所属上下文）；detailIssueId 非空 =
// 详情弹窗打开（看到评论原上下文）。
const issueTitles = ref<Record<number, string>>({})
const detailIssueId = ref<number | null>(null)

async function resolveIssueTitles() {
  const ids = [
    ...new Set(
      notifications.value
        .map((n) => n.issue_id)
        .filter((v): v is number => v !== null && !(v in issueTitles.value)),
    ),
  ]
  for (const id of ids) {
    try {
      const r = await request('board', 'issue.get', { id })
      issueTitles.value[id] = r?.issue?.title || `#${id}`
    } catch {
      issueTitles.value[id] = `#${id}`
    }
  }
}

function openContext(n: Notification) {
  markRead(n)
  if (n.issue_id !== null) {
    detailIssueId.value = n.issue_id
  }
}

async function load(silent = false) {
  if (!silent) loading.value = true
  try {
    const r = await request('board', 'inbox.list', { unread_only: unreadOnly.value })
    notifications.value = r?.notifications || []
    unread.value = r?.unread || 0
    await resolveIssueTitles()
  } catch (e: any) {
    if (silent) console.warn('[InboxPanel] silent refresh failed:', e)
    else toast.error('加载收件箱失败: ' + e)
  } finally {
    loading.value = false
  }
}

async function markRead(n: Notification) {
  if (n.read) return
  try {
    const r = await request('board', 'inbox.mark_read', { id: n.id })
    n.read = true
    unread.value = r?.unread ?? Math.max(0, unread.value - 1)
  } catch (e: any) {
    toast.error('标记已读失败: ' + e)
  }
}

async function markAllRead() {
  try {
    const r = await request('board', 'inbox.mark_read', { all: true })
    toast.success(`已全部标记（${r?.marked ?? 0} 条）`)
    await load()
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

onMounted(load)
// board-changed 推送：新通知（评论/派发/状态变更）到达时静默换新。
useBoardChanged(() => load(true))
</script>

<template>
  <div>
    <div class="panel-toolbar">
      <button class="btn btn-primary" :disabled="unread === 0" @click="markAllRead">
        全部已读{{ unread ? `（${unread}）` : '' }}
      </button>
      <label class="muted" style="display: flex; align-items: center; gap: var(--space-1); cursor: pointer;">
        <input type="checkbox" v-model="unreadOnly" @change="load()" />
        仅看未读
      </label>
      <span class="muted">共 {{ notifications.length }} 条 · 未读 {{ unread }}</span>
    </div>

    <div v-if="loading" style="text-align: center; padding: var(--space-8);">
      <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
    </div>

    <div v-else-if="notifications.length === 0" class="empty-state">
      <h3>{{ unreadOnly ? '没有未读通知' : '暂无通知' }}</h3>
      <p>issue 被评论、@提及、指派或状态变化时会出现在这里</p>
    </div>

    <div v-else class="inbox-list">
      <div
        v-for="n in notifications"
        :key="n.id"
        class="inbox-item"
        :class="{ unread: !n.read, linked: n.issue_id !== null }"
        @click="openContext(n)"
      >
        <div class="inbox-item-head">
          <span class="badge" :class="KIND_BADGE[n.kind] || 'badge-neutral'">{{ KIND_LABEL[n.kind] || n.kind }}</span>
          <strong class="inbox-item-title">{{ n.title }}</strong>
          <span v-if="!n.read" class="unread-dot" title="未读"></span>
          <span class="muted inbox-item-time">{{ fmtTime(n.created_at) }}</span>
        </div>
        <div v-if="n.content" class="inbox-item-body">{{ n.content }}</div>
        <div v-if="n.issue_id !== null" class="inbox-item-context muted">
          ↳ {{ issueTitles[n.issue_id] || `#${n.issue_id}` }} · 点击查看原上下文
        </div>
      </div>
    </div>

    <!-- H2：评论的原上下文 = issue 详情弹窗（完整线程/活动/订阅者） -->
    <IssueDetailModal
      v-if="detailIssueId !== null"
      :issue-id="detailIssueId"
      @close="detailIssueId = null"
    />
  </div>
</template>

<style scoped>
.muted {
  color: var(--text-muted);
  font-size: var(--text-sm);
}
.panel-toolbar {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-4);
}
.panel-toolbar > .muted:last-child {
  margin-left: auto;
}
.inbox-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.inbox-item {
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: var(--space-3);
  cursor: pointer;
}
.inbox-item.unread {
  border-left: 3px solid var(--accent);
}
.inbox-item-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-1);
}
/* H3：标题与正文视觉分层——标题加粗主色，正文弱化灰。 */
.inbox-item-title {
  color: var(--text-primary);
  font-size: var(--text-sm);
}
.inbox-item-time {
  margin-left: auto;
}
.inbox-item-body {
  font-size: var(--text-sm);
  color: var(--text-muted);
  white-space: pre-wrap;
  margin-bottom: var(--space-1);
}
.inbox-item-context {
  font-size: var(--text-xs);
}
.inbox-item.linked {
  cursor: pointer;
}
.inbox-item.linked:hover {
  border-color: var(--accent);
}
.unread-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--accent);
  display: inline-block;
}
</style>
