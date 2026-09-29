<script setup lang="ts">
/**
 * 设置页「皮肤」tab 面板（skins feature / VITE_FEATURE_SKINS 门控）。
 *
 * 卡片列表（含 broken 灰卡——D6 灰卡展示而非隐藏）+ 签名徽标五态（P3 起
 * +🚫 revoked）+ 设为默认观感（WSAPI skins.set_active → useSkin
 * .applySkinRefresh 免刷新换肤；**live 脚本在场时切换 = 整页刷新**——JS
 * 状态不可干净卸载，裁定 2）+ 重新加载 + 下载皮肤（P2 verify-before-
 * install 三入口：官方 Release / 任意 https URL / 本地文件导入，共用
 * 「验签 → 徽标 → 落盘」管线——所有信任结论都落盘，仅物理损坏拒收）+
 * CRL 快照状态行（P3 吊销第五态的管理面诚实呈现）+ 脚本能力（P2a）：
 * 「允许皮肤携带脚本」全局开关（config `ui.skins.allow_scripts`，关 =
 * 服务端 403 静默纯 CSS）+ 脚本同意卡（live 前置裁决；签名状态如实展示
 * 含 ⚠/🚫 但不拦截——开关 + 同意是唯一授权，裁定 1）。管理面按需现扫
 *（stateless scan-per-call），reload = 语义锚点命令。皮肤只有一种语义：
 * 给当前应用换观感（无任何「打开独立应用」入口——app 形态已裁定移除）。
 */
