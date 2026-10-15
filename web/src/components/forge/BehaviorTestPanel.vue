<script setup lang="ts">
// A 线行为回归结果面板（自进化升级 2026-10-14）。
// 对 workspace/skills/<name> 执行 frontmatter `tests` 段（forge.test WSAPI），
// 展示逐用例结果（确定性断言失败明细 + judge 分数）。结果只读——闸的
// 真相源在后端 pipeline（Stage2 && Stage3 全过才 deploy）。
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const skills = ref<string[]>([])
const selected = ref('')
const running = ref(false)
const lastResult = ref<any>(null)

onMounted(loadSkills)

async function loadSkills() {
  try {
    const data = await request('forge', 'registry.list')
    skills.value = (data?.skill_directories || [])
      .filter((s: any) => s.has_skill_md)
      .map((s: any) => s.name)
    if (!selected.value && skills.value.length > 0) selected.value = skills.value[0]
  } catch { /* ignore */ }
}

async function runTests() {
  if (!selected.value || running.value) return
  running.value = true
  lastResult.value = null
  try {
    const data = await request('forge', 'test', { name: selected.value })
    lastResult.value = data || null
    if (data?.skipped) {
      toast.info('该 skill 未声明行为测试（无 tests 段）')
    } else if (data?.gate) {
      toast.success(`行为回归全过（${data.passed}/${data.passed + data.failed}）`)
    } else if (data && data.failed > 0) {
      toast.error(`行为回归未过（${data.failed} 例失败）——部署闸不通过`)
    }
  } catch (e: any) {
    toast.error('执行失败: ' + e)
  } finally {
    running.value = false
  }
}

function verdictClass(r: any): string {
  return r?.passed ? 'badge-success' : 'badge-error'
}
</script>

<template>
  <div class="behavior-test-panel">
    <div class="card" style="margin-bottom: var(--space-4);">
      <div class="card-header"><h3>Skill 行为回归</h3></div>
      <div class="card-body">
        <div style="display: flex; gap: var(--space-3); align-items: center;">
          <select v-model="selected" class="input" style="max-width: 260px;" data-testid="skill-select">
            <option v-for="s in skills" :key="s" :value="s">{{ s }}</option>
          </select>
          <button class="btn btn-primary" :disabled="!selected || running" data-testid="run-btn" @click="runTests">
            {{ running ? '执行中…' : '运行行为测试' }}
          </button>
          <span v-if="lastResult?.skipped" class="badge badge-neutral" data-testid="skipped-badge">
            未声明 tests 段（诚实跳过）
          </span>
          <span v-else-if="lastResult" class="badge" :class="lastResult.gate ? 'badge-success' : 'badge-error'" data-testid="gate-badge">
            {{ lastResult.gate ? '部署闸通过' : '部署闸不通过' }}
          </span>
        </div>
        <p style="font-size: var(--text-xs); color: var(--text-muted); margin-top: var(--space-2);">
          tests 段随 SKILL.md frontmatter 分发；四层确定性断言（工具/文本/效果/预算）+ 可选 judge 层。
        </p>
      </div>
    </div>

    <div v-if="lastResult && !lastResult.skipped" class="card">
      <div class="card-header">
        <h3>结果</h3>
        <span style="font-size: var(--text-sm); color: var(--text-secondary);">
          {{ lastResult.passed }}/{{ lastResult.passed + lastResult.failed }} passed
        </span>
      </div>
      <div class="card-body">
        <div v-if="!lastResult.results?.length" class="empty-state"><p>无结果</p></div>
        <div v-else class="table-wrap">
          <table>
            <thead>
              <tr><th>用例</th><th>结论</th><th>失败明细</th><th>judge</th></tr>
            </thead>
            <tbody>
              <tr v-for="(r, idx) in lastResult.results" :key="idx" :data-testid="'case-row-' + r.name">
                <td style="font-weight: 500;">{{ r.name }}</td>
                <td><span class="badge" :class="verdictClass(r)">{{ r.passed ? '✓ pass' : '✗ FAIL' }}</span></td>
                <td style="font-size: var(--text-xs); color: var(--text-secondary);">
                  <div v-for="(f, i) in r.failures || []" :key="i">{{ f }}</div>
                  <span v-if="!r.failures?.length">--</span>
                </td>
                <td style="font-size: var(--text-xs);">
                  <template v-if="r.judge">{{ r.judge.score?.toFixed(2) }} / {{ r.judge.threshold }}</template>
                  <span v-else>--</span>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>
  </div>
</template>
