<script setup lang="ts">
// Standalone Chat 独立入口根组件（2026-09-03 二次回归 BUG-2 根修）：
// 旧实现（chat/main.ts 内联组件 + template 字符串）有两处硬伤——
// ① Vite 下 `vue` 包默认解析为 runtime-only 构建，字符串模板没有运行时
//    编译器可用（页面空白，控制台告警 runtime compilation not supported）；
// ② ChatPanel 仅被模板字符串引用（非代码引用），Rollup tree-shake 直接剔除。
// 改为标准 SFC（<template> 构建期编译 + 显式组件引用）；登录卡复用
// AuthOverlay（与 Dashboard 单一真相源，不再手搓重复登录表单）。
import { onMounted } from 'vue'
import AuthOverlay from '../components/AuthOverlay.vue'
import ChatPanel from '../components/ChatPanel.vue'
import { useAuthStore } from '../stores/auth'

const auth = useAuthStore()

onMounted(async () => {
  // Auto-login（与 App.vue 的 localStorage 分支同语义；桌面端 token 注入、
  // URL fragment 等 Dashboard 专属通道不适用于独立聊天页）
  const savedToken = localStorage.getItem('nemesisbot_auth_token')
  if (savedToken && !auth.authenticated) {
    const success = await auth.autoLogin(savedToken)
    if (!success) {
      localStorage.removeItem('nemesisbot_auth_token')
    }
  }
})
</script>

<template>
  <AuthOverlay v-if="!auth.authenticated" />
  <ChatPanel v-else standalone />
</template>
