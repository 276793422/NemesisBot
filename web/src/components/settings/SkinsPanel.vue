<script setup lang="ts">
/**
 * 设置页「皮肤」tab 面板（skins feature / VITE_FEATURE_SKINS 门控）。
 *
 * 卡片列表（含 broken 灰卡——D6 灰卡展示而非隐藏）+ 签名徽标四态 +
 * 设为默认观感（WSAPI skins.set_active → useSkin.applySkinRefresh 免刷新
 * 换肤）+ 重新加载 + 下载 stub。管理面按需现扫（stateless
 * scan-per-call），reload = 语义锚点命令。皮肤只有一种语义：给当前应用
 * 换观感（无任何「打开独立应用」入口——app 形态已裁定移除）。
 */
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { applySkinRefresh } from '../../composables/useSkin'

interface SkinManifest {
  id: string
  name: string
  version: string
  author: string
  description: string
  type: string
  variants: string[]
  entry?: string | null
}
interface SkinEntry {
  id: string
  file: string
  status: 'ok' | 'broken'
  status_detail?: string | null
  signature: 'verified' | 'unsigned' | 'invalid' | 'unverified'
  sig_detail?: string | null
  manifest: SkinManifest
  sha256: string
  id_mismatch: boolean
}
interface ListResp {
  dir: string | null
  dir_exists: boolean
  skins: SkinEntry[]
}

const { request } = useWSAPI()
const toast = useToast()

const skins = ref<SkinEntry[]>([])
const dir = ref<string | null>(null)
const dirExists = ref(true)
const loading = ref(true)
const busy = ref(false)
const activeId = ref('default')

const SIG_BADGES: Record<string, { label: string; cls: string; title: string }> = {
  verified: { label: '✅ 已验证', cls: 'sig-verified', title: '签名验证通过（官方根锚）' },
  unsigned: { label: '⚪ 未签名', cls: 'sig-unsigned', title: '无签名——来源未知，可正常使用' },
  invalid: { label: '🔴 签名无效', cls: 'sig-invalid', title: '' },
  unverified: {
    label: '❔ 无法验证',
    cls: 'sig-unverified',
    title: '本机构建未注入验证根锚，无法验证签名（官方 CI 构建可验证）',
  },
}

function sigBadge(e: SkinEntry) {
  const b = SIG_BADGES[e.signature] || SIG_BADGES.unverified
  const title = e.signature === 'invalid' && e.sig_detail
    ? `签名验证失败：${e.sig_detail}`
    : b.title
  return { ...b, title }
}

function isTheme(e: SkinEntry): boolean {
  return e.status === 'ok' && !!e.manifest.entry
}

function applyList(data: ListResp | null) {
  skins.value = data?.skins || []
  dir.value = data?.dir ?? null
  dirExists.value = data?.dir_exists ?? false
  activeId.value = localStorage.getItem('nemesisbot_skin') || 'default'
}

async function load() {
  loading.value = true
  try {
    applyList((await request('skins', 'list')) as ListResp)
  } catch (e: any) {
    toast.error('皮肤列表加载失败: ' + e)
  }
  loading.value = false
}

async function reload() {
  busy.value = true
  try {
    applyList((await request('skins', 'reload')) as ListResp)
    toast.success('已重新扫描皮肤目录')
  } catch (e: any) {
    toast.error('重新加载失败: ' + e)
  }
  busy.value = false
}

async function setActive(id: string) {
  if (busy.value || id === activeId.value) return
  busy.value = true
  try {
    await request('skins', 'set_active', { id })
    await applySkinRefresh()
    activeId.value = localStorage.getItem('nemesisbot_skin') || id
    toast.success(id === 'default' ? '已关闭皮肤，回到默认观感' : `已切换到「${displayName(skins.value.find((s) => s.id === id))}」`)
  } catch (e: any) {
    toast.error('切换失败: ' + e)
  }
  busy.value = false
}

function displayName(e?: SkinEntry): string {
  if (!e) return ''
  return e.manifest.name || e.manifest.id || e.id
}

function shortSha(sha: string): string {
  return sha ? sha.slice(0, 12) : ''
}

onMounted(load)
</script>

