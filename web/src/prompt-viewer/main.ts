import { createApp } from 'vue'
import PromptViewerPage from './PromptViewerPage.vue'

// 提示词内容可视化（2026-11-13）：段全文独立只读查看器页（第 5 个 Vite MPA
// 入口）。无 router/pinia——自持最小 WS 客户端（useWebSocket 是 dashboard
// 的全局单例，不适用独立页）；鉴权 = 同源共享的 localStorage 登录 token
// （浏览器新标签页与 Dashboard 同源，天然带登录态）。
import '../styles/theme.css'
import '../styles/components.css'
import '../styles/prompt-segments.css'

// 主题对齐 dashboard（useTheme 模块加载即 applyTheme：读 nemesisbot_theme /
// prefers-color-scheme，写 data-theme 属性；开关状态与 dashboard 共享同一
// localStorage 键）。
import '../composables/useTheme'

createApp(PromptViewerPage).mount('#app')
