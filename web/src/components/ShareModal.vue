<script setup lang="ts">
/**
 * L4（2026-09-07 devtool-upgrade 阶段 7）：会话分享弹窗 ——
 * 创建/复制/撤销只读分享链接。
 *
 * 后端：sessions.share_create / share_list / share_revoke（存储 +
 * 白名单投影在 crate::share；公开只读端点 GET /api/share/{token}，
 * token 即凭据）。链接指向独立 share 页（第 4 个 Vite MPA 入口）。
 *
 * 诚实边界（页面同样标注）：live 视图非快照 —— 会话继续增长则
 * 分享页同步增长；撤销即刻失效；无过期时间/密码。
 */
import { ref, computed, watch } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const props = defineProps<{ sessionId: string }>()
const emit = defineEmits<{ (e: 'close'): void }>()

interface ShareItem {
  token: string
  session_id: string
  created_at: string
  revoked: boolean
  title?: string | null
}

const { request } = useWSAPI()
const toast = useToast()

const loading = ref(false)
const creating = ref(false)
const items = ref<ShareItem[]>([])
const showRevoked = ref(false)

const active = computed(() => items.value.filter((s) => s.session_id === props.sessionId && !s.revoked))
const revokedList = computed(() => items.value.filter((s) => s.session_id === props.sessionId && s.revoked))

async function refresh() {
  if (!props.sessionId) return
  loading.value = true
  try {
    const resp = await request('sessions', 'share_list')
    items.value = resp?.shares ?? []
  } catch (e: any) {
    toast.error(e?.message || '拉取分享列表失败')
  } finally {
    loading.value = false
  }
}

watch(() => props.sessionId, refresh, { immediate: true })

async function create() {
  if (creating.value) return
  creating.value = true
  try {
    await request('sessions', 'share_create', { session_id: props.sessionId })
    toast.success('分享链接已创建')
    await refresh()
  } catch (e: any) {
    toast.error(e?.message || '创建分享失败')
  } finally {
    creating.value = false
  }
}

function shareUrl(token: string): string {
  return `${location.origin}/share/?t=${token}`
}

async function copy(token: string) {
  const url = shareUrl(token)
  try {
    await navigator.clipboard.writeText(url)
    toast.success('链接已复制')
  } catch {
    toast.error('复制失败，请手动复制链接文本')
  }
}

async function revoke(token: string) {
  try {
    await request('sessions', 'share_revoke', { token })
    toast.success('分享已撤销，链接即刻失效')
    await refresh()
  } catch (e: any) {
    toast.error(e?.message || '撤销失败')
  }
}

function fmtTime(iso: string): string {
  try {
    return new Date(iso).toLocaleString()
  } catch {
    return iso
  }
}
</script>

<template>
  <div class="modal-backdrop" @click.self="emit('close')">
    <div class="modal share-modal">
      <div class="modal-header">
        <h3>🔗 分享会话（只读）</h3>
        <button class="close-btn" @click="emit('close')">×</button>
      </div>
      <div class="modal-body">
        <p class="hint">
          生成一条只读分享链接：对方无需登录即可在独立页面查看本会话的
          <strong>实时只读视图</strong>（会话继续增长则同步可见）。链接含随机
          token，可随时撤销（即刻失效）。图片只显示数量，本地路径不会外泄。
        </p>

        <div v-if="loading" class="share-loading">加载中…</div>
        <template v-else>
          <div v-for="s in active" :key="s.token" class="share-item">
            <div class="share-url-row">
              <code class="share-url">{{ shareUrl(s.token) }}</code>
              <button class="btn btn-sm" @click="copy(s.token)">复制</button>
            </div>
            <div class="share-meta">
              创建于 {{ fmtTime(s.created_at) }}
              <button class="share-revoke" @click="revoke(s.token)">撤销</button>
            </div>
          </div>

          <div v-if="!active.length" class="share-empty">本会话还没有分享链接。</div>

          <button class="btn btn-primary share-create" :disabled="creating" @click="create">
            {{ creating ? '创建中…' : active.length ? '分享已存在' : '创建分享链接' }}
          </button>

          <div v-if="revokedList.length" class="share-revoked">
            <button class="share-revoked-toggle" @click="showRevoked = !showRevoked">
              {{ showRevoked ? '▾' : '▸' }} 已撤销（{{ revokedList.length }}）
            </button>
            <div v-for="s in showRevoked ? revokedList : []" :key="s.token" class="share-item revoked">
              <code class="share-url dead">…{{ s.token.slice(-8) }}</code>
              <span class="share-meta">已撤销 · 创建于 {{ fmtTime(s.created_at) }}</span>
            </div>
          </div>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.share-modal { max-width: 560px; }

.hint {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  line-height: 1.6;
  margin: 0 0 var(--space-3);
}

.share-loading { color: var(--text-muted); font-size: var(--text-sm); padding: var(--space-2) 0; }

.share-item {
  border: 1px solid var(--border-light);
  border-radius: var(--radius-md);
  padding: var(--space-2) var(--space-3);
  margin-bottom: var(--space-2);
}
.share-item.revoked { opacity: 0.6; }

.share-url-row { display: flex; align-items: center; gap: var(--space-2); }

.share-url {
  flex: 1;
  font-size: var(--text-xs);
  background: var(--bg-secondary);
  padding: 4px 8px;
  border-radius: var(--radius-sm);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.share-url.dead { text-decoration: line-through; }

.share-meta {
  margin-top: var(--space-1);
  font-size: var(--text-xs);
  color: var(--text-muted);
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.share-revoke {
  background: transparent;
  border: 1px solid var(--danger, #dc3545);
  color: var(--danger, #dc3545);
  border-radius: var(--radius-sm);
  font-size: var(--text-xs);
  padding: 1px 8px;
  cursor: pointer;
}
.share-revoke:hover { background: var(--danger, #dc3545); color: #fff; }

.share-empty { color: var(--text-muted); font-size: var(--text-sm); margin-bottom: var(--space-2); }

.share-create { width: 100%; }
.share-create:disabled { opacity: 0.6; cursor: not-allowed; }

.share-revoked { margin-top: var(--space-2); }
.share-revoked-toggle {
  background: transparent;
  border: none;
  color: var(--text-muted);
  font-size: var(--text-xs);
  cursor: pointer;
  padding: 2px 0;
}
</style>
