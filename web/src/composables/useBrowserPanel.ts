/**
 * ChatPanel 侧「浏览器点名面板」开合的共享状态（2026-11-15，计划 D10）。
 *
 * 复刻 useFileTreePanel 单例模式——状态提升为模块级单例：
 * BrowserPickPanel 消费渲染、SessionSidebar「浏览器」按钮触发 toggle。
 * 点名发生在对话中途（跳页点名反人类），职责收窄为纯点名器：
 * 运行实例状态 + tab 列表可点选；管理/配置/审计归执行单元【浏览器】TAB。
 * 内存态不持久化：面板开合是即时意图，刷新即收比带陈旧开态诚实。
 */
import { ref } from 'vue'

const open = ref(false)

export function useBrowserPanel() {
  function toggle() {
    open.value = !open.value
  }

  function setOpen(v: boolean) {
    open.value = v
  }

  return { open, toggle, setOpen }
}