<template>
  <div class="skins-panel">
    <div class="skins-toolbar">
      <div class="skins-dir" :title="dir || ''">
        皮肤目录：<code>{{ dir || '未知' }}</code>
        <span v-if="dir && !dirExists" class="dir-missing">（目录不存在）</span>
      </div>
      <div class="skins-actions">
        <button class="btn" :disabled="busy || loading" @click="reload">🔄 重新加载</button>
        <button class="btn" disabled title="皮肤下载即将上线（未来接 Release 分发）">⬇ 下载皮肤</button>
      </div>
    </div>

    <div v-if="loading" class="skins-empty">加载中…</div>
    <div v-else-if="!dir" class="skins-empty">皮肤系统未装配（exe 同级 skins 目录不可定位）。</div>

    <div class="skins-grid">
      <!-- 内置默认观感卡：set_active('default') = 关皮肤，回落内置 -->
      <div class="card skin-card" :class="{ 'skin-active': activeId === 'default' }">
        <div class="skin-head">
          <span class="skin-name">默认观感（内置）</span>
          <span v-if="activeId === 'default'" class="skin-active-tag">当前</span>
        </div>
        <p class="skin-desc">不启用任何 .nbskin 皮肤包，使用 Dashboard 内置观感。</p>
        <div class="skin-foot">
          <button
            class="btn btn-primary"
            :disabled="busy || activeId === 'default'"
            @click="setActive('default')"
          >
            {{ activeId === 'default' ? '当前皮肤' : '恢复默认' }}
          </button>
        </div>
      </div>

      <div
        v-for="s in skins"
        :key="s.id"
        class="card skin-card"
        :class="{ 'skin-broken': s.status === 'broken', 'skin-active': activeId === s.id }"
      >
        <div class="skin-head">
          <span class="skin-name">{{ displayName(s) }}</span>
          <span :class="['sig-badge', sigBadge(s).cls]" :title="sigBadge(s).title">{{ sigBadge(s).label }}</span>
          <span v-if="activeId === s.id" class="skin-active-tag">当前</span>
        </div>
        <p v-if="s.manifest.description" class="skin-desc">{{ s.manifest.description }}</p>
        <div class="skin-meta">
          <span v-if="s.manifest.version">v{{ s.manifest.version }}</span>
          <span v-if="s.manifest.author">{{ s.manifest.author }}</span>
          <span
            v-for="v in s.manifest.variants || []"
            :key="v"
            class="skin-variant"
          >{{ v === 'light' ? '亮色' : v === 'dark' ? '暗色' : v }}</span>
          <code class="skin-sha" :title="`SHA-256: ${s.sha256}`">{{ shortSha(s.sha256) }}</code>
        </div>
        <p v-if="s.status === 'broken'" class="skin-warn">⚠ 包体损坏：{{ s.status_detail }}</p>
        <p v-if="s.id_mismatch" class="skin-warn">
          ⚠ manifest.id（{{ s.manifest.id }}）与文件名（{{ s.id }}）不一致——元数据可信度存疑
        </p>
        <div class="skin-foot">
          <button
            v-if="isTheme(s)"
            class="btn btn-primary"
            :disabled="busy || activeId === s.id"
            @click="setActive(s.id)"
          >
            {{ activeId === s.id ? '当前皮肤' : '设为默认观感' }}
          </button>
        </div>
      </div>
    </div>

    <p class="skins-note">
      未签名 / 签名无效的皮肤包同样可加载使用（签名只是来源徽标）；「设为默认观感」
      是信任决策，受 <code>ui.skins.require_signed</code> 策略约束（默认关）。
    </p>
  </div>
</template>

<style scoped>
.skins-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  margin-bottom: var(--space-4);
  flex-wrap: wrap;
}
.skins-dir {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.skins-dir code {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
}
.dir-missing {
  color: var(--warning);
}
.skins-actions {
  display: flex;
  gap: var(--space-2);
  flex-shrink: 0;
}
.skins-empty {
  color: var(--text-muted);
  padding: var(--space-6) 0;
  text-align: center;
}
.skins-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: var(--space-4);
}
.skin-card {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  padding: var(--space-4);
}
.skin-card.skin-broken {
  opacity: 0.55;
}
.skin-card.skin-active {
  border-color: var(--accent);
}
.skin-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}
.skin-name {
  font-weight: 600;
  font-size: var(--text-base);
}
.skin-active-tag {
  font-size: var(--text-xs);
  color: var(--accent);
  border: 1px solid var(--accent);
  border-radius: 999px;
  padding: 0 8px;
  line-height: 18px;
}
.sig-badge {
  font-size: var(--text-xs);
  border-radius: 999px;
  padding: 0 8px;
  line-height: 18px;
  white-space: nowrap;
}
.sig-verified {
  color: var(--success);
  background: var(--success-bg);
}
.sig-unsigned {
  color: var(--text-secondary);
  background: var(--surface);
}
.sig-invalid {
  color: var(--error);
  background: var(--error-bg);
}
.sig-unverified {
  color: var(--text-muted);
  background: var(--surface);
}
.skin-desc {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  margin: 0;
}
.skin-meta {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
  font-size: var(--text-xs);
  color: var(--text-muted);
}
.skin-variant {
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 0 6px;
  line-height: 18px;
}
.skin-sha {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
}
.skin-warn {
  font-size: var(--text-xs);
  color: var(--warning);
  margin: 0;
}
.skin-foot {
  margin-top: auto;
  display: flex;
  gap: var(--space-2);
}
.skins-note {
  margin-top: var(--space-4);
  font-size: var(--text-xs);
  color: var(--text-muted);
}
</style>
