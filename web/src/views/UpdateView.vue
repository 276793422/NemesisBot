<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { on as onSSE, off as offSSE } from '../composables/useSSE'

// 【更新】页（2026-10-27）：当前运行版本 vs 云端最新版本对照 + 手动更新
// 操作面。痛点闭环——更新成功重启后侧栏长条消失，无处确认「现在跑的就是
// 新版本」；本页对照卡给出终态证明（当前=新版本 + 无可用更新）。
// 数据源 = selfupdate.status（10s 轮询）+ selfupdate.check（手动）+
// SSE selfupdate-progress。状态机与操作语义照抄 Sidebar 自更新长条（同源
// 同命令，两处零耦合）。

const { request } = useWSAPI()

// ── 状态机（五态，与侧栏 suState 同语义）──
type SuState = 'hidden' | 'idle' | 'downloading' | 'ready' | 'applying'
const st = ref<SuState>('hidden')

// ── 版本对照卡 ──
const curVersion = ref('--')
const curCommit = ref('')
const commitKnown = ref(false)
interface CloudCheck {
  latest_tag: string
  latest_commit: string
  update_available: boolean
  checked_at_ms?: number
  asset_present?: boolean
}
const cloud = ref<CloudCheck | null>(null)
const checking = ref(false)

// ── 操作区 ──
const pendingTag = ref('')
const pct = ref<number | null>(null)
const downloaded = ref(0)
const total = ref<number | null>(null)
const dlError = ref('')
// apply 失败内联红条（单一裁决点 = status.last_apply_error；后端在新一轮
// apply 受理时清空——红条随之消失，不做过期粘滞）。
const lastApplyError = ref('')
const applyErrDowngrade = ref(false)
const baseline = ref(true)

// ── 空态 / 闸面 ──
const wired = ref(true)
const platformOk = ref(true)
const journalPhase = ref<string | null>(null)

function fmtMb(n: number) {
  return (n / (1024 * 1024)).toFixed(1) + 'MB'
}
function shortCommit(c: string) {
  return c ? c.slice(0, 12) : ''
}
function fmtTime(ms?: number) {
  if (!ms) return ''
  return new Date(ms).toLocaleString()
}

function applyProgress(d: number, t: unknown) {
  downloaded.value = d
  const tv = typeof t === 'number' ? t : null
  total.value = tv
  pct.value = tv !== null && tv > 0 ? Math.min(100, Math.round((d * 100) / tv)) : null
}

const progressLabel = computed(() => {
  if (pct.value !== null) {
    return `${pct.value}%（${fmtMb(downloaded.value)} / ${fmtMb(total.value ?? 0)}）`
  }
  return fmtMb(downloaded.value)
})
const progressWidth = computed(() => (pct.value === null ? '40%' : pct.value + '%'))

// ── 对照卡三态徽标：✅ 已是最新 / ⬆ 有新版本 / ？版本未知 ──
type Badge = { cls: 'ok' | 'warn' | 'muted'; label: string; title: string }
const badge = computed<Badge>(() => {
  if (!cloud.value) {
    return { cls: 'muted', label: '版本未知', title: '尚未取得云端版本信息：点【检查更新】实时查询' }
  }
  if (cloud.value.update_available) {
    return { cls: 'warn', label: `有新版本 ${cloud.value.latest_tag}`, title: '云端有比当前更新的发布产物' }
  }
  return {
    cls: 'ok',
    label: '已是最新',
    title: '当前运行版本与云端最新一致——更新成功的终态证明',
  }
})

