import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { router } from './router'
import App from './App.vue'

// Global styles
import './styles/theme.css'
import './styles/layout.css'
import './styles/components.css'

// Vue Flow styles MUST be imported globally — importing inside a <style scoped>
// block via @import does not work (scoped adds attribute hashes to selectors,
// but @import'd CSS is not rewritten, so Vue Flow's internal styles never apply).
import '@vue-flow/core/dist/style.css'
import '@vue-flow/core/dist/theme-default.css'
import '@vue-flow/controls/dist/style.css'
import '@vue-flow/minimap/dist/style.css'

const app = createApp(App)
app.use(createPinia())
app.use(router)
app.mount('#app')
