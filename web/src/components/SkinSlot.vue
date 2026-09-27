<script setup lang="ts">
/**
 * 皮肤结构挂载点（v2）：显示为 display:contents 的空壳容器，把引擎渲染
 * 的槽位结构直接注入布局流（sidebar 槽的 <aside> 即成为 AppLayout 的
 * 直接 flex 子元素，与原生 Sidebar 同一布局地位）。
 *
 * 重挂时机：watch [skinState.id, structRev]——id 切换（热翻/预览）或结构
 * 内容原子更新（T3 校准）时重建；引擎未就绪时同步消费 T0 冷启动缓存
 * （index.html IIFE 注入的 inert template），皮肤侧栏随首帧渲染（无原生闪）。
 * 卸载时交还引擎清理（effectScope.stop + 文档级监听摘除——无泄漏）。
 */
import { ref, watch, onMounted, onUnmounted } from 'vue'
import { skinState, skinHasSlot, getSkinEngine, consumeStructureCache } from '../composables/useSkin'

const props = defineProps<{ name: string }>()
const host = ref<HTMLElement | null>(null)

function render(): void {
  const node = host.value
  if (!node) return
  if (!skinHasSlot(props.name)) consumeStructureCache() // T0 缓存同步消费（幂等）
  if (skinHasSlot(props.name)) getSkinEngine().mount(props.name, node)
  else node.textContent = ''
}

watch(
  () => [skinState.id, skinState.structRev],
  () => render(),
  { flush: 'post' }
)
onMounted(render)
onUnmounted(() => getSkinEngine().unmount(props.name))
</script>

<template>
  <div ref="host" class="nb-slot-host" :data-nb-slot-host="name" style="display: contents"></div>
</template>
