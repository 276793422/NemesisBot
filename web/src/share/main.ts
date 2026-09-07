import { createApp } from 'vue'
import SharePage from './SharePage.vue'

// L4 会话分享：独立只读页（第 4 个 Vite MPA 入口）。无 router/pinia/
// WebSocket —— 纯 fetch GET /api/share/{token}，token 来自 ?t=。零外部
// 依赖（自托管即分享），样式只引主题变量。
import '../styles/theme.css'

createApp(SharePage).mount('#app')
