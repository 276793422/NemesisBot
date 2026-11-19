<script setup lang="ts">
/**
 * 浏览器 TAB 面板（2026-11-15，计划 D10/D15）。
 *
 * 挂 ExecutorsView 复合页内第四 TAB（VITE_FEATURE_BROWSER 门控；无自身
 * page-header——页面级标题归宿主页，padding 与滚动由宿主 .page-body 提供，
 * 同 SubagentsView/ProcessesView 布局契约）。内含三子页签（五轮拍板 D15，
 * 子页签与父级 TAB 拉开层级）：
 *
 * - 【列表】高频点名面：运行实例卡（kind+序号+状态点+端口+profile+关闭）
 *   + 卡内嵌标签页表（圆点=工作 tab 点选即 attach、加载态徽标、🤖AI操作中
 *   徽标）；右上 [+打开浏览器]；空态居中打开按钮；结构容纳未来多实例。
 *   会话列表 3s 慢拍、展开卡的 tab 列表 2s 跟随。
 * - 【设置】低频配置面：D14 枚举卡片（detect 数据源 + 可用/二期徽标）+
 *   自定义路径 + 设为默认；config.json `browser` 段经 config.set_field
 *   持久化（gateway prefs_getter 每次 spawn 现读，改完即生效）。
 * - 【日志】审计回看面：编排操作时间线（browser.logs 环形缓冲投影）+
 *   SSE browser_event 实时并入；AI 动作路走 tool_event（ToolCallCard），
 *   本页只收编排操作路——两路数据源照常落日志/审计链（计划 D15）。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useToast } from '../composables/useToast'
import { useSSE } from '../composables/useSSE'
import { useBrowserStore, type BrowserSessionSummary } from '../stores/browser'

const toast = useToast()
const { on, off } = useSSE()
const store = useBrowserStore()

type SubTab = 'list' | 'settings' | 'logs'
const subTab = ref<SubTab>('list')

// --- 【列表】 ---
const expandedTabs = ref<Set<string>>(new Set())
const spawning = ref(false)
let sessionTimer: ReturnType<typeof setInterval> | null = null
let tabTimer: ReturnType<typeof setInterval> | null = null

/** 展开卡集合变化 → 重装 tab 跟随定时器（只轮询展开的卡）。 */
watch(expandedTabs, () => armTabTimer(), { deep: true })

function toggleExpand(id: string) {
  const next = new Set(expandedTabs.value)
  if (next.has(id)) next.delete(id)
  else {
    next.add(id)
    // 展开即拉一次（不等 2s 跟随定时器——点名高频面首屏就要有表）。
    void store.fetchTabs(id).catch(() => {})
  }
  expandedTabs.value = next
}

function armSessionTimer() {
  if (sessionTimer) clearInterval(sessionTimer)
  sessionTimer = setInterval(() => void store.fetchSessions(), 3000)
}

function armTabTimer() {
  if (tabTimer) clearInterval(tabTimer)
  const ids = [...expandedTabs.value]
  if (ids.length === 0) {
    tabTimer = null
    return
  }
  tabTimer = setInterval(() => {
    for (const id of ids) {
      // F27（2026-11-15 五审）：死实例跳过轮询（后端 reaper 已置位——拉取
      // 必失败被吞，死卡上 stale tab 行还挂着「加载中」假象）。
      const s = store.sessions.find((x) => x.id === id)
      if (s && s.alive === false) continue
      void store.fetchTabs(id).catch(() => {})
    }
  }, 2000)
}

async function openBrowser(kind?: string) {
  if (spawning.value) return
  spawning.value = true
  try {
    const s = await store.spawn(kind || (store.settings.kind !== 'auto' ? store.settings.kind : undefined))
    toast.success(`已打开浏览器：${s?.kind_name ?? ''}`)
    if (s) {
      const next = new Set(expandedTabs.value)
      next.add(s.id)
      expandedTabs.value = next
      await store.fetchTabs(s.id).catch(() => {})
    }
  } catch (e: any) {
    toast.error(e?.message ?? String(e))
  } finally {
    spawning.value = false
  }
}

