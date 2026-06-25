import { createApp } from 'vue'
import { createPinia } from 'pinia'
import WorkflowChatStandalone from './WorkflowChatStandalone.vue'

// Global styles (standalone chat reuses dashboard styles)
import '../styles/theme.css'
import '../styles/layout.css'
import '../styles/components.css'

// Parse the workflow index from the URL path. Vite serves this file at
// /workflow-chat.html in dev, but the path-mode URL is /workflow/chat/<8hex>
// — both forms are handled in WorkflowChatStandalone.vue.
const pathMatch = window.location.pathname.match(/\/workflow\/chat\/([0-9a-fA-F]+)/)
const hashMatch = window.location.hash.match(/workflow\/chat\/([0-9a-fA-F]+)/)
const index = pathMatch?.[1] || hashMatch?.[1] || ''

const app = createApp(WorkflowChatStandalone, { index })
app.use(createPinia())
app.mount('#app')
