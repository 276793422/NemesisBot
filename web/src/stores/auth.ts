import { defineStore } from 'pinia'
import { ref } from 'vue'
import { testConnection, connect, disconnect } from '../composables/useWebSocket'
import { connectEvents, disconnectEvents } from '../composables/useSSE'
import { useChatStore } from './chat'

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
      // 认证成功即落盘（2026-11-13）：注入链（plugin-ui __DASHBOARD_TOKEN__ /
      // URL token / 桌面 runtime 事件）此前只写内存，独立页（prompt-viewer 等
      // 与 Dashboard 同源的新 JS 上下文）读不到凭据——提示词查看器恒报
      // 「未检测到登录凭据」。同类坑第二例是 SSE 恒 401（2026-10-18，当时按
      // 内存优先散点修补）。凭据投影收口在此：任何认证成功路径都写，
      // 失败/登出都清；login(remember=false) 是唯一显式豁免。
      localStorage.setItem('nemesisbot_auth_token', testToken)
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
    // W3（2026-10-14 审计）：登出清掉全部会话的 busy/在飞 turn 登记——
    // 旧会话的轮次状态不该带进下个登录周期。
    useChatStore().dropAllSessionState()
    localStorage.removeItem('nemesisbot_auth_token')
    authenticated.value = false
    token.value = ''
  }

  return { token, authenticated, login, autoLogin, logout }
})