async function attachTab(session: BrowserSessionSummary, targetId: string) {
  try {
    await store.attach(session.id, targetId)
    toast.success('已点名该标签页为工作 tab')
  } catch (e: any) {
    toast.error(e?.message ?? String(e))
  }
}

async function releaseTab(session: BrowserSessionSummary) {
  try {
    await store.release(session.id)
    toast.success('已释放工作 tab')
  } catch (e: any) {
    toast.error(e?.message ?? String(e))
  }
}

async function closeBrowser(session: BrowserSessionSummary) {
  if (!confirm(`关闭浏览器 ${session.kind_name}·${session.seq}？（优雅关闭，会话数据落盘）`)) return
  try {
    await store.closeSession(session.id)
    const next = new Set(expandedTabs.value)
    next.delete(session.id)
    expandedTabs.value = next
    toast.success('已关闭')
  } catch (e: any) {
    toast.error(e?.message ?? String(e))
  }
}

function hostOf(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return url.slice(0, 40)
  }
}

// --- 【设置】 ---
const settingsKind = ref('auto')
const settingsExecutable = ref('')
const savingSettings = ref(false)

async function loadSettings() {
  await store.fetchSettings()
  settingsKind.value = store.settings.kind
  settingsExecutable.value = store.settings.executable
  void store.fetchDetected(true)
}

async function saveSettings() {
  if (savingSettings.value) return
  if (settingsKind.value === 'custom' && !settingsExecutable.value.trim()) {
    toast.error('自定义浏览器需填写可执行文件路径')
    return
  }
  savingSettings.value = true
  try {
    await store.saveSettings({
      kind: settingsKind.value,
      executable: settingsExecutable.value.trim(),
    })
    toast.success('已保存（下次打开浏览器生效）')
  } catch (e: any) {
    toast.error(e?.message ?? String(e))
  } finally {
    savingSettings.value = false
  }
}

// --- 【日志】 ---
const logFilterSession = ref('')
let logTimer: ReturnType<typeof setInterval> | null = null

function armLogTimer() {
  if (logTimer) clearInterval(logTimer)
  logTimer = setInterval(() => {
    void store.fetchLogs(logFilterSession.value || undefined)
  }, 4000)
}

const filteredLogs = computed(() =>
  logFilterSession.value
    ? store.logs.filter((l) => l.session_id === logFilterSession.value)
    : store.logs,
)

function fmtTs(tsMs: number): string {
  const d = new Date(tsMs)
  return `${d.getHours().toString().padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}:${d.getSeconds().toString().padStart(2, '0')}`
}

// SSE browser_event：编排操作路实时并入（agent_build.rs sink → EventHub）。
function onBrowserEvent(ev: any) {
  store.ingestEvent(ev)
  if (subTab.value === 'list') void store.fetchSessions()
}

onMounted(() => {
  void store.fetchSessions()
  armSessionTimer()
  on('browser_event', onBrowserEvent)
})

onBeforeUnmount(() => {
  if (sessionTimer) clearInterval(sessionTimer)
  if (tabTimer) clearInterval(tabTimer)
  if (logTimer) clearInterval(logTimer)
  off('browser_event', onBrowserEvent)
})

watch(subTab, (t) => {
  if (t === 'settings') void loadSettings()
  if (t === 'logs') {
    void store.fetchLogs(logFilterSession.value || undefined)
    armLogTimer()
  } else if (logTimer) {
    clearInterval(logTimer)
    logTimer = null
  }
})
</script>

