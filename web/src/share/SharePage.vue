<script setup lang="ts">
/**
 * L4（2026-09-07 devtool-upgrade 阶段 7）：会话分享只读页。
 *
 * URL 形如 /share/?t=<token>。token 即凭据 —— GET /api/share/{token}
 * 不过 dashboard 认证（未知/撤销/会话已删一律 404）。渲染只透
 * role/content/timestamp/model + 图片数量占位（后端白名单投影，
 * 本地路径/base64/file_changes 绝不出域）。
 */
import { ref, onMounted } from 'vue'

interface ShareMessage {
  role: string
  content: string
  timestamp: string
  model?: string
  image_count?: number
}

const loading = ref(true)
const error = ref('')
const title = ref('')
const createdAt = ref('')
const messages = ref<ShareMessage[]>([])

function tokenFromUrl(): string {
  return new URLSearchParams(location.search).get('t') ?? ''
}

function fmtTime(ts: string): string {
  if (!ts) return ''
  try {
    return new Date(ts).toLocaleString()
  } catch {
    return ts
  }
}

async function load() {
  const t = tokenFromUrl()
  if (!t) {
    error.value = '链接缺少分享令牌（?t=）'
    loading.value = false
    return
  }
  try {
    const resp = await fetch(`/api/share/${encodeURIComponent(t)}`)
    if (!resp.ok) {
      const body = await resp.json().catch(() => ({}))
      error.value = body?.error ?? `加载失败（HTTP ${resp.status}）`
      return
    }
    const data = await resp.json()
    title.value = data.title ?? '会话分享'
    createdAt.value = data.created_at ?? ''
    messages.value = data.messages ?? []
  } catch {
    error.value = '网络错误，无法加载分享内容'
  } finally {
    loading.value = false
  }
}

onMounted(load)
</script>

<template>
  <div class="share-page">
    <header class="share-header">
      <div class="share-brand">NemesisBot · 会话分享</div>
      <h1 v-if="title" class="share-title">{{ title }}</h1>
      <div v-if="createdAt" class="share-sub">分享创建于 {{ fmtTime(createdAt) }} · 实时只读视图</div>
    </header>

    <main class="share-body">
      <div v-if="loading" class="share-state">加载中…</div>
      <div v-else-if="error" class="share-state share-error">{{ error }}</div>
      <template v-else>
        <div v-for="(m, i) in messages" :key="i" class="msg" :class="m.role">
          <div class="msg-meta">
            <span class="msg-role">{{ m.role === 'user' ? '用户' : '助手' }}</span>
            <span v-if="m.model" class="msg-model">{{ m.model }}</span>
            <span class="msg-time">{{ fmtTime(m.timestamp) }}</span>
          </div>
          <div class="msg-content">{{ m.content }}</div>
          <div v-if="m.image_count" class="msg-images">🖼 {{ m.image_count }} 张图片（分享视图不展示图片内容）</div>
        </div>
        <div v-if="!messages.length" class="share-state">该会话暂无可展示的消息。</div>
      </template>
    </main>

    <footer class="share-footer">
      由 NemesisBot 分享 · 只读快照链接 · 内容不包含图片与文件变更明细
    </footer>
  </div>
</template>

<style scoped>
.share-page {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
  background: var(--bg-primary);
  color: var(--text-primary);
}

.share-header {
  padding: 20px 24px 12px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-secondary);
}

.share-brand { font-size: 12px; color: var(--text-muted); letter-spacing: 0.05em; }

.share-title {
  margin: 6px 0 4px;
  font-size: 20px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-sub { font-size: 12px; color: var(--text-muted); }

.share-body {
  flex: 1;
  width: 100%;
  max-width: 860px;
  margin: 0 auto;
  padding: 20px 24px 48px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.share-state { color: var(--text-muted); text-align: center; padding: 48px 0; }
.share-error { color: var(--danger, #dc3545); }

.msg { border: 1px solid var(--border); border-radius: 10px; padding: 12px 16px; background: var(--bg-secondary); }
.msg.user { border-left: 3px solid var(--accent, #4f8cff); }
.msg.assistant { border-left: 3px solid var(--success, #34c77b); }

.msg-meta { display: flex; align-items: center; gap: 10px; font-size: 12px; color: var(--text-muted); margin-bottom: 6px; }
.msg-role { font-weight: 600; color: var(--text-secondary); }
.msg-model { background: var(--bg-primary); border-radius: 4px; padding: 1px 6px; }
.msg-time { margin-left: auto; }

.msg-content { font-size: 14px; line-height: 1.7; white-space: pre-wrap; word-break: break-word; }

.msg-images { margin-top: 8px; font-size: 12px; color: var(--text-muted); }

.share-footer {
  padding: 12px 24px;
  border-top: 1px solid var(--border);
  font-size: 12px;
  color: var(--text-muted);
  text-align: center;
  background: var(--bg-secondary);
}
</style>
