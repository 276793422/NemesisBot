<script setup lang="ts">
/**
 * 全局自定义右键菜单浮层（2026-11-12 子需求 B）。菜单内容与定位来自
 * useContextMenu() 的全局状态（区域分发在 composable 的 listener 里完成，
 * 本组件只负责渲染：鼠标坐标定位 + 视口贴边收口 + 点外/Esc/滚动关闭）。
 *
 * 挂载：App.vue 一次性（与 installContextMenu() 配对）。开关 off 态下
 * listener 不 preventDefault 也不 openAt → 本组件自然永不显示，零残留。
 */
import { onMounted, onUnmounted, nextTick, ref, watch } from 'vue'
import { useContextMenu } from '../composables/useContextMenu'

const { state, close } = useContextMenu()

const root = ref<HTMLElement | null>(null)
/** 贴边收口后的最终坐标（状态到达时现算）。 */
const pos = ref({ x: 0, y: 0 })

function reposition(x: number, y: number) {
  const w = root.value?.offsetWidth ?? 140
  const h = root.value?.offsetHeight ?? 120
  pos.value = {
    x: Math.min(x, window.innerWidth - w - 8),
    y: Math.min(y, window.innerHeight - h - 8),
  }
}

watch(
  () => state.value,
  async (s) => {
    if (!s) return
    pos.value = { x: s.x, y: s.y }
    await nextTick()
    reposition(s.x, s.y)
  },
)

function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape' && state.value) {
    close()
    e.stopPropagation()
  }
}

function run(i: { disabled?: boolean; run: () => void }) {
  if (i.disabled) return
  close()
  i.run()
}

onMounted(() => window.addEventListener('keydown', onKey, true))
onUnmounted(() => window.removeEventListener('keydown', onKey, true))
</script>

<template>
  <Teleport to="body">
    <template v-if="state">
      <!-- 全屏透明覆盖层：点外/滚轮/右键关闭（互斥语义的兜底面——
           ChatPanel「⋯」菜单的覆盖层同构）。 -->
      <div class="ctx-overlay" @click="close" @wheel.passive="close" @contextmenu.prevent="close" />
      <div
        ref="root"
        class="ctx-menu"
        :style="{ left: pos.x + 'px', top: pos.y + 'px' }"
        @contextmenu.prevent="close"
      >
        <button
          v-for="(i, idx) in state.items"
          :key="idx"
          class="ctx-item"
          :class="{ 'ctx-item-disabled': i.disabled }"
          :title="i.title"
          :disabled="i.disabled"
          @click="run(i)"
        >
          {{ i.label }}
        </button>
      </div>
    </template>
  </Teleport>
</template>

<style scoped>
/* 覆盖层与菜单脱离 scoped 作用域走 Teleport → 用全局类名 + 非 scoped 块。
   这里只保留占位注释；真实样式见下方非 scoped 样式块。 */
</style>

<style>
.ctx-overlay {
  position: fixed;
  inset: 0;
  z-index: 2100;
  background: transparent;
}
.ctx-menu {
  position: fixed;
  z-index: 2101;
  min-width: 132px;
  padding: 4px;
  background: var(--surface, #fff);
  border: 1px solid var(--border, #ddd);
  border-radius: 6px;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.14);
  display: flex;
  flex-direction: column;
}
.ctx-item {
  border: none;
  background: none;
  text-align: left;
  padding: 6px 12px;
  font-size: 13px;
  color: var(--text-primary, inherit);
  cursor: pointer;
  border-radius: 4px;
  white-space: nowrap;
}
.ctx-item:hover {
  background: var(--bg-primary, rgba(0, 0, 0, 0.05));
}
.ctx-item-disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
.ctx-item-disabled:hover {
  background: none;
}
</style>
