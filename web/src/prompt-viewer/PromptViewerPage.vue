<script setup lang="ts">
// 提示词内容可视化（2026-11-13）：段全文独立只读查看器页。
//
// 数据链：URL ?session=<会话id>&segment=<段key>&ticket=<一次性票据> → 自持
// WS 连接（登录态取同源 localStorage nemesisbot_auth_token）→ WSAPI
// chat.prompt_segment（ticket 单次消费）→ 渲染。
// 一次性页面语义（2026-11-13 UX 收口，用户拍板）：链接打开后 10 秒内有效
// （服务端票据 TTL 单次消费，刷新/重放/转发全失效）；已打开的页面内容
// 驻留内存可持续查看——顶部横幅倒计时提示，倒计时归零转「链接已失效」常驻
// 注记。刷新按钮已移除（刷新必撞已消费票据）。
// 与 meter 同源守护（后端 session_prompt_segment 与 session_context_status 共享
// 同一底层取数/渲染），本页只做展示、零加工：markdown 段走全局消毒闸
// renderMarkdownHtml，json/文本按 kind 分支。段配色复用 prompt-segments.css
// 全局类（与构成条/弹层按钮同一真相源）。
import { ref, computed, nextTick, onUnmounted } from 'vue'
import hljs from 'highlight.js/lib/core'
import javascript from 'highlight.js/lib/languages/javascript'
import typescript from 'highlight.js/lib/languages/typescript'
import python from 'highlight.js/lib/languages/python'
import rust from 'highlight.js/lib/languages/rust'
import bash from 'highlight.js/lib/languages/bash'
import json from 'highlight.js/lib/languages/json'
import xml from 'highlight.js/lib/languages/xml'
import css from 'highlight.js/lib/languages/css'
import sql from 'highlight.js/lib/languages/sql'
import yaml from 'highlight.js/lib/languages/yaml'
import markdown from 'highlight.js/lib/languages/markdown'
import diff from 'highlight.js/lib/languages/diff'
import 'highlight.js/styles/github-dark.min.css'
import { renderMarkdownHtml } from '../utils/markdown'
import { wsUrl } from '../lib/appBase'
import { uuidv4 } from '../lib/uuid'

hljs.registerLanguage('javascript', javascript)
hljs.registerLanguage('typescript', typescript)
hljs.registerLanguage('python', python)
hljs.registerLanguage('rust', rust)
hljs.registerLanguage('bash', bash)
hljs.registerLanguage('json', json)
hljs.registerLanguage('xml', xml)
hljs.registerLanguage('html', xml)
hljs.registerLanguage('css', css)
hljs.registerLanguage('sql', sql)
hljs.registerLanguage('yaml', yaml)
hljs.registerLanguage('markdown', markdown)
hljs.registerLanguage('diff', diff)

interface PartToolCall {
  name: string
  arguments: string
}
interface SegmentPart {
  label: string
  kind: string
  content: string
  tokens: number
  tool_calls?: PartToolCall[]
  image_refs_count?: number
}
interface SegmentResult {
  segment: string
  title: string
  parts: SegmentPart[]
  total_tokens: number
  generated_at: string
}

const SEGMENT_KEYS = ['system', 'tools', 'mcp', 'skill', 'memory', 'summary', 'history'] as const
type SegmentKey = (typeof SEGMENT_KEYS)[number]

const params = new URLSearchParams(window.location.search)
const sessionId = params.get('session') ?? ''
const segmentKey = params.get('segment') ?? ''
const ticket = params.get('ticket') ?? ''
const segmentValid = (SEGMENT_KEYS as readonly string[]).includes(segmentKey)

const state = ref<'loading' | 'error' | 'ready'>('loading')
const errorMsg = ref('')
const result = ref<SegmentResult | null>(null)
const collapsed = ref<Set<number>>(new Set())

// —— 一次性链接倒计时（10s 口径与后端 PROMPT_TICKET_TTL_SECS 同源；前端
// 只是展示，真正的失效由服务端票据单次消费裁决）——
const TICKET_TTL_SECS = 10
const secondsLeft = ref(TICKET_TTL_SECS)
const linkExpired = ref(false)
let countdownTimer: ReturnType<typeof setInterval> | null = null
function startCountdown() {
  countdownTimer = setInterval(() => {
    secondsLeft.value -= 1
    if (secondsLeft.value <= 0) {
      if (countdownTimer) clearInterval(countdownTimer)
      countdownTimer = null
      linkExpired.value = true
    }
  }, 1000)
}
onUnmounted(() => {
  if (countdownTimer) clearInterval(countdownTimer)
})
startCountdown()

const fmtTok = (n: number): string => n.toLocaleString('en-US')

