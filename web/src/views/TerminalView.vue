<script setup lang="ts">
// L8（devtool-upgrade 阶段 7）：PTY 内嵌终端（dashboard 终端 tab）。
//
// 后端：`/ws/pty?token=`（nemesis-web/src/pty.rs，`terminal` feature +
// config `terminal.enabled` 双闸）。协议：Binary 帧 = PTY 原始字节；
// Text 帧 = control JSON（本页只发 resize；ping/pong 心跳可选）。
//
// 诚实边界（与后端模块头一致）：交互 shell 无法逐命令审批，信任级 =
// dashboard 登录用户；全部 I/O 审计落 `<workspace>/logs/terminal/`，
// **含回显的密码类输入**。
import { ref, onMounted, onUnmounted, nextTick } from 'vue'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import '@xterm/xterm/css/xterm.css'

const WELCOME =
  'NemesisBot 终端\r\n' +
  '信任级：dashboard 登录用户（交互 shell 不逐命令审批）。\r\n' +
  '全部输入/输出已审计（含回显的密码类输入）。\r\n\r\n'

const container = ref<HTMLDivElement | null>(null)
const status = ref<'disconnected' | 'connecting' | 'connected'>('disconnected')

let ws: WebSocket | null = null
let term: Terminal | null = null
let fit: FitAddon | null = null
let resizeObserver: ResizeObserver | null = null
let disposed = false

function ptyUrl(): string {
  const token = localStorage.getItem('nemesisbot_auth_token') ?? ''
  const backend = (window as any).__DASHBOARD_BACKEND__
  const base = backend
    ? 'ws://' + backend + '/ws/pty'
    : (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws/pty'
  const qs = token ? '?token=' + encodeURIComponent(token) : ''
  return base + qs
}

function sendResize() {
  if (!ws || ws.readyState !== WebSocket.OPEN || !fit || !term) return
  try { fit.fit() } catch { /* 容器不可见时 fit 抛错，忽略 */ }
  ws.send(JSON.stringify({ type: 'resize', cols: term.cols, rows: term.rows }))
}

function connect() {
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) return
  status.value = 'connecting'
  ws = new WebSocket(ptyUrl())
  ws.binaryType = 'arraybuffer'

  ws.onopen = () => {
    status.value = 'connected'
    term?.writeln('')
    sendResize()
  }

  ws.onmessage = (ev) => {
    if (typeof ev.data === 'string') {
      // control JSON（pong 等）——本页只关心展示，忽略未知控制帧
      return
    }
    term?.write(new Uint8Array(ev.data))
  }

  ws.onclose = () => {
    status.value = 'disconnected'
    term?.write('\r\n\x1b[90m[连接已断开]\x1b[0m\r\n')
  }

  ws.onerror = () => {
    status.value = 'disconnected'
  }
}

function disconnect() {
  if (ws) { ws.close(); ws = null }
  status.value = 'disconnected'
}

function clearScreen() {
  term?.clear()
  term?.write('\x1b[H\x1b[2J')
}

onMounted(async () => {
  await nextTick()
  if (!container.value || disposed) return
  term = new Terminal({
    cursorBlink: true,
    fontSize: 13,
    fontFamily: 'Consolas, "Courier New", monospace',
    theme: { background: '#141821', foreground: '#d4d4d4' },
    scrollback: 5000,
  })
  fit = new FitAddon()
  term.loadAddon(fit)
  term.open(container.value)
  term.writeln(WELCOME.replace(/\r\n/g, '\r\n'))
  term.onData((data) => {
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(new TextEncoder().encode(data))
    }
  })
  // 容器尺寸变化 → fit + 通知 PTY resize
  resizeObserver = new ResizeObserver(() => sendResize())
  resizeObserver.observe(container.value)
  connect()
})

onUnmounted(() => {
  disposed = true
  resizeObserver?.disconnect()
  resizeObserver = null
  disconnect()
  term?.dispose()
  term = null
})
</script>

<template>
  <div class="terminal-view">
    <div class="terminal-toolbar">
      <div class="terminal-title">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M4 17l6-6-6-6M12 19h8" />
        </svg>
        <span>终端</span>
        <span class="terminal-status" :class="status">
          {{ status === 'connected' ? '已连接' : status === 'connecting' ? '连接中…' : '未连接' }}
        </span>
      </div>
      <div class="terminal-actions">
        <button v-if="status !== 'connected'" class="terminal-btn" @click="connect">连接</button>
        <button v-else class="terminal-btn" @click="disconnect">断开</button>
        <button class="terminal-btn" @click="clearScreen">清屏</button>
      </div>
    </div>
    <div ref="container" class="terminal-container"></div>
    <div class="terminal-footer">
      信任级：dashboard 登录用户（交互 shell 不逐命令审批）· 全部 I/O 审计于 workspace/logs/terminal/（含回显的密码类输入）
    </div>
  </div>
</template>

<style scoped>
.terminal-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}
.terminal-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border-color, #2a2f3a);
  flex-shrink: 0;
}
.terminal-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-weight: 600;
}
.terminal-status {
  font-size: 12px;
  font-weight: 400;
  padding: 2px 8px;
  border-radius: 10px;
  background: var(--bg-tertiary, #1d222c);
  color: var(--text-secondary, #8b93a3);
}
.terminal-status.connected { color: #2ecc71; }
.terminal-status.connecting { color: #f39c12; }
.terminal-actions { display: flex; gap: 8px; }
.terminal-btn {
  padding: 4px 12px;
  font-size: 12px;
  border: 1px solid var(--border-color, #2a2f3a);
  border-radius: 6px;
  background: transparent;
  color: var(--text-primary, inherit);
  cursor: pointer;
}
.terminal-btn:hover { background: var(--bg-tertiary, #1d222c); }
.terminal-container {
  flex: 1;
  min-height: 0;
  padding: 6px 8px;
  background: #141821;
}
.terminal-footer {
  flex-shrink: 0;
  padding: 6px 12px;
  font-size: 11px;
  color: var(--text-secondary, #8b93a3);
  border-top: 1px solid var(--border-color, #2a2f3a);
}
</style>
