import { createApp } from 'vue'
import { createPinia } from 'pinia'
import ChatApp from './ChatApp.vue'

// Global styles (standalone chat reuses dashboard styles)
import '../styles/theme.css'
import '../styles/layout.css'
import '../styles/components.css'

createApp(ChatApp).use(createPinia()).mount('#app')