const headerMeta = computed(() => {
  if (!result.value) return ''
  return `session ${sessionId || '—'} · 共 ~${fmtTok(result.value.total_tokens)} tok · 取自 ${result.value.generated_at}`
})

// —— 自持最小 WS 客户端（镜像 useWebSocket 的 URL 构造：desktop 桥接后端
// 绝对地址分支优先，其余走 wsUrl 的 <base> 感知路径）——
let ws: WebSocket | null = null
let reqTimer: ReturnType<typeof setTimeout> | null = null

function buildBackendWsUrl(): string {
  if (window.__DASHBOARD_BACKEND__) {
    // desktop（wry）形态：注入的绝对 backend 地址（useWebSocket 同款分支）。
    return 'ws://' + window.__DASHBOARD_BACKEND__ + '/ws'
  }
  return wsUrl('/ws')
}

function fail(msg: string) {
  state.value = 'error'
  errorMsg.value = msg
  cleanup()
}

function cleanup() {
  if (reqTimer) {
    clearTimeout(reqTimer)
    reqTimer = null
  }
  if (ws) {
    // 置空回调防 onclose 分支在主动清理后误报错
    ws.onopen = null
    ws.onmessage = null
    ws.onerror = null
    ws.onclose = null
    try {
      ws.close()
    } catch {
      /* 已断开 */
    }
    ws = null
  }
}

// 渲染容器（模板 ref）——markdown 事后 DOM 高亮的查询根
const bodyEl = ref<HTMLElement | null>(null)

// 渲染后对 markdown 部分的代码块跑 hljs（事后 DOM 路径，ChatPanel
// renderCodeBlocks 同款；renderMarkdownHtml 是消毒闸，高亮在其后）。
async function highlightCodeBlocks() {
  await nextTick()
  if (!bodyEl.value) return
  bodyEl.value.querySelectorAll('.pv-md pre code:not(.hljs)').forEach((block) => {
    try {
      hljs.highlightElement(block as HTMLElement)
    } catch {
      /* 未知语言等高亮失败保持原样 */
    }
  })
}

function jsonHighlighted(src: string): string {
  try {
    return hljs.highlight(src, { language: 'json' }).value
  } catch {
    return ''
  }
}

// 折叠切换：整体换新 Set（ref 重赋值必然触发），不依赖响应式 Set 的
// 原地方法追踪语义。
function toggleCollapse(i: number) {
  const next = new Set(collapsed.value)
  if (next.has(i)) {
    next.delete(i)
  } else {
    next.add(i)
  }
  collapsed.value = next
}

function fetchSegment() {
  cleanup()
  collapsed.value = new Set()
  result.value = null
  if (!sessionId || !segmentKey) {
    fail('缺少参数：URL 需携带 ?session=<会话id>&segment=<段key>&ticket=<一次性票据>（system / tools / mcp / skill / memory / summary / history）')
    return
  }
  if (!segmentValid) {
    fail(`未知段 key：${segmentKey}（合法值：${SEGMENT_KEYS.join(' / ')}）`)
    return
  }
  const token = localStorage.getItem('nemesisbot_auth_token') ?? ''
  if (!token) {
    fail('未检测到登录凭据（nemesisbot_auth_token）：请先登录 Dashboard（本页与 Dashboard 同源共享登录态），再从构成条弹层的段按钮打开本页')
    return
  }
  if (!ticket) {
    fail('缺少一次性访问票据（ticket）：本页为一次性页面，请从 Dashboard 构成条弹层的段按钮打开（直接粘贴的链接不再可用）')
    return
  }
  state.value = 'loading'
  errorMsg.value = ''
  let sock: WebSocket
  try {
    sock = new WebSocket(buildBackendWsUrl() + '?token=' + encodeURIComponent(token))
  } catch (e) {
    fail('WebSocket 创建失败：' + String(e))
    return
  }
  ws = sock
  const reqId = uuidv4()
  // 30s 与 dashboard REQUEST_TIMEOUT 同口径
  reqTimer = setTimeout(() => {
    if (state.value === 'loading') fail('请求超时（30s）：网关未响应 chat.prompt_segment')
  }, 30000)
  sock.onopen = () => {
    sock.send(
      JSON.stringify({
        type: 'request',
        module: 'chat',
        cmd: 'prompt_segment',
        reqId,
        data: { session_id: sessionId, segment: segmentKey, ticket },
      }),
    )
  }
  sock.onmessage = (ev: MessageEvent) => {
    let frame: { type?: string; reqId?: string; data?: unknown; error?: unknown }
    try {
      frame = JSON.parse(typeof ev.data === 'string' ? ev.data : '')
    } catch {
      return
    }
    if (frame.type !== 'response' || frame.reqId !== reqId) return
    if (frame.error) {
      fail('后端错误：' + String(frame.error))
      return
    }
    const seg = (frame.data as { prompt_segment?: SegmentResult } | undefined)?.prompt_segment
    if (!seg || !Array.isArray(seg.parts)) {
      fail('响应形态异常：缺少 prompt_segment.parts')
      return
    }
    result.value = seg
    state.value = 'ready'
    cleanup()
    void highlightCodeBlocks()
  }
  sock.onerror = () => {
    if (state.value === 'loading') fail('WebSocket 连接失败：网关不可达或凭据无效')
  }
  sock.onclose = () => {
    if (state.value === 'loading') fail('连接已关闭：网关不可达或凭据无效')
  }
}

