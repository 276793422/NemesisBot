<script setup lang="ts">
import { useToast, type Toast } from '../composables/useToast'

const { toasts, remove } = useToast()

const icons: Record<Toast['type'], string> = {
  info: '\u2139\uFE0F',
  success: '\u2705',
  warn: '\u26A0\uFE0F',
  error: '\u274C',
}
const titles: Record<Toast['type'], string> = {
  info: '信息',
  success: '成功',
  warn: '警告',
  error: '错误',
}
</script>

<template>
  <div class="toast-container">
    <div
      v-for="toast in toasts"
      :key="toast.id"
      class="toast"
      :class="toast.type"
    >
      <div class="toast-icon">{{ icons[toast.type] }}</div>
      <div class="toast-body">
        <div class="toast-title">{{ titles[toast.type] }}</div>
        <div class="toast-message">{{ toast.message }}</div>
      </div>
      <button class="toast-close" @click="remove(toast.id)">&times;</button>
    </div>
  </div>
</template>
