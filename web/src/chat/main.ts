import { createApp } from 'vue'
import { createPinia } from 'pinia'
import ChatPanel from '../components/ChatPanel.vue'
import { useAuthStore } from '../stores/auth'

// Global styles (standalone chat reuses dashboard styles)
import '../styles/theme.css'
import '../styles/layout.css'
import '../styles/components.css'

const app = createApp({
  setup() {
    const auth = useAuthStore()

    return { auth }
  },
  template: `
    <div v-if="!auth.authenticated" style="display: flex; align-items: center; justify-content: center; height: 100vh; background: var(--bg);">
      <div class="auth-card">
        <h1>NemesisBot</h1>
        <p class="auth-subtitle">请输入访问密钥</p>
        <div class="auth-form">
          <input class="form-input" type="password" placeholder="访问密钥" autocomplete="off"
            :value="token" @input="token = ($event.target as HTMLInputElement).value"
            @keydown.enter="doLogin" :disabled="loading">
          <label class="auth-remember">
            <input type="checkbox" v-model="remember">
            <span>记住我</span>
          </label>
          <button class="btn btn-primary btn-lg" @click="doLogin" :disabled="loading">
            {{ loading ? '登录中...' : '登录' }}
          </button>
          <p class="auth-error" v-if="error">{{ error }}</p>
        </div>
      </div>
    </div>
    <ChatPanel v-else standalone />
  `,
  data() {
    return {
      token: '' as string,
      remember: true as boolean,
      loading: false as boolean,
      error: '' as string,
    }
  },
  async mounted() {
    // Auto-login
    const savedToken = localStorage.getItem('nemesisbot_auth_token')
    if (savedToken) {
      const success = await this.auth.autoLogin(savedToken)
      if (!success) {
        localStorage.removeItem('nemesisbot_auth_token')
      }
    }
  },
  methods: {
    async doLogin() {
      const t = this.token.trim()
      if (!t) { this.error = '请输入访问密钥'; return }
      this.loading = true
      this.error = ''
      const result = await this.auth.login(t, this.remember)
      this.loading = false
      if (!result.success) {
        this.error = '访问密钥无效，请检查后重试'
      }
    },
  },
})
app.use(createPinia())
app.mount('#app')