// 首次进入即取数
fetchSegment()
</script>

<template>
  <div class="pv-page">
    <header class="pv-header">
      <div class="pv-header-row">
        <div class="pv-title-block">
          <span
            v-if="segmentValid"
            class="pv-chip"
            :class="`usage-seg-${segmentKey}`"
          ></span>
          <h1 class="pv-title">{{ result?.title ?? '提示词查看器' }}</h1>
          <span v-if="result" class="pv-meta">{{ headerMeta }}</span>
        </div>
      </div>
      <!-- 一次性链接倒计时（2026-11-13 UX 收口）：展示面；真正的链接失效由
           服务端票据单次消费裁决（刷新/重放后端拒绝）。错误态不显横幅。 -->
      <div v-if="state !== 'error'" class="pv-timer-bar" :class="{ 'pv-timer-expired': linkExpired }">
        <template v-if="!linkExpired">
          <span class="pv-timer-num">{{ secondsLeft }}</span>
          <span>秒后，链接不可访问（页面不刷新、关闭则可持续查看）</span>
        </template>
        <template v-else>
          <span>🔒 链接已失效（一次性页面）：当前内容仍可持续查看；刷新或关闭后需从 Dashboard 构成条重新打开</span>
        </template>
      </div>
    </header>

    <main class="pv-body-wrap">
      <div v-if="state === 'loading'" class="pv-note pv-loading">
        <span class="spinner"></span> 正在拉取「{{ segmentKey }}」段全文…
      </div>

      <div v-else-if="state === 'error'" class="pv-error-card">
        <div class="pv-error-title">⚠ 无法显示提示词段</div>
        <div class="pv-error-msg">{{ errorMsg }}</div>
        <div class="pv-error-hint">
          打开方式：在 Dashboard 聊天输入框上方的 context 构成条上悬停，点击弹层里的段色按钮。
        </div>
      </div>

      <div v-else-if="result" ref="bodyEl" class="pv-body">
        <p v-if="result.parts.length === 0" class="pv-note">
          本段当前为空（该段在本次请求中不占 token）。
        </p>
        <section
          v-for="(p, i) in result.parts"
          :key="i"
          class="pv-part"
        >
          <header
            class="pv-part-head"
            :title="collapsed.has(i) ? '点击展开' : '点击折叠'"
            @click="toggleCollapse(i)"
          >
            <span class="pv-caret">{{ collapsed.has(i) ? '▸' : '▾' }}</span>
            <span class="pv-part-label">{{ p.label }}</span>
            <span class="pv-part-kind">{{ p.kind }}</span>
            <span class="pv-part-tokens">~{{ fmtTok(p.tokens) }} tok</span>
            <span v-if="p.image_refs_count" class="pv-part-aux">🖼 ×{{ p.image_refs_count }}（只计数注记，像素不入上下文估算）</span>
            <span v-if="p.tool_calls?.length" class="pv-part-aux">tool_calls ×{{ p.tool_calls.length }}</span>
          </header>
          <div v-show="!collapsed.has(i)" class="pv-part-body">
            <!-- tool 轮旁注：assistant 轮的调用清单（名称 + 参数高亮） -->
            <div v-if="p.tool_calls?.length" class="pv-toolcalls">
              <div v-for="(c, ci) in p.tool_calls" :key="ci" class="pv-toolcall">
                <span class="pv-toolcall-name">{{ c.name }}</span>
                <pre class="pv-code"><code class="hljs language-json" v-html="jsonHighlighted(c.arguments)"></code></pre>
              </div>
            </div>
            <div
              v-if="p.kind === 'markdown'"
              class="pv-md markdown-body"
              v-html="renderMarkdownHtml(p.content)"
            ></div>
            <pre v-else-if="p.kind === 'json'" class="pv-code"><code class="hljs language-json" v-html="jsonHighlighted(p.content)"></code></pre>
            <pre v-else class="pv-plain">{{ p.content }}</pre>
          </div>
        </section>

        <footer class="pv-foot">
          token 数为估算口径（与聊天页 context 构成条同源同公式，非 provider 实报）；
          每轮请求前注入的临时 system 消息（当前时间 / 运行环境）为运行期动态内容，不在本查看器内。
        </footer>
      </div>
    </main>
  </div>
