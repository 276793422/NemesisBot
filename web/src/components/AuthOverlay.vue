<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useAuthStore } from '../stores/auth'

const auth = useAuthStore()

const loginToken = ref('')
const loginRemember = ref(true)
const loginError = ref('')
const loginLoading = ref(false)
const tokenInput = ref<HTMLInputElement | null>(null)

onMounted(() => {
  tokenInput.value?.focus()
})

async function handleLogin() {
  const token = loginToken.value.trim()
  if (!token) {
    loginError.value = '请输入访问密钥'
    return
  }

  loginError.value = ''
  loginLoading.value = true

  const result = await auth.login(token, loginRemember.value)

  loginLoading.value = false
  if (!result.success) {
    loginError.value = '访问密钥无效，请检查后重试'
  }
}

function handleKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter') {
    e.preventDefault()
    handleLogin()
  }
}
</script>

<template>
  <div class="auth-overlay">
    <div class="auth-card">
      <h1>Nemesis<span class="brand-dot">Bot</span></h1>
      <p class="auth-subtitle">请输入访问密钥以继续</p>
      <div class="auth-form">
        <input
          ref="tokenInput"
          class="form-input"
          type="password"
          placeholder="访问密钥"
          autocomplete="off"
          v-model="loginToken"
          @keydown="handleKeydown"
          :disabled="loginLoading"
        >
        <label class="auth-remember">
          <input type="checkbox" v-model="loginRemember">
          <span>记住我</span>
        </label>
        <button class="btn btn-primary btn-lg" @click="handleLogin" :disabled="loginLoading">
          <span v-if="!loginLoading">登录</span>
          <span v-else>
            <span class="spinner" style="width:16px;height:16px;border-width:2px;"></span>
            连接中...
          </span>
        </button>
        <p class="auth-error" v-if="loginError">{{ loginError }}</p>
      </div>
    </div>
  </div>
</template>
