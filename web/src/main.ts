import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { router } from './router'
import App from './App.vue'
import { applySkinBoot } from './composables/useSkin'
import { installNemesisSkin } from './skins/contract'

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

// 皮肤脚本契约（P2b）：同步安装，必须先于 applySkinBoot——脚本注入走网络
// 异步路径，同步安装保证任何皮肤脚本执行时 window.NemesisSkin 必然在场。
installNemesisSkin()

// 皮肤异步校准（不阻塞挂载——index.html 内联脚本已同步注入缓存态）
void applySkinBoot()

const app = createApp(App)
app.use(createPinia())
app.use(router)
app.mount('#app')
