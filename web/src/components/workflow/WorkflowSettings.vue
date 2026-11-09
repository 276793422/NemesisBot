<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'

// 工作流设置（2026-11-08 配置页补全批次 2）：workflow.wall_clock_budget_secs
// 墙钟保险丝——执行超时取消（Failed + 原因注记，区别于用户取消）。
// config.set_field 写 config.json 的 workflow 段（typed）。
const { request } = useWSAPI()
const toast = useToast()

const budgetSecs = ref<number | null>(null)

onMounted(async () => {
  try {
    const cfg = await request('config', 'get')
    budgetSecs.value =
      typeof cfg?.workflow?.wall_clock_budget_secs === 'number'
        ? cfg.workflow.wall_clock_budget_secs
        : 3600
  } catch {
    budgetSecs.value = 3600
  }
})

function saveBudget(e: any) {
  const n = parseInt(e.target.value, 10)
  if (Number.isNaN(n) || n < 0) {
    e.target.value = String(budgetSecs.value ?? 3600)
    toast.error('须为 ≥0 整数秒（0 = 关闭保险丝）')
    return
  }
  budgetSecs.value = n
  request('config', 'set_field', { path: 'workflow.wall_clock_budget_secs', value: n })
    .then(() => toast.success('已更新 workflow.wall_clock_budget_secs'))
    .catch((err: any) => toast.error('保存失败: ' + err))
}
</script>

<template>
  <div class="wf-settings">
    <div class="card">
      <div class="card-header"><h3>执行参数</h3></div>
      <div class="card-body">
        <div class="form-group">
          <label class="form-label">墙钟保险丝（workflow.wall_clock_budget_secs）</label>
          <input
            class="form-input"
            type="number"
            min="0"
            step="60"
            style="max-width: 200px;"
            :value="budgetSecs ?? 3600"
            @change="saveBudget"
          >
          <span class="form-hint">
            单次工作流执行的墙钟上限（秒），超时取消（执行落 Failed + 原因注记，区别于用户取消）。
            0 = 关闭保险丝；默认 3600（1 小时）。
          </span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.wf-settings {
  height: 100%;
  overflow-y: auto;
  padding: var(--space-4);
}
</style>