async function refresh() {
  let resp: any = null
  try {
    resp = await request('selfupdate', 'status', {}, 5000)
  } catch {
    return // WS 未就绪——保持原状态，下个轮询周期再试
  }
  if (!resp) return
  curVersion.value = resp.version || '--'
  curCommit.value = resp.commit || ''
  commitKnown.value = resp.commit_known !== false
  wired.value = resp.wired !== false
  platformOk.value = resp.platform_supported !== false
  journalPhase.value = resp.journal_phase ?? null
  baseline.value = resp.sequence_baseline_present !== false
  if (resp.cached_check && resp.cached_check.latest_tag !== undefined) {
    cloud.value = resp.cached_check
  }
  // apply 失败红条（后端在新一轮 apply 受理时清空，红条随之消失）。
  const applyErr = resp.last_apply_error
  lastApplyError.value = applyErr && applyErr.message ? applyErr.message : ''
  applyErrDowngrade.value = !!(applyErr && applyErr.downgrade_denied)
  // 操作区状态机（侧栏 refreshSelfupdate 同构）。
  if (!resp.wired || !resp.platform_supported || resp.journal_phase) {
    st.value = 'hidden'
    return
  }
  if (st.value === 'applying') {
    if (!applyErr && resp.pending) return
    st.value = resp.pending && resp.pending.release_tag ? 'ready' : 'hidden'
  }
  if (resp.pending && resp.pending.release_tag) {
    st.value = 'ready'
    pendingTag.value = resp.pending.release_tag
    return
  }
  if (resp.download && resp.download.active) {
    st.value = 'downloading'
    applyProgress(resp.download.downloaded_bytes ?? 0, resp.download.total_bytes)
    return
  }
  dlError.value = (resp.download && resp.download.phase === 'error' && resp.download.error) || ''
  if (cloud.value && cloud.value.update_available) {
    pendingTag.value = cloud.value.latest_tag || ''
    st.value = 'idle'
    return
  }
  st.value = 'hidden'
}

// ── 检查更新（实时；15s 超时对齐侧栏 suCheckedOnce 同值）──
async function checkNow() {
  if (checking.value) return
  checking.value = true
  try {
    const c: any = await request('selfupdate', 'check', {}, 15000)
    if (c && c.latest_tag !== undefined) {
      cloud.value = {
        latest_tag: c.latest_tag ?? '',
        latest_commit: c.latest_commit ?? '',
        update_available: !!c.update_available,
        checked_at_ms: Date.now(),
        asset_present: !!c.asset_present,
      }
      // 对照刷新后操作区状态可能变化（有更新 → idle 可下载）。
      refresh()
    }
  } catch (e: any) {
    // 云端侧保持原样（badge 维持「版本未知」或旧缓存），失败原因 toast。
    useToast().error(`检查更新失败：${e?.message ?? e}`)
  } finally {
    checking.value = false
  }
}

// ── 操作区动作（语义照抄侧栏）──
async function startDownload() {
  try {
    await request('selfupdate', 'download', {}, 8000)
    st.value = 'downloading'
    applyProgress(0, null) // 真实进度由 SSE 帧驱动；幂等受理则下轮 status 校正
  } catch (e: any) {
    useToast().error(`下载受理失败：${e?.message ?? e}`)
  }
}

async function cancelDownload() {
  try {
    await request('selfupdate', 'cancel_download', {}, 5000)
  } catch { /* 取消失败不打断——下轮轮询按真实态校正 */ }
  st.value = 'idle'
}

async function applyRequest(data: Record<string, unknown>) {
  try {
    await request('selfupdate', 'apply', data, 8000)
    st.value = 'applying'
  } catch (e: any) {
    useToast().error(`更新受理失败：${e?.message ?? e}`)
  }
}

async function startApply() {
  const target = pendingTag.value || '新版本'
  // 首更形态（无 applied 基准）提前告知：顺序闸默认 Unknown 拒绝，随确认
  // 框取到豁免授权一并下发（签名验真与完整性闸照常，不受豁免影响）。
  const firstUpdate = !baseline.value
  const notice = firstUpdate
    ? `确定更新到 ${target}？本机没有已安装版本的清单基准（首次更新形态），将跳过版本顺序比对（签名验真照常）。更新将校验签名、替换程序并自动重启。`
    : `确定更新到 ${target}？更新将校验签名、替换程序并自动重启。`
  if (!window.confirm(notice)) return
  await applyRequest(firstUpdate ? { allow_downgrade: true } : {})
}

// 顺序闸拒绝后的显式豁免重试（内联按钮，不打断式弹窗）：豁免只放开顺序
// 判定，签名验真照常；操作记入审计链。
async function retryApplyWithExemption() {
  await applyRequest({ allow_downgrade: true })
}

