<script setup lang="ts">
// P5-R2 ④：VM 屏幕查看 modal（noVNC 动态 import —— VNC 前端 ~200KB
// 不进主 bundle，打开本 modal 才拉 chunk；体积纪律与后端三线一致：
// shim feature 门控 + plugin-vm 独立 cdylib + 本组件动态 import）。
//
// 连接链：noVNC RFB → WS /ws/vm-display（网关 shim，token+agent+mode
// query）→ 帧中继 → plugin-vm（ro 模式丢 RFB 写类消息）→ QEMU VNC。
// 服务端 readonly 是权威闸（插件 per-session 过滤）；前端 viewOnly 只是
// 少发写消息的体验层（ belts: 两层都开）。
import { ref, onMounted, onBeforeUnmount, nextTick } from 'vue'
import { useAuthStore } from '../../stores/auth'
import { wsUrl } from '../../lib/appBase'

const props = defineProps<{
  /** 目标 agent id（VM 实例；空串 = 主实例缺省）。 */
  agentId: string
}>()

const emit = defineEmits<{ (e: 'close'): void }>()

const container = ref<HTMLDivElement | null>(null)
const status = ref<'connecting' | 'connected' | 'disconnected'>('connecting')
const errorMsg = ref('')
/** 观察档（ro，缺省）/ 操作档（rw）——切换即按新档重连。 */
const mode = ref<'ro' | 'rw'>('ro')

type RfbLike = {
  disconnect(): void
  focus(options?: { preventScroll?: boolean }): void
  scaleViewport: boolean
  resizeSession: boolean
  viewOnly: boolean
  addEventListener(type: string, listener: (ev: { detail?: unknown }) => void): void
}
let rfb: RfbLike | null = null
let disposed = false

function displayUrl(): string {
  // token 真相源 = auth store 内存 token（与 /ws/pty 同款，localStorage 兜底）。
  const auth = useAuthStore()
  const token = auth.token || localStorage.getItem('nemesisbot_auth_token') || ''
  const backend = (window as any).__DASHBOARD_BACKEND__
  const base = backend
    ? 'ws://' + backend + '/ws/vm-display'
    : wsUrl('/ws/vm-display')
  const qs = new URLSearchParams()
  if (token) qs.set('token', token)
  if (props.agentId) qs.set('agent', props.agentId)
  qs.set('mode', mode.value)
  return base + '?' + qs.toString()
}

async function connect() {
  status.value = 'connecting'
  errorMsg.value = ''
  await nextTick()
  if (!container.value || disposed) return
  try {
    // 动态 import：本 modal 打开才加载 noVNC chunk。
    const mod = await import('@novnc/novnc')
    if (disposed || !container.value) return
    const RFB = mod.default
    const client = new RFB(container.value, displayUrl(), { shared: true })
    client.scaleViewport = true
    // resizeSession（SetDesktopSize）属写语义——只 ro 档不请求改分辨率
    // （插件层 type 过滤不拦 251 族；前端不发送才是权威）。
    client.resizeSession = mode.value === 'rw'
    client.viewOnly = mode.value === 'ro'
    client.addEventListener('connect', () => {
      status.value = 'connected'
    })
    client.addEventListener('disconnect', (ev) => {
      status.value = 'disconnected'
      const detail = ev.detail as { clean?: boolean } | undefined
      if (detail && detail.clean === false) {
        errorMsg.value = '连接异常断开（VM 未运行 / 插件未装载 / 会话被急停断连）'
      }
    })
    rfb = client
    client.focus({ preventScroll: true })
  } catch (e: any) {
    status.value = 'disconnected'
    errorMsg.value = 'noVNC 加载失败：' + (e?.message ?? e)
  }
}

function switchMode(m: 'ro' | 'rw') {
  // 不做 `m === mode.value` 防抖：v-model 与 @change 同元素时 Vue 按属性
  // 顺序先跑 v-model，进来时 mode.value 已等于 m，防抖恒命中 → 切档永不
  // 重连（旧 ro 连接静默保持，服务端继续丢键鼠事件——2026-10-21 演示实
  // 录）。radio 原生 change 只在选中值变化时触发，天然无重复。
  mode.value = m
  rfb?.disconnect()
  rfb = null
  // 断开后容器会被 noVNC 清空，重连前清一次防残留画布。
  if (container.value) container.value.innerHTML = ''
  connect()
}

function close() {
  emit('close')
}

onMounted(connect)

onBeforeUnmount(() => {
  disposed = true
  rfb?.disconnect()
  rfb = null
})
</script>

<template>
  <div class="vm-display-overlay" @click.self="close">
    <div class="vm-display-modal" data-test="vm-display-modal">
      <div class="vm-display-header">
        <h3 style="margin: 0; font-size: var(--text-base);">
          VM 屏幕 <code v-if="agentId" style="font-size: var(--text-xs);">{{ agentId.slice(0, 16) }}</code>
        </h3>
        <div style="display: flex; gap: var(--space-2); align-items: center;">
          <label style="display: flex; gap: var(--space-1); align-items: center; font-size: var(--text-xs); cursor: pointer;">
            <input type="radio" value="ro" v-model="mode" @change="switchMode('ro')" data-test="display-mode-ro" />
            观察（只读）
          </label>
          <label style="display: flex; gap: var(--space-1); align-items: center; font-size: var(--text-xs); cursor: pointer;">
            <input type="radio" value="rw" v-model="mode" @change="switchMode('rw')" data-test="display-mode-rw" />
            操作（键鼠）
          </label>
          <span class="vm-display-status" :data-state="status">
            {{ status === 'connecting' ? '连接中…' : status === 'connected' ? '● 已连接' : '○ 已断开' }}
          </span>
          <button class="btn btn-sm" data-test="display-close" @click="close">关闭</button>
        </div>
      </div>
      <div v-if="errorMsg" class="vm-display-error" data-test="display-error">{{ errorMsg }}</div>
      <div ref="container" class="vm-display-screen" data-test="display-screen"></div>
      <div class="vm-display-footer">
        当前模式：{{ mode === 'ro' ? '观察（键鼠输入被服务端过滤，画面只读）' : '操作（键鼠直达 VM；试玩/生产实例请谨慎操作）' }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.vm-display-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.55);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}
.vm-display-modal {
  background: var(--bg-primary, #1a1a2e);
  border: 1px solid var(--border);
  border-radius: 8px;
  width: min(1100px, 94vw);
  max-height: 92vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.vm-display-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--border);
  flex-wrap: wrap;
}
.vm-display-status {
  font-size: var(--text-xs);
  color: var(--text-secondary);
}
.vm-display-status[data-state='connected'] {
  color: var(--success);
}
.vm-display-error {
  padding: var(--space-2) var(--space-3);
  color: var(--danger, #c0392b);
  font-size: var(--text-sm);
}
.vm-display-screen {
  min-height: 420px;
  height: 66vh;
  background: #000;
  overflow: auto;
}
.vm-display-footer {
  padding: var(--space-1) var(--space-3);
  border-top: 1px solid var(--border);
  color: var(--text-secondary);
  font-size: var(--text-xs);
}
</style>
