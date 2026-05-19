import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { router } from './router'
import App from './App.vue'

// Global styles
import './styles/theme.css'
import './styles/layout.css'
import './styles/components.css'

const app = createApp(App)
app.use(createPinia())
app.use(router)
app.mount('#app')
