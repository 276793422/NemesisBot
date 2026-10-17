/**
 * 工作区文件树开合的共享状态（2026-09-08 侧栏三块改版）。
 *
 * 开合入口从 FileTreePanel 自身的细条 rail 收编到会话侧栏顶部功能区的
 * 「工作区」按钮（侧栏结构：功能按钮区 / 项目区 / 对话区）。状态提升为
 * 模块级单例——FileTreePanel 消费渲染、SessionSidebar 触发 toggle。
 * 持久化沿用原 localStorage 键（'nb_filetree_collapsed'，'0'=展开），
 * 升级用户的既有偏好保留。
 */
import { ref } from 'vue'

const STORAGE_KEY = 'nb_filetree_collapsed'

function initialCollapsed(): boolean {
  // '0' = 用户偏好展开；'1'/缺省 = 折叠。
  return localStorage.getItem(STORAGE_KEY) !== '0'
}

const collapsed = ref(initialCollapsed())

/**
 * I5（2026-10-17）：当前上下文文件（null = 未设定）。
 *
 * 文件树面板行尾 📌 设定/切换/取消（见 FileTreePanel），ChatPanel 每次发送
 * 读走——chat.send data.open_files 上报，后端清洗后注入每轮 merged 快照节
 * （`# Open Files (client-reported)`，只报路径不读内容）。设定一次后续消息
 * 自动携带，免反复 @ 路径。内存态不持久化：上下文是即时意图，刷新即清比
 * 带陈旧路径诚实。
 */
const currentFile = ref<string | null>(null)

export function useFileTreePanel() {
  function persist() {
    localStorage.setItem(STORAGE_KEY, collapsed.value ? '1' : '0')
  }

  function toggle() {
    collapsed.value = !collapsed.value
    persist()
  }

  function setCollapsed(v: boolean) {
    collapsed.value = v
    persist()
  }

  /** 📌 行动作：同路径再点 = 取消，异路径 = 切换。 */
  function setCurrentFile(path: string | null) {
    currentFile.value = path
  }

  return { collapsed, toggle, setCollapsed, currentFile, setCurrentFile }
}
