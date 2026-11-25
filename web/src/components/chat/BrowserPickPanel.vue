<script setup lang="ts">
/**
 * BrowserPickPanel — ChatPanel 侧「浏览器点名面板」（2026-11-15，计划 D10）。
 *
 * 复刻 FileTreePanel 面板形态（ChatView `.chat-page-layout` 左列，开合走
 * useBrowserPanel 单例——入口在会话侧栏顶部功能区的「浏览器」按钮）。
 * 职责收窄为**纯点名器**：运行实例状态 + 标签页可点选（点名即授权——AI
 * 只看被点名 tab）；管理/配置/审计归执行单元页【浏览器】TAB。点名发生在
 * 对话中途，跳页点名反人类，故此面板常驻可开。
 *
 * 2026-11-25 批 A+B（死实例堵死新建入口修复）：
 * - A：footer 常驻「打开浏览器」（仅非空态渲染——空态保留居中 CTA）；
 * - B：死实例卡「↻ 重新打开」= 同 kind 重开（F16 闸防同 kind 重复）。
 */
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useToast } from '../../composables/useToast'
import { useBrowserPanel } from '../../composables/useBrowserPanel'
import { useBrowserStore, type BrowserSessionSummary } from '../../stores/browser'

const toast = useToast()
const { open, setOpen } = useBrowserPanel()
const store = useBrowserStore()

/** 启动防抖：footer 常驻按钮 / 死实例「重新打开」/ 空态按钮共享一把闸。 */
const spawning = ref(false)

let sessionTimer: ReturnType<typeof setInterval> | null = null
let tabTimer: ReturnType<typeof setInterval> | null = null

function armTimers() {
  if (sessionTimer) clearInterval(sessionTimer)
  if (tabTimer) clearInterval(tabTimer)
  sessionTimer = setInterval(() => void store.fetchSessions(), 3000)
  // F26（2026-11-15 四审）：tab 跟随刷**全部**实例——面板按全部会话渲染，
  // 只刷 sessions[0] 时第二实例起的 tabs 永远停在「读取标签页中…」（多
  // kind 实例并存是合法形态，profile 单实例闸只限同 kind）。
  // F27（2026-11-15 五审）：死实例跳过（手关窗口后拉取必失败被吞，死实例
  // 行恒「读取标签页中…」假象——改渲染「已退出」态）。
  tabTimer = setInterval(() => {
    for (const s of store.sessions) {
      if (s.alive === false) continue
      void store.fetchTabs(s.id).catch(() => {})
    }
  }, 2000)
}

/** 开面板首拉：先会话后 tab（串行——tab 目标取决于会话列表），全活实例。 */
async function refreshAll() {
  await store.fetchSessions()
  for (const s of store.sessions) {
    if (s.alive === false) continue
    void store.fetchTabs(s.id).catch(() => {})
  }
}

watch(open, (v) => {
  if (v) {
    void refreshAll()
    armTimers()
  } else {
    if (sessionTimer) clearInterval(sessionTimer)
    if (tabTimer) clearInterval(tabTimer)
    sessionTimer = null
    tabTimer = null
  }
})

// 挂载时面板已开（面板未挂载窗口内侧栏点了开关——ChatView 懒挂载/路由切换
// 窗口）：watch 不触发（值无变化），这里补首拉 + 定时器，语义与 watch 一致。
onMounted(() => {
  if (open.value) {
    void refreshAll()
    armTimers()
  }
})

/**
 * 统一启动入口（2026-11-25 批 A+B）：
 * - A：footer 常驻「打开浏览器」——旧实现按钮只在空态分支，而死实例
 *   （已退出）保留注册表使空态永不达成 = 面板永无新建入口；
 * - B：死实例卡「重新打开」= 同 kind 重开（后端 F16 闸只挡同 kind 活体，
 *   死会话不挡道；同 kind 活体在场时诚实报错走 toast）。
 * kind 缺省 = 后端解析序（配置 kind > auto）。
 */