// ── SSE 实时进度 ──
function onProgress(d: any) {
  if (!d || typeof d !== 'object') return
  if (d.phase === 'ready' || d.phase === 'error') {
    if (d.phase === 'error' && !d.message) {
      // 不带 message = 独立下载失败（updates/ 已整清，可重下）；带 message
      // 的 apply 编排失败不在这里 toast——refresh() 从 status 的
      // last_apply_error 单点处理（红条 + 豁免按钮），避免双报。
      useToast().warn('更新包下载失败，已清理，可重新下载')
    }
    refresh() // 收口态以 status 快照为准
    return
  }
  st.value = 'downloading'
  applyProgress(d.downloaded_bytes ?? 0, d.total_bytes)
}

let timer: number | null = null

onMounted(() => {
  refresh()
  timer = window.setInterval(refresh, 10000)
  onSSE('selfupdate-progress', onProgress)
})

onUnmounted(() => {
  if (timer !== null) window.clearInterval(timer)
  offSSE('selfupdate-progress', onProgress)
})
</script>

<template>
  <div class="page-update">
    <div class="page-header"><h2>更新</h2></div>
    <div class="page-body">
      <!-- 版本对照卡 -->
      <div class="card">
        <div class="card-header">
          <h3>版本对照</h3>
          <span data-test="badge" class="badge" :class="`badge-${badge.cls}`" :title="badge.title">{{ badge.label }}</span>
        </div>
        <div class="card-body">
          <div class="cmp-grid">
            <div data-test="cmp-current" class="cmp-cell">
              <div class="cmp-key">当前运行</div>
              <div class="cmp-version">{{ curVersion }}</div>
              <div v-if="commitKnown" class="cmp-sub" :title="curCommit">commit {{ shortCommit(curCommit) }}</div>
              <div v-else class="cmp-sub cmp-muted">commit 未知（旧版本 / 开发构建）</div>
            </div>
            <div class="cmp-divider"></div>
            <div data-test="cmp-cloud" class="cmp-cell">
              <div class="cmp-key">云端最新</div>
              <div class="cmp-version">{{ cloud ? cloud.latest_tag : '—' }}</div>
              <div v-if="cloud && cloud.latest_commit" class="cmp-sub" :title="cloud.latest_commit">commit {{ shortCommit(cloud.latest_commit) }}</div>
              <div v-else class="cmp-sub cmp-muted">版本未知（未查询或查询失败）</div>
              <div v-if="cloud && cloud.checked_at_ms" class="cmp-sub cmp-muted">检查于 {{ fmtTime(cloud.checked_at_ms) }}</div>
            </div>
          </div>
          <div class="cmp-actions">
            <button data-test="check-btn" class="btn btn-secondary" :disabled="checking" @click="checkNow">
              {{ checking ? '检查中…' : '检查更新' }}
            </button>
          </div>
        </div>
      </div>

      <!-- 未收口更新事务（启动收口自动处理，期间不干预——侧栏同语义） -->
      <div v-if="journalPhase" data-test="journal-note" class="card">
        <div class="card-body note warn-note">
          有未收口的更新事务（journal phase={{ journalPhase }}）：重启 gateway 后启动收口会自动处理（完成或回滚），期间更新操作不可用。
        </div>
      </div>

      <!-- apply 失败内联红条（后端在新一轮受理时清空） -->
      <div v-if="lastApplyError" data-test="apply-error" class="card">
        <div class="card-body note err-note">
          <div>更新执行失败：{{ lastApplyError }}</div>
          <button v-if="applyErrDowngrade" data-test="exempt-retry" class="btn btn-secondary btn-sm" style="margin-top: var(--space-2);" @click="retryApplyWithExemption">
            跳过版本比对重试（签名验真照常，记入审计链）
          </button>
        </div>
      </div>

      <!-- 操作区 -->
      <div class="card">
        <div class="card-header"><h3>更新操作</h3></div>
        <div class="card-body">
          <!-- 空态诚实 -->
          <div v-if="!wired" data-test="empty-unwired" class="empty-state">
            <p>当前构建未装配自更新通道（selfupdate feature 未编译）：请手动下载 Release 包替换。</p>
          </div>
          <div v-else-if="!platformOk" data-test="empty-platform" class="empty-state">
            <p>当前平台不支持自动更新：请手动下载 Release 包替换。</p>
          </div>
          <div v-else-if="journalPhase" class="empty-state">
            <p>更新事务收口中，操作暂不可用。</p>
          </div>

          <template v-else>
            <!-- idle：可下载 -->
            <div v-if="st === 'idle'" data-test="op-idle" class="op-row">
              <div class="op-text">
                <div class="op-title">{{ dlError ? `上次下载失败：${dlError}` : `发现新版本 ${pendingTag}` }}</div>
                <div class="op-sub">下载更新包（SHA-256 复验 + 签名验签通过后才会安装）</div>
              </div>
              <button data-test="dl-btn" class="btn btn-primary" @click="startDownload">{{ dlError ? '重试下载' : '下载更新' }}</button>
            </div>

            <!-- downloading：进度 + 取消 -->
            <div v-else-if="st === 'downloading'" data-test="op-downloading" class="op-row">
              <div class="op-text">
                <div class="op-title">正在下载更新包…</div>
                <div class="progress-track">
                  <div class="progress-bar" :style="{ width: progressWidth }"></div>
                </div>
                <div data-test="progress-label" class="op-sub">{{ progressLabel }}</div>
              </div>
              <button data-test="dl-cancel" class="btn btn-secondary" @click="cancelDownload">取消</button>
            </div>

            <!-- ready：待确认安装 -->
            <div v-else-if="st === 'ready'" data-test="op-ready" class="op-row">
              <div class="op-text">
                <div class="op-title">更新包已就绪（{{ pendingTag }}，已通过 SHA-256 复验）</div>
                <div class="op-sub">点击开始更新，确认后将校验签名并自动重启</div>
              </div>
              <button data-test="apply-btn" class="btn btn-primary" @click="startApply">开始更新</button>
            </div>

            <!-- applying：执行中 -->
            <div v-else-if="st === 'applying'" data-test="op-applying" class="op-row">
              <div class="op-text">
                <div class="op-title">更新执行中…</div>
                <div class="op-sub">签名校验与程序替换进行中，成功后自动重启（请勿关闭进程）</div>
              </div>
            </div>

            <!-- hidden：无可用操作 -->
            <div v-else data-test="op-none" class="empty-state">
              <p>{{ badge.cls === 'ok' ? '当前已是最新版本，无需更新。' : '暂无可用更新操作——点上方【检查更新】查询云端版本。' }}</p>
            </div>
          </template>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.cmp-grid {
  display: flex;
  align-items: stretch;
  gap: var(--space-4);
}
.cmp-cell {
  flex: 1;
  min-width: 0;
}
.cmp-divider {
  flex: none;
  width: 1px;
  background: var(--border);
}
.cmp-key {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-bottom: var(--space-1);
}
.cmp-version {
  font-size: var(--text-xl);
  font-weight: 600;
}
.cmp-sub {
  font-size: var(--text-xs);
  color: var(--text-secondary);
  margin-top: 2px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.cmp-muted {
  color: var(--text-muted);
}
.cmp-actions {
  margin-top: var(--space-4);
  display: flex;
  justify-content: flex-end;
}
.badge {
  font-size: var(--text-xs);
  font-weight: 600;
  padding: 2px 10px;
  border-radius: 999px;
}
.badge-ok {
  color: var(--success);
  background: color-mix(in srgb, var(--success) 12%, transparent);
}
.badge-warn {
  color: var(--warning);
  background: color-mix(in srgb, var(--warning) 12%, transparent);
}
.badge-muted {
  color: var(--text-muted);
  background: var(--bg-secondary);
}
.note {
  font-size: var(--text-sm);
}
.warn-note {
  color: var(--warning);
}
.err-note {
  color: var(--danger);
}
.op-row {
  display: flex;
  align-items: center;
  gap: var(--space-4);
}
.op-text {
  flex: 1;
  min-width: 0;
}
.op-title {
  font-weight: 600;
}
.op-sub {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-top: var(--space-1);
}
.progress-track {
  height: 6px;
  border-radius: 3px;
  background: var(--bg-secondary);
  overflow: hidden;
  margin-top: var(--space-2);
}
.progress-bar {
  height: 100%;
  border-radius: 3px;
  background: var(--primary);
  transition: width 0.3s ease;
}
</style>
