<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { injectCanvasCsp, useCanvas } from '../composables/useCanvas'
import { useSessionStore } from '../stores/session'

/**
 * P30（WS14）：Canvas 面板。agent 终答检出合法 ```canvas 块后经 SSE
 * `canvas.open` 推到这里，以 iframe srcdoc 渲染：
 * - `sandbox="allow-scripts"`（刻意**不带** allow-same-origin：canvas
 *   内容触达不了父页 DOM/localStorage，逃逸面收敛到 srcdoc 自身）；
 * - srcdoc 注入严格 CSP meta（default-src 'none'）——v1 完全无网络，
 *   图片/外链一律拦截；数据必须内联（application/json 数据岛原样保留）；
 * - 自导航兜底（2026-09-26 复检挂账高优 Canvas-#1）：sandbox 不带
 *   allow-top-navigation 只挡导航父级、不挡 iframe 导航它自己——
 *   `location.href='http://…'` 跳出后新文档脱离我们的 CSP。srcdoc 文档
 *   的每次额外 load 事件 = 发生过一次面板内导航：计数告警 + 以 reset
 *   nonce 强制重灌 srcdoc 拉回初始文档。启发式兜底而非硬闸。
 */
const props = defineProps<{ sessionId?: string }>()

const { canvasesBySession, initCanvas, closeCanvas } = useCanvas()

/** 未显式指定会话时跟随当前聊天会话（懒取——纯组件测试可不装 pinia）。 */
const resolvedSessionId = computed(() => {
  if (props.sessionId) return props.sessionId
  return useSessionStore().currentId ?? ''
})

const canvases = computed(() => canvasesBySession[resolvedSessionId.value] ?? [])

/** 当前展示的块。新块到达自动切到最新（canvas.open = 打开新画布）；
 * 无新块且选中块被关掉时落到最后一块。 */
const activeIndex = ref(0)
watch(
  () => canvases.value.map(c => c.index),
  (idxs, prev) => {
    if (!idxs.length) return
    const prevSet = new Set(prev ?? [])
    const fresh = idxs.filter(i => !prevSet.has(i))
    if (fresh.length) {
      activeIndex.value = fresh[fresh.length - 1]
    } else if (!idxs.includes(activeIndex.value)) {
      activeIndex.value = idxs[idxs.length - 1]
    }
  },
  { immediate: true }
)

const active = computed(() => canvases.value.find(c => c.index === activeIndex.value) ?? canvases.value[0] ?? null)

// ---- 自导航兜底状态机 ----
const navBlocked = ref(0)
const resetNonce = ref(0)
/** srcdoc 文档就绪后的 load 次数；>1 = 面板内发生过导航。 */
let settledLoads = 0
/** 重灌 srcdoc 引起的 load 不计入（一次性消费标志）。 */
let resetting = false
let prevNonce = 0

/** srcdoc 原文 = 注入 CSP meta 后的块内容；reset nonce 变化时追加注释
 * 强制字符串变化 → iframe 重灌回我们的初始文档。 */
const preparedHtml = computed(() => {
  const base = active.value ? injectCanvasCsp(active.value.html) : ''
  return base ? `${base}\n<!-- nav-reset:${resetNonce.value} -->` : ''
})

watch(preparedHtml, () => {
  if (resetNonce.value === prevNonce) {
    // 内容变化（切块/数据重灌）→ 会有一次新 load，重数计数。
    settledLoads = 0
  } else {
    prevNonce = resetNonce.value
  }
})

function onFrameLoad() {
  if (resetting) {
    resetting = false
    return
  }
  settledLoads += 1
  if (settledLoads > 1) {
    navBlocked.value += 1
    settledLoads = 1
    resetting = true
    resetNonce.value += 1
    console.warn('[canvas] 检测到面板内导航，已重灌初始文档（第 %d 次）', navBlocked.value)
  }
}

function onClose() {
  if (!active.value) return
  closeCanvas(resolvedSessionId.value, active.value.index)
}

defineExpose({ initCanvas })
</script>

<template>
  <div v-if="active" class="canvas-panel" data-testid="canvas-panel">
    <div class="canvas-panel-header">
      <span class="canvas-panel-title">Canvas</span>
      <span
        v-if="navBlocked > 0"
        class="canvas-nav-warn"
        data-testid="canvas-nav-warn"
        title="画布内容试图导航到外部页面，已被拦截并重置"
      >⚠ 导航已拦截 ×{{ navBlocked }}</span>
      <div v-if="canvases.length > 1" class="canvas-tabs">
        <button
          v-for="c in canvases"
          :key="c.index"
          class="canvas-tab"
          :class="{ active: c.index === activeIndex }"
          data-testid="canvas-tab"
          @click="activeIndex = c.index"
        >
          块 {{ c.index + 1 }}
        </button>
      </div>
      <button class="canvas-close" data-testid="canvas-close" title="关闭画布" @click="onClose">×</button>
    </div>
    <iframe
      class="canvas-frame"
      data-testid="canvas-frame"
      sandbox="allow-scripts"
      :srcdoc="preparedHtml"
      @load="onFrameLoad"
    ></iframe>
  </div>
</template>

<style scoped>
.canvas-panel {
  position: fixed;
  right: 16px;
  bottom: 16px;
  width: min(520px, calc(100vw - 32px));
  height: min(420px, calc(100vh - 32px));
  display: flex;
  flex-direction: column;
  background: var(--bg-secondary, #1e1e28);
  border: 1px solid var(--border-color, #33334a);
  border-radius: 10px;
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.35);
  z-index: 900;
  overflow: hidden;
}

.canvas-panel-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  border-bottom: 1px solid var(--border-color, #33334a);
}

.canvas-panel-title {
  font-weight: 600;
  font-size: 13px;
}

.canvas-nav-warn {
  color: var(--warning, #eab308);
  font-size: 11px;
  white-space: nowrap;
}

.canvas-tabs {
  display: flex;
  gap: 4px;
  flex: 1;
  overflow-x: auto;
}

.canvas-tab {
  border: 1px solid transparent;
  background: transparent;
  color: var(--text-secondary, #9aa);
  padding: 2px 8px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 12px;
}

.canvas-tab.active {
  color: var(--text-primary, #eee);
  border-color: var(--border-color, #445);
  background: var(--bg-tertiary, #2a2a3a);
}

.canvas-close {
  margin-left: auto;
  border: none;
  background: transparent;
  color: var(--text-secondary, #9aa);
  font-size: 16px;
  line-height: 1;
  cursor: pointer;
  padding: 2px 6px;
  border-radius: 6px;
}

.canvas-close:hover {
  color: var(--danger, #e55);
  background: var(--bg-tertiary, #2a2a3a);
}

.canvas-frame {
  flex: 1;
  width: 100%;
  border: none;
  background: #fff;
}
</style>