<template>
  <!-- TAB 面板（挂 ExecutorsView 复合页内）：无自身 page-header，
       padding 与滚动由宿主 .page-body 提供（裸 div 仅作结构分组）。 -->
  <div class="page-browser">
    <div class="tabs browser-subtabs">
      <button class="tab" :class="{ active: subTab === 'list' }" data-test="browser-tab-list" @click="subTab = 'list'">列表</button>
      <button class="tab" :class="{ active: subTab === 'settings' }" data-test="browser-tab-settings" @click="subTab = 'settings'">设置</button>
      <button class="tab" :class="{ active: subTab === 'logs' }" data-test="browser-tab-logs" @click="subTab = 'logs'">日志</button>
    </div>

    <!-- 【列表】 -->
    <div v-if="subTab === 'list'" class="browser-list" data-test="browser-list">
      <div class="list-toolbar">
        <span class="list-hint">点选标签页圆点即点名工作 tab——AI 只操作被点名页；未点名时工具诚实报错。</span>
        <span class="list-spacer" />
        <button class="btn btn-primary btn-sm" data-test="browser-open" :disabled="spawning" @click="openBrowser()">
          {{ spawning ? '打开中…' : '+ 打开浏览器' }}
        </button>
      </div>

      <div v-if="store.sessions.length === 0" class="empty-state" data-test="browser-empty">
        <h3>尚无运行中的浏览器</h3>
        <p>点「+ 打开浏览器」启动一个专用实例（独立 profile + 调试端口），之后 AI 经 host_browser 工具在它里面看页面、点页面。</p>
        <button class="btn btn-primary" :disabled="spawning" @click="openBrowser()">打开浏览器</button>
      </div>

      <div v-for="s in store.sessions" :key="s.id" class="card session-card" :class="{ 'session-card-dead': !s.alive }" :data-test="`browser-session-${s.kind}`">
        <div class="session-head" @click="toggleExpand(s.id)">
          <span class="dot" :class="!s.alive ? 'dot-dead' : s.operating ? 'dot-operating' : 'dot-running'" :title="!s.alive ? '浏览器已退出（窗口被手关或崩溃）' : s.operating ? 'AI 操作中' : '运行中'" />
          <span class="session-name">{{ s.kind_name }}·{{ s.seq }}</span>
          <span class="session-meta">127.0.0.1:{{ s.port }}</span>
          <span v-if="!s.alive" class="badge badge-error" data-test="browser-session-dead">已退出</span>
          <span v-if="s.alive && s.operating" class="badge badge-info">🤖 AI操作中</span>
          <span class="list-spacer" />
          <button class="btn btn-sm btn-danger" data-test="browser-close" @click.stop="closeBrowser(s)">关闭</button>
        </div>
        <div class="session-profile" :title="s.profile">profile: {{ s.profile }}</div>

        <div v-if="expandedTabs.has(s.id)" class="tab-table" :data-test="`browser-tabs-${s.kind}`">
          <div class="tab-table-head">
            <span>标签页</span>
            <span class="list-spacer" />
            <button v-if="s.alive && s.working_tab" class="btn btn-sm" @click.stop="releaseTab(s)">释放工作 tab</button>
            <!-- F21：原「再开一个」按钮移除——同 kind 二实例与 profile 单实例
                 架构互斥（profile 按 kind 定死，二实例撞 user-data-dir），
                 此前从未真正可用；开新实例走工具栏「+ 打开浏览器」。 -->
          </div>
          <div v-if="!s.alive" class="tab-empty">浏览器已退出（窗口被手关或崩溃）——点「关闭」清理本卡，或重新打开。</div>
          <template v-else>
            <div v-if="(store.tabsBySession[s.id] || []).length === 0" class="tab-empty">读取中…</div>
            <div
              v-for="t in store.tabsBySession[s.id] || []"
              :key="t.target_id"
              class="tab-row"
              :class="{ 'tab-row-working': t.working }"
              :data-test="`browser-tab-row`"
              @click.stop="attachTab(s, t.target_id)"
            >
              <span class="tab-dot" :class="{ 'tab-dot-working': t.working }" :title="t.working ? '工作 tab（点选切换）' : '点选即点名'" />
              <span class="tab-title" :title="t.title">{{ t.title || '(无标题)' }}</span>
              <span class="tab-host">{{ hostOf(t.url) }}</span>
              <span v-if="t.working && t.operating" class="badge badge-info">🤖 AI操作中</span>
              <span class="tab-state">{{ t.working ? (t.load_state === 'complete' ? '已加载' : '加载中…') : '未点名' }}</span>
            </div>
          </template>
        </div>
      </div>
    </div>

    <!-- 【设置】 -->
    <div v-else-if="subTab === 'settings'" class="browser-settings" data-test="browser-settings">
      <div class="card">
        <div class="card-header"><h3>默认浏览器</h3></div>
        <div class="card-body">
          <p class="settings-hint">
            「自动」= Chrome 检出即优先（默认浏览器偏好信号），否则平台保底（Windows=Edge）。
            换浏览器后首次使用需重新登录（vault 凭据按域名绑定，流程自动兜）。
          </p>
          <div class="kind-grid">
            <button
              class="kind-card"
              :class="{ active: settingsKind === 'auto' }"
              @click="settingsKind = 'auto'"
            >
              <span class="kind-name">自动</span>
              <span class="kind-desc">Chrome 偏好优先，否则平台保底</span>
            </button>
            <button
              v-for="b in store.detected || []"
              :key="b.kind"
              class="kind-card"
              :class="{ active: settingsKind === b.kind }"
              :data-test="`browser-kind-${b.kind}`"
              @click="settingsKind = b.kind"
            >
              <span class="kind-name">{{ b.name }}</span>
              <span class="badge" :class="b.phase2 ? 'badge-neutral' : 'badge-success'">{{ b.phase2 ? '二期' : '可用' }}</span>
              <span class="kind-desc" :title="b.path || ''">{{ b.path || '未检测到' }}</span>
            </button>
            <button
              class="kind-card"
              :class="{ active: settingsKind === 'custom' }"
              data-test="browser-kind-custom"
              @click="settingsKind = 'custom'"
            >
              <span class="kind-name">自定义路径</span>
              <span class="kind-desc">指定可执行文件</span>
            </button>
          </div>
          <div v-if="settingsKind === 'custom'" class="form-group">
            <label class="form-label">可执行文件路径</label>
            <input
              v-model="settingsExecutable"
              class="form-input"
              data-test="browser-custom-path"
              placeholder="C:\Program Files\...\browser.exe"
            />
          </div>
          <div class="settings-actions">
            <button class="btn btn-primary" data-test="browser-save-settings" :disabled="savingSettings" @click="saveSettings">
              {{ savingSettings ? '保存中…' : '保存' }}
            </button>
            <span v-if="settingsKind !== 'auto'" class="settings-default-note">
              当前默认：{{ settingsKind === 'custom' ? `自定义（${settingsExecutable || '未填'}）` : settingsKind }}
            </span>
          </div>
        </div>
      </div>
    </div>

    <!-- 【日志】 -->
    <div v-else class="browser-logs" data-test="browser-logs">
      <div class="list-toolbar">
        <select v-model="logFilterSession" class="form-select" data-test="browser-log-filter" @change="void store.fetchLogs(logFilterSession || undefined)">
          <option value="">全部实例</option>
          <option v-for="s in store.sessions" :key="s.id" :value="s.id">{{ s.kind_name }}·{{ s.seq }}</option>
        </select>
        <span class="list-hint">spawn / attach / 释放 / 关闭等编排操作全留痕；AI 动作在工具事件卡片（对话内）。</span>
      </div>
      <div v-if="filteredLogs.length === 0" class="empty-state"><h3>暂无操作记录</h3><p>打开浏览器或点名标签页后这里会出现时间线。</p></div>
      <div v-for="(l, i) in filteredLogs" :key="`${l.ts_ms}-${i}`" class="log-row" :data-test="`browser-log-row`">
        <span class="log-ts">{{ fmtTs(l.ts_ms) }}</span>
        <span class="log-action">{{ l.action }}</span>
        <span class="log-target" :title="l.target">{{ l.target || '—' }}</span>
        <span class="log-duration" v-if="l.duration_ms">{{ l.duration_ms }}ms</span>
        <span class="badge" :class="l.ok ? 'badge-success' : 'badge-error'">{{ l.ok ? '成功' : '失败' }}</span>
        <span class="log-detail" :title="l.detail">{{ l.detail }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.browser-subtabs {
  font-size: var(--text-xs);
}
.browser-subtabs .tab {
  padding: var(--space-1) var(--space-3);
}

.list-toolbar {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-3);
}
.list-spacer {
  flex: 1;
}
.list-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.session-card {
  margin-bottom: var(--space-3);
}
.session-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  cursor: pointer;
  padding: var(--space-1) 0;
}
.dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}
.dot-running {
  background: var(--success, #22c55e);
}
/* F27：死实例置灰（点灰 + 卡半透明——手关窗口后不再冒充运行中）。 */
.dot-dead {
  background: var(--text-muted, #94a3b8);
}
.session-card-dead {
  opacity: 0.55;
}
.dot-operating {
  background: var(--accent);
  animation: pulse 1.2s ease-in-out infinite;
}
@keyframes pulse {
  50% { opacity: 0.4; }
}
.session-name {
  font-weight: 600;
}
.session-meta {
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-family: var(--font-mono, monospace);
}
.session-profile {
  font-size: var(--text-xs);
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tab-table {
  margin-top: var(--space-2);
  border-top: 1px solid var(--border-light);
  padding-top: var(--space-2);
}
.tab-table-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-xs);
  font-weight: 600;
  color: var(--text-muted);
  margin-bottom: var(--space-1);
}
.tab-empty {
  font-size: var(--text-xs);
  color: var(--text-muted);
  padding: var(--space-2) 0;
}
.tab-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-1) var(--space-2);
  border-radius: var(--radius-sm, 4px);
  cursor: pointer;
  font-size: var(--text-sm);
}
.tab-row:hover {
  background: var(--bg-hover, rgba(128, 128, 128, 0.08));
}
.tab-row-working {
  background: var(--bg-active, rgba(99, 102, 241, 0.08));
}
.tab-dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  border: 1px solid var(--border);
  flex-shrink: 0;
}
.tab-dot-working {
  background: var(--accent);
  border-color: var(--accent);
}
.tab-title {
  max-width: 220px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.tab-host {
  font-size: var(--text-xs);
  color: var(--text-muted);
  font-family: var(--font-mono, monospace);
  max-width: 200px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.tab-state {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-left: auto;
  flex-shrink: 0;
}

.kind-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
  gap: var(--space-2);
  margin: var(--space-3) 0;
}
.kind-card {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: var(--space-1);
  padding: var(--space-3);
  border: 1px solid var(--border);
  border-radius: var(--radius, 6px);
  background: none;
  cursor: pointer;
  text-align: left;
}
.kind-card:hover {
  border-color: var(--accent);
}
.kind-card.active {
  border-color: var(--accent);
  background: var(--bg-active, rgba(99, 102, 241, 0.08));
}
.kind-name {
  font-weight: 600;
}
.kind-desc {
  font-size: var(--text-xs);
  color: var(--text-muted);
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.settings-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
}
.settings-actions {
  display: flex;
  align-items: center;
  gap: var(--space-3);
}
.settings-default-note {
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.log-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-1) 0;
  border-bottom: 1px solid var(--border-light);
  font-size: var(--text-xs);
}
.log-ts {
  font-family: var(--font-mono, monospace);
  color: var(--text-muted);
  flex-shrink: 0;
}
.log-action {
  font-weight: 600;
  flex-shrink: 0;
  min-width: 64px;
}
.log-target {
  font-family: var(--font-mono, monospace);
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex-shrink: 0;
}
.log-duration {
  color: var(--text-muted);
  flex-shrink: 0;
}
.log-detail {
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