async function spawnBrowser(kind?: string) {
  if (spawning.value) return
  spawning.value = true
  try {
    const s = await store.spawn(kind)
    toast.success(`已打开浏览器：${s?.kind_name ?? ''}`)
    await store.fetchTabs(s!.id).catch(() => {})
  } catch (e: any) {
    toast.error(e?.message ?? String(e))
  } finally {
    spawning.value = false
  }
}
const openBrowser = () => spawnBrowser()
const reopenSession = (s: BrowserSessionSummary) => spawnBrowser(s.kind)

async function attachTab(session: BrowserSessionSummary, targetId: string) {
  try {
    await store.attach(session.id, targetId)
    toast.success('已点名该标签页——现在可以对话里让 AI 操作它')
  } catch (e: any) {
    toast.error(e?.message ?? String(e))
  }
}

function hostOf(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return url.slice(0, 32)
  }
}

onBeforeUnmount(() => {
  if (sessionTimer) clearInterval(sessionTimer)
  if (tabTimer) clearInterval(tabTimer)
})
</script>

<template>
  <div v-if="open" class="browserpick-wrap" data-test="browser-pick-panel">
    <div class="browserpick-panel">
      <div class="browserpick-header">
        <span class="browserpick-title">🌐 浏览器</span>
        <span class="browserpick-spacer" />
        <button class="browserpick-btn" type="button" title="刷新实例与标签页" @click="() => { void refreshAll() }">⟳</button>
        <button class="browserpick-btn" type="button" title="收起（也可用侧栏「浏览器」按钮）" @click="setOpen(false)">⟨</button>
      </div>

      <div class="browserpick-hint">点选标签页即点名工作 tab——AI 只操作被点名页。管理与日志在「执行单元 → 浏览器」。</div>

      <div v-if="store.sessions.length === 0" class="browserpick-empty">
        <p>尚无运行中的浏览器实例。</p>
        <button class="btn btn-primary btn-sm" @click="openBrowser">打开浏览器</button>
      </div>

      <div v-else class="browserpick-body">
        <div v-for="s in store.sessions" :key="s.id" class="browserpick-session" :class="{ dead: !s.alive }">
          <div class="browserpick-session-head">
            <span class="browserpick-dot" :class="{ operating: s.alive && s.operating, dead: !s.alive }" />
            <span class="browserpick-session-name">{{ s.kind_name }}·{{ s.seq }}</span>
            <span v-if="!s.alive" class="browserpick-deadtag">已退出</span>
            <span v-else-if="s.operating" class="badge badge-info">🤖</span>
          </div>
          <template v-if="!s.alive">
            <div class="browserpick-notabs">浏览器已退出（窗口被手关或崩溃）</div>
            <button
              class="browserpick-reopen"
              type="button"
              data-test="browser-pick-reopen"
              :disabled="spawning"
              title="以同类型重新启动一个实例"
              @click="reopenSession(s)"
            >↻ 重新打开</button>
          </template>
          <template v-else>
            <div
              v-for="t in store.tabsBySession[s.id] || []"
              :key="t.target_id"
              class="browserpick-row"
              :class="{ working: t.working }"
              :title="t.working ? '工作 tab（点其他行切换）' : '点选即点名'"
              @click="attachTab(s, t.target_id)"
            >
              <span class="browserpick-tabdot" :class="{ on: t.working }" />
              <span class="browserpick-tabtitle">{{ t.title || '(无标题)' }}</span>
              <span class="browserpick-tabhost">{{ hostOf(t.url) }}</span>
            </div>
            <div v-if="!(store.tabsBySession[s.id] || []).length" class="browserpick-notabs">读取标签页中…</div>
          </template>
        </div>
      </div>

      <div v-if="store.sessions.length > 0" class="browserpick-footer">
        <button
          class="btn btn-primary btn-sm"
          type="button"
          data-test="browser-pick-open"
          :disabled="spawning"
          @click="openBrowser"
        >{{ spawning ? '打开中…' : '＋ 打开浏览器' }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.browserpick-wrap {
  display: flex;
  min-width: 0;
  min-height: 0;
}
.browserpick-panel {
  display: flex;
  flex-direction: column;
  width: 240px;
  min-height: 0;
  border-right: 1px solid var(--border);
  background: var(--bg-elev);
  overflow: hidden;
}
.browserpick-header {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 6px 8px;
  border-bottom: 1px solid var(--border);
  font-size: var(--text-xs, 12px);
  color: var(--text-muted);
}
.browserpick-title {
  font-weight: 600;
}
.browserpick-spacer {
  flex: 1;
}
.browserpick-btn {
  border: none;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 12px;
  padding: 2px 4px;
}
.browserpick-btn:hover {
  color: var(--text);
}
.browserpick-hint {
  font-size: 11px;
  color: var(--text-muted);
  padding: 6px 8px;
  border-bottom: 1px solid var(--border);
  line-height: 1.5;
}
.browserpick-empty {
  padding: var(--space-4, 16px) 8px;
  text-align: center;
  font-size: var(--text-xs, 12px);
  color: var(--text-muted);
  display: flex;
  flex-direction: column;
  gap: 8px;
  align-items: center;
}
.browserpick-body {
  flex: 1;
  overflow-y: auto;
  padding: 4px 0;
}
.browserpick-session {
  margin-bottom: 8px;
}
.browserpick-session-head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 8px;
  font-size: var(--text-xs, 12px);
}
.browserpick-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--success, #22c55e);
  flex-shrink: 0;
}
.browserpick-dot.operating {
  background: var(--accent);
  animation: bp-pulse 1.2s ease-in-out infinite;
}
/* F27：死实例置灰（手关窗口后不冒充运行中）。 */
.browserpick-dot.dead {
  background: var(--text-muted, #94a3b8);
}
.browserpick-session.dead {
  opacity: 0.55;
}
.browserpick-deadtag {
  font-size: 10px;
  color: var(--danger, #ef4444);
}
@keyframes bp-pulse {
  50% { opacity: 0.4; }
}
.browserpick-session-name {
  font-weight: 600;
}
.browserpick-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 3px 8px 3px 16px;
  cursor: pointer;
  font-size: 12px;
}
.browserpick-row:hover {
  background: var(--bg-hover, rgba(128, 128, 128, 0.08));
}
.browserpick-row.working {
  background: var(--bg-active, rgba(99, 102, 241, 0.08));
}
.browserpick-tabdot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  border: 1px solid var(--border);
  flex-shrink: 0;
}
.browserpick-tabdot.on {
  background: var(--accent);
  border-color: var(--accent);
}
.browserpick-tabtitle {
  max-width: 110px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.browserpick-tabhost {
  font-size: 10px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.browserpick-notabs {
  font-size: 11px;
  color: var(--text-muted);
  padding: 2px 8px 2px 16px;
}
/* A：footer 常驻启动（仅非空态渲染——空态有自己的居中 CTA，避免双按钮）。 */
.browserpick-footer {
  border-top: 1px solid var(--border);
  padding: 8px;
  display: flex;
  justify-content: center;
  flex-shrink: 0;
}
/* B：死实例卡「重新打开」幽灵小按钮。 */
.browserpick-reopen {
  margin: 2px 8px 6px 16px;
  border: 1px solid var(--border);
  background: transparent;
  color: var(--text-muted);
  border-radius: 4px;
  font-size: 11px;
  padding: 2px 8px;
  cursor: pointer;
}
.browserpick-reopen:hover {
  color: var(--text);
  border-color: var(--text-muted);
}
.browserpick-reopen:disabled {
  opacity: 0.5;
  cursor: default;
}
</style>
