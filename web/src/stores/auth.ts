import { defineStore } from 'pinia'
import { ref } from 'vue'
import { testConnection, connect, disconnect } from '../composables/useWebSocket'
import { connectEvents, disconnectEvents } from '../composables/useSSE'

export const useAuthStore = defineStore('auth', () => {
  const token = ref('')
  const authenticated = ref(false)

  async function login(testToken: string, remember = true): Promise<{ success: boolean; error?: string }> {
    const success = await testConnection(testToken)
    if (success) {
      if (remember) {
        localStorage.setItem('nemesisbot_auth_token', testToken)
      }
      token.value = testToken
      authenticated.value = true
      connect(null, testToken)
      connectEvents()
      return { success: true }
    }
    return { success: false, error: '认证失败' }
  }

  async function autoLogin(testToken: string): Promise<boolean> {
    const success = await testConnection(testToken)
    if (success) {
      token.value = testToken
      authenticated.value = true
      connect(null, testToken)
      connectEvents()
      return true
    }
    localStorage.removeItem('nemesisbot_auth_token')
    return false
  }

  function logout() {
    disconnect()
    disconnectEvents()
    localStorage.removeItem('nemesisbot_auth_token')
    authenticated.value = false
    token.value = ''
  }

  return { token, authenticated, login, autoLogin, logout }
})
