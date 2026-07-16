<script setup lang="ts">
defineProps<{ embedded?: boolean }>()
</script>

<template>
  <div :class="embedded ? 'local-models-embed' : 'page-local-models'">
    <div v-if="!embedded" class="page-header">
      <h2>本地模型</h2>
      <span class="badge badge-neutral">即将推出</span>
    </div>
    <div :class="embedded ? '' : 'page-body'">
      <div class="empty-state">
        <h3>功能尚未开放</h3>
        <p>
          本地大模型的安装与管理仍在开发中，当前不会提供可用操作。
          请先使用「云端 / API」标签配置模型。
        </p>
        <span class="badge badge-neutral" style="margin-top: var(--space-3);">即将推出</span>
      </div>
    </div>
  </div>
</template>