import { ref, computed, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'
import { applySkinRefresh, skinState, resolveScriptConsent, skinScriptNeedsReload, discardScriptPending } from '../../composables/useSkin'
import { authedFetch } from '../../lib/authFetch'

interface SkinManifest {
  id: string
  name: string
  version: string
  author: string
  description: string
  type: string
  variants: string[]
  entry?: string | null
  /** 结构载荷（v2 声明式结构引擎；在场 = 结构皮肤） */
  structure?: string | null
  /** 脚本载荷（P2a；在场 = 带脚本包，授权 = 开关 + 逐包同意） */
  script?: string | null
}
interface SkinEntry {
  id: string
  file: string
  status: 'ok' | 'broken'
  status_detail?: string | null
  signature: 'verified' | 'unsigned' | 'invalid' | 'unverified' | 'revoked'
  sig_detail?: string | null
  manifest: SkinManifest
  sha256: string
  id_mismatch: boolean
  /** 包带脚本载荷（管理面展示徽标；授权面见同意卡 + 全局开关） */
  has_script: boolean
}
/** CRL 快照管理面状态（P3；不在场 = 全默认，面板不渲染该行） */
interface CrlInfo {
  present: boolean
  verified: boolean
  expired: boolean
  version: number
  valid_until: number
  entries: number
  note?: string | null
}
interface ListResp {
  dir: string | null
  dir_exists: boolean
  skins: SkinEntry[]
  crl?: CrlInfo | null
}
/** 渠道安装单包结论（WSAPI skins.install / 导入端点返回体） */
interface InstallOutcome {
  id: string
  file: string
  signature: SkinEntry['signature']
  sig_detail?: string | null
  manifest?: Partial<SkinManifest> | null
  sha256: string
  overwritten: boolean
  source_file?: string
}

const { request } = useWSAPI()
const toast = useToast()

const skins = ref<SkinEntry[]>([])
const dir = ref<string | null>(null)
const dirExists = ref(true)
const crl = ref<CrlInfo | null>(null)
const loading = ref(true)
const busy = ref(false)
const activeId = ref('default')

const SIG_BADGES: Record<string, { label: string; cls: string; title: string }> = {
  verified: { label: '✅ 已验证', cls: 'sig-verified', title: '签名验证通过（官方根锚）' },
  unsigned: { label: '⚪ 未签名', cls: 'sig-unsigned', title: '无签名——来源未知，可正常使用' },
  invalid: { label: '🔴 签名无效', cls: 'sig-invalid', title: '' },
  revoked: {
    label: '🚫 已吊销',
    cls: 'sig-revoked',
    title: '签名被吊销（CRL 快照命中）——建议停用并移除该包',
  },
  unverified: {
    label: '❔ 无法验证',
    cls: 'sig-unverified',
    title: '本机构建未注入验证根锚，无法验证签名（官方 CI 构建可验证）',
  },
}

function sigBadgeInfo(sig: string, detail?: string | null) {
  const b = SIG_BADGES[sig] || SIG_BADGES.unverified
  const title =
    (sig === 'invalid' || sig === 'revoked') && detail
      ? `签名验证失败：${detail}`
      : b.title
  return { ...b, title }
}

function sigBadge(e: SkinEntry) {
  return sigBadgeInfo(e.signature, e.sig_detail)
}

/** 可激活 = 包体健康且至少带一种载荷（CSS 换色 ∥ structure 结构 ∥ script 脚本）。 */
function canActivate(e: SkinEntry): boolean {
  return e.status === 'ok' && (!!e.manifest.entry || !!e.manifest.structure || !!e.manifest.script)
}

function applyList(data: ListResp | null) {
  skins.value = data?.skins || []
  dir.value = data?.dir ?? null
  dirExists.value = data?.dir_exists ?? false
  crl.value = data?.crl ?? null
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
    // 裁定 2 刷新边界：live 脚本在场时任何切向 = 整页刷新（JS 状态不可
    // 干净卸载；config 已写好，reload 后 boot 链按新激活 id 装载）。
    if (skinScriptNeedsReload()) {
      location.reload()
      return
    }
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

// ---------------------------------------------------------------------------
// 脚本能力（P2a）：全局开关 + 逐包同意卡
// ---------------------------------------------------------------------------

/** 「允许皮肤携带脚本」全局开关（config `ui.skins.allow_scripts`；默认
 * 关——关 = 服务端 script 端点 403，前端静默纯 CSS）。 */
const allowScripts = ref(false)
const scriptSwitchBusy = ref(false)

/** 待同意卡数据（watch skinState.scriptPending 的投影；entry 取自本
 * 面板已加载的 skins 列表——签名徽标如实展示，含 ⚠/🚫，不拦截）。 */
const pendingId = computed(() => skinState.scriptPending)
const pendingEntry = computed(() => skins.value.find((s) => s.id === pendingId.value) || null)

async function loadAllowScripts() {
  try {
    const cfg = (await request('config', 'get')) as {
      ui?: { skins?: { allow_scripts?: boolean } }
    }
    allowScripts.value = !!cfg?.ui?.skins?.allow_scripts
  } catch {
    // config 不可读 = 闸状态未知 → 保持 false（与服务端 fail-closed 同侧）
  }
}

async function toggleAllowScripts(on: boolean) {
  if (scriptSwitchBusy.value) return
  scriptSwitchBusy.value = true
  try {
    await request('config', 'set_field', { path: 'ui.skins.allow_scripts', value: on })
    allowScripts.value = on
    if (!on) {
      // 裁定 2 规则表「开关关 = JS 不执行」：未裁决的 pending 卡一并废弃
      // ——卡上 bytes-in-hand 不得在闸关后仍可被同意注入。
      discardScriptPending()
      if (skinScriptNeedsReload()) {
        // 裁定 2：运行中关开关且 live 脚本在场 → 整页刷新（JS 不可逆卸载）
        location.reload()
        return
      }
    }
    if (on) {
      // 开闸 = 渐进注入（当前无 live 脚本）：重应用当前皮肤拉脚本
      await applySkinRefresh()
    }
    toast.success(on ? '已允许皮肤携带脚本（逐包仍需同意）' : '已禁止皮肤脚本（纯 CSS 模式）')
  } catch (e: any) {
    toast.error('开关保存失败: ' + e)
    allowScripts.value = !on // 回滚 UI
  }
  scriptSwitchBusy.value = false
}

function decideConsent(allow: boolean) {
  return resolveScriptConsent(allow)
}

// ---------------------------------------------------------------------------
// 下载皮肤（P2 verify-before-install 三入口）
// ---------------------------------------------------------------------------

const dlOpen = ref(false)
const dlBusy = ref(false)
const dlUrl = ref('')
const dlOverwrite = ref(false)
const lastResult = ref<{ entries: InstallOutcome[]; errors: string[] } | null>(null)

/** 单包 InstallOutcome / release 多包 {installed, errors} 统一成结果卡形态 */
function normalizeInstall(resp: any): { entries: InstallOutcome[]; errors: string[] } {
  if (resp && Array.isArray(resp.installed)) {
    const errors = (resp.errors || []).map((e: any) =>
      e?.error ? `${e.file || '包'}：${e.error}` : String(e),
    )
    return { entries: resp.installed as InstallOutcome[], errors }
  }
  return { entries: resp ? [resp as InstallOutcome] : [], errors: [] }
}

async function runInstall(fn: () => Promise<any>) {
  if (dlBusy.value) return
  dlBusy.value = true
  lastResult.value = null
  try {
    lastResult.value = normalizeInstall(await fn())
    if (lastResult.value.entries.length) {
      toast.success(`已安装 ${lastResult.value.entries.length} 个皮肤包`)
    }
    await load()
  } catch (e: any) {
    toast.error('安装失败: ' + e)
  }
  dlBusy.value = false
}

function installOfficial() {
  return runInstall(() =>
    request('skins', 'install', { source: 'release', overwrite: dlOverwrite.value }),
  )
}

function installUrl() {
  const url = dlUrl.value.trim()
  if (!url) return
  return runInstall(() => request('skins', 'install', { url, overwrite: dlOverwrite.value }))
}

async function onFilePicked(ev: Event) {
  const input = ev.target as HTMLInputElement
  const f = input.files?.[0]
  input.value = '' // 允许重选同一文件
  if (!f) return
  return runInstall(async () => {
    const res = await authedFetch(`/api/skins/import${dlOverwrite.value ? '?overwrite=true' : ''}`, {
      method: 'POST',
      headers: { 'content-type': 'application/octet-stream' },
      body: f,
    })
    if (!res.ok) {
      const j = await res.json().catch(() => null)
      throw new Error(j?.message || `HTTP ${res.status}`)
    }
    return res.json()
  })
}

onMounted(() => {
  load()
  loadAllowScripts()
})
</script>

<template>
  <div class="skins-panel">
    <div class="skins-toolbar">
      <div class="skins-dir" :title="dir || ''">
        皮肤目录：<code>{{ dir || '未知' }}</code>
        <span v-if="dir && !dirExists" class="dir-missing">（目录不存在）</span>
      </div>
      <div class="skins-actions">
        <button class="btn" :disabled="busy || loading || dlBusy" @click="reload">🔄 重新加载</button>
        <button
          class="btn"
          :class="{ 'btn-primary': dlOpen }"
          :disabled="loading"
          data-test="dl-toggle"
          @click="dlOpen = !dlOpen"
        >⬇ 下载皮肤</button>
      </div>
    </div>

    <!-- CRL 快照状态（P3：在场才渲染，诚实呈现装载各态） -->
    <div v-if="crl?.present" class="crl-line" :title="crl.note || ''">
      🛡 吊销快照：v{{ crl.version }} · {{ crl.entries }} 条
      <template v-if="crl.verified && !crl.expired">· 已验签生效</template>
      <template v-else-if="crl.expired">· 已过期未应用</template>
      <template v-else>· 验签失败未应用</template>
    </div>

    <!-- 脚本能力开关（P2a）：总闸关 = 服务端 403，前端纯 CSS 静默 -->
    <label class="script-switch" data-test="script-switch">
      <input
        type="checkbox"
        :checked="allowScripts"
        :disabled="scriptSwitchBusy"
        @change="toggleAllowScripts(($event.target as HTMLInputElement).checked)"
      />
      允许皮肤携带脚本
      <span class="script-switch-hint">总闸（默认关）；开闸后每个带脚本包首次运行仍需逐包同意</span>
    </label>

    <!-- 脚本同意卡（P2a）：watch skinState.scriptPending；签名状态如实
         展示（含 ⚠/🚫）但不拦截——开关 + 同意是唯一授权 -->
    <div v-if="pendingId" class="card consent-card" data-test="script-consent">
      <div class="consent-head">
        <b>⚠ 皮肤「{{ displayName(pendingEntry || undefined) || pendingId }}」携带可执行脚本</b>
      </div>
      <p class="consent-body">
        允许后该脚本将随当前页面执行（对页面有完全访问能力）。签名状态：
        <span :class="['sig-badge', sigBadgeInfo(pendingEntry?.signature ?? 'unverified', pendingEntry?.sig_detail).cls]">
          {{ sigBadgeInfo(pendingEntry?.signature ?? 'unverified', pendingEntry?.sig_detail).label }}
        </span>
        <span v-if="pendingEntry" class="dl-hint"> · 包 <code>{{ shortSha(pendingEntry.sha256) }}</code>（包更新后需重新同意）</span>
      </p>
      <div class="consent-actions">
        <button class="btn btn-primary" data-test="script-allow" @click="decideConsent(true)">允许并运行</button>
        <button class="btn" data-test="script-deny" @click="decideConsent(false)">仅用样式，不运行</button>
      </div>
    </div>

    <!-- 下载面板：三入口共用一条验签落盘管线 -->
    <div v-if="dlOpen" class="card dl-panel" data-test="dl-panel">
      <div class="dl-row">
        <button class="btn btn-primary" :disabled="dlBusy" data-test="dl-official" @click="installOfficial">
          🏛 从官方 Release 安装
        </button>
        <span class="dl-hint">拉取最新 Release 的 nightly-skins.zip，逐包验签安装</span>
      </div>
      <div class="dl-row">
        <input
          v-model="dlUrl"
          class="dl-input"
          type="url"
          placeholder="https://…/skin.nbskin"
          :disabled="dlBusy"
          data-test="dl-url"
          @keyup.enter="installUrl"
        />
        <button class="btn" :disabled="dlBusy || !dlUrl.trim()" data-test="dl-url-go" @click="installUrl">从 URL 安装</button>
      </div>
      <div class="dl-row">
        <input type="file" accept=".nbskin" :disabled="dlBusy" data-test="dl-file" @change="onFilePicked" />
        <label class="dl-check">
          <input v-model="dlOverwrite" type="checkbox" :disabled="dlBusy" data-test="dl-overwrite" />
          同名覆盖
        </label>
      </div>
      <p class="dl-note">
        三入口共用同一条管线：验签 → 徽标 → 落盘（所有信任结论都会落盘，
        仅物理损坏的包被拒收）；能否「设为默认观感」由
        <code>ui.skins.require_signed</code> 在启用时把关。
      </p>
    </div>

    <!-- 安装结果卡（复用五态徽标语言） -->
    <div v-if="lastResult" class="card dl-result" data-test="dl-result">
      <div class="dl-result-head">
        <b>安装结果</b>
        <button class="btn btn-sm" data-test="dl-result-close" @click="lastResult = null">✕</button>
      </div>
      <div v-for="(e, i) in lastResult.entries" :key="i" class="dl-result-row">
        <span :class="['sig-badge', sigBadgeInfo(e.signature, e.sig_detail).cls]" :title="sigBadgeInfo(e.signature, e.sig_detail).title">
          {{ sigBadgeInfo(e.signature, e.sig_detail).label }}
        </span>
        <span class="dl-result-id">{{ e.id }}</span>
        <span v-if="e.manifest?.name" class="dl-hint">{{ e.manifest.name }}</span>
        <span v-if="e.manifest?.version" class="dl-hint">v{{ e.manifest.version }}</span>
        <span v-if="e.overwritten" class="dl-hint">（覆盖旧包）</span>
      </div>
      <p v-for="(err, i) in lastResult.errors" :key="'e' + i" class="skin-warn">⚠ {{ err }}</p>
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
            v-if="s.manifest.structure"
            class="skin-variant"
            title="自带声明式 UI 结构（结构皮肤）：换骨架 + 换色"
          >结构</span>
          <span
            v-if="s.has_script"
            class="skin-variant skin-script-tag"
            title="携带脚本载荷：运行需全局开关 + 逐包同意（签名状态只是徽标）"
          >脚本</span>
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
            v-if="canActivate(s)"
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
      带「结构」标的皮肤自带声明式 UI 结构（包内不执行任何代码，宿主引擎
      清洗后渲染）= 换骨架 + 换色；纯 CSS 皮肤 = 原生布局换色。带「脚本」
      标的皮肤携带可执行脚本：需「允许皮肤携带脚本」总闸 + 逐包同意卡
      双授权（签名状态在同意卡如实展示但不拦截），运行中关闭总闸或切换
      皮肤会整页刷新以卸载脚本。
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
.crl-line {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-bottom: var(--space-3);
}
/* 脚本开关 + 同意卡（P2a） */
.script-switch {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-sm);
  margin-bottom: var(--space-3);
  cursor: pointer;
  user-select: none;
}
.script-switch-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
}
.consent-card {
  padding: var(--space-4);
  margin-bottom: var(--space-4);
  border-color: var(--warning);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.consent-body {
  margin: 0;
  font-size: var(--text-sm);
  color: var(--text-secondary);
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}
.consent-actions {
  display: flex;
  gap: var(--space-2);
}
.skin-script-tag {
  color: var(--warning);
  border-color: var(--warning);
}
.skins-empty {
  color: var(--text-muted);
  padding: var(--space-6) 0;
  text-align: center;
}
/* 下载面板（P2 三入口） */
.dl-panel {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  padding: var(--space-4);
  margin-bottom: var(--space-4);
}
.dl-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}
.dl-input {
  flex: 1;
  min-width: 220px;
}
.dl-check {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: var(--text-sm);
  color: var(--text-secondary);
  user-select: none;
}
.dl-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
}
.dl-note {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--text-muted);
}
/* 安装结果卡 */
.dl-result {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  padding: var(--space-4);
  margin-bottom: var(--space-4);
}
.dl-result-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.dl-result-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
  font-size: var(--text-sm);
}
.dl-result-id {
  font-weight: 600;
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
.sig-revoked {
  color: var(--error);
  background: var(--error-bg);
  border: 1px solid var(--error);
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
