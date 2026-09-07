/**
 * M6 (2026-09-07 devtool-upgrade 阶段 7): Ctrl+K 命令面板全局开关。
 *
 * 模块级单例状态——App.vue 的 document keydown（Ctrl/Cmd+K）调 open()，
 * AppLayout 挂载 <CommandPalette> 消费 visible。不进 Pinia：纯 UI 开关，
 * 无跨页面共享数据。
 */
import { ref } from 'vue'

const visible = ref(false)

export function useCommandPalette() {
  function open() {
    visible.value = true
  }
  function close() {
    visible.value = false
  }
  function toggle() {
    visible.value = !visible.value
  }
  return { visible, open, close, toggle }
}