</template>

<style scoped>
.pv-page {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
  background: var(--bg);
  color: var(--text-primary);
}
.pv-header {
  position: sticky;
  top: 0;
  z-index: 5;
  display: flex;
  flex-direction: column;
  gap: 5px;
  padding: 10px 16px 8px;
  background: var(--surface);
  border-bottom: 1px solid var(--border);
}
.pv-header-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.pv-timer-bar {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-secondary);
}
.pv-timer-num {
  min-width: 1.4em;
  text-align: center;
  font-weight: 700;
  font-size: 14px;
  font-variant-numeric: tabular-nums;
  color: var(--warning, #d9a441);
}
.pv-timer-bar.pv-timer-expired {
  color: var(--text-muted);
}
.pv-title-block {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}
.pv-chip {
  width: 12px;
  height: 12px;
  border-radius: 3px;
  flex-shrink: 0;
}
.pv-title {
  font-size: 15px;
  font-weight: 600;
  margin: 0;
  white-space: nowrap;
}
.pv-meta {
  color: var(--text-muted);
  font-size: 12px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pv-body-wrap {
  flex: 1;
  width: 100%;
  max-width: 980px;
  margin: 0 auto;
  padding: 16px;
  box-sizing: border-box;
}
.pv-loading {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--text-secondary);
}
.spinner {
  width: 16px;
  height: 16px;
  border: 2px solid var(--border);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: pv-spin 0.8s linear infinite;
  display: inline-block;
}
@keyframes pv-spin {
  to {
    transform: rotate(360deg);
  }
}
.pv-note {
  color: var(--text-muted);
  font-size: 13px;
}
.pv-error-card {
  margin-top: 32px;
  padding: 20px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface);
}
.pv-error-title {
  font-size: 14px;
  font-weight: 600;
  margin-bottom: 8px;
}
.pv-error-msg {
  color: var(--text-secondary);
  font-size: 13px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-word;
}
.pv-error-hint {
  margin-top: 12px;
  color: var(--text-muted);
  font-size: 12px;
}
.pv-body {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.pv-part {
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface);
  overflow: hidden;
}
.pv-part-head {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  background: var(--surface-hover);
  cursor: pointer;
  user-select: none;
  flex-wrap: wrap;
}
.pv-caret {
  color: var(--text-muted);
  font-size: 11px;
  width: 12px;
}
.pv-part-label {
  font-weight: 600;
  font-size: 13px;
}
.pv-part-kind {
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 4px;
  background: var(--surface-active, var(--bg));
  color: var(--text-secondary);
}
.pv-part-tokens {
  margin-left: auto;
  color: var(--text-muted);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}
.pv-part-aux {
  font-size: 11px;
  color: var(--text-muted);
}
.pv-part-body {
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.pv-toolcalls {
  display: flex;
  flex-direction: column;
  gap: 6px;
  border-left: 3px solid var(--border);
  padding-left: 10px;
}
.pv-toolcall-name {
  font-size: 12px;
  color: var(--text-secondary);
  font-weight: 600;
}
.pv-code {
  margin: 0;
  padding: 8px 10px;
  border-radius: 6px;
  overflow-x: auto;
  font-size: 12px;
  line-height: 1.5;
  background: #0d1117; /* github-dark 底色，与 hljs 主题一致（两主题下代码块统一暗底，ChatPanel 同款约定） */
}
.pv-plain {
  margin: 0;
  padding: 8px 10px;
  border-radius: 6px;
  overflow-x: auto;
  font-size: 12px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
  background: var(--surface-hover);
  color: var(--text-primary);
}
.pv-md {
  font-size: 13px;
}
.pv-md :deep(.md-table-wrap) {
  overflow-x: auto;
}
.pv-foot {
  color: var(--text-muted);
  font-size: 12px;
  line-height: 1.6;
  padding: 4px 2px 16px;
}
</style>

<style>
/* 滚动条修复（2026-11-13）：theme.css 的 body { overflow: hidden } 是
   dashboard 全局布局「自管滚动容器」的语义；查看器独立页导入同一 theme
   会被连带吞掉文档滚动——段内容超长时页面纹丝不动。本页恢复文档流滚动
   （sticky header 在文档滚动下照常吸顶）。非 scoped 块：scoped 选择器
   够不到 body/html。全局选择器只在本 MPA 入口生效，不污染 dashboard。 */
html,
body {
  overflow: auto;
}
</style>
