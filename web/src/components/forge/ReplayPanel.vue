<script setup lang="ts">
// C 线失败重放面板（自进化升级 2026-10-14）。
// turn WAL 失败记录 → 提取/用例化（forge.replay_extract）→ 待审区展示 +
// 确认入库（forge.replay_confirm）；已入回归集列表只读。执行面归 CLI
// （forge replay-run）——本面板承担 v3.1 裁决的「默认需评审」人工闸。
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../../composables/useWSAPI'
import { useToast } from '../../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const pending = ref<any[]>([])
const confirmed = ref<any[]>([])
const extracting = ref(false)

onMounted(load)

async function load() {
  try {
    const data = await request('forge', 'replay_list')
    pending.value = data?.pending || []
    confirmed.value = data?.confirmed || []
  } catch { /* ignore */ }
}

async function extract() {
  if (extracting.value) return
  extracting.value = true
  try {
    const data = await request('forge', 'replay_extract')
    toast.success(`提取 ${data?.extracted ?? 0} 条失败记录 → ${data?.dest === 'confirmed' ? '已入库（auto_ingest 开）' : '待审区'}`)
    await load()
  } catch (e: any) {
    toast.error('提取失败: ' + e)
  } finally {
    extracting.value = false
  }
}

async function confirmCase(name?: string) {
  try {
    const data = await request('forge', 'replay_confirm', name ? { name } : {})
    toast.success(`已确认 ${data?.confirmed ?? 0} 条入回归集`)
    await load()
  } catch (e: any) {
    toast.error('确认失败: ' + e)
  }
}
</script>

<template>
  <div class="replay-panel">
    <div class="card" style="margin-bottom: var(--space-4);">
      <div class="card-header">
        <h3>失败重放回归</h3>
        <button class="btn" :disabled="extracting" data-testid="extract-btn" @click="extract">
          {{ extracting ? '提取中…' : '从 turn WAL 提取失败' }}
        </button>
      </div>
      <div class="card-body">
        <p style="font-size: var(--text-xs); color: var(--text-muted);">
          被打断/出错的会话任务 → 重放用例；入库开关（forge.replay.auto_ingest）默认需在此确认。重放执行走 CLI（forge replay-run）。
        </p>
      </div>
    </div>

    <div class="card" style="margin-bottom: var(--space-4);">
      <div class="card-header">
        <h3>待审区</h3>
        <div style="display: flex; gap: var(--space-2); align-items: center;">
          <span style="font-size: var(--text-sm); color: var(--text-secondary);">{{ pending.length }} 条</span>
          <button
            v-if="pending.length > 0"
            class="btn btn-primary"
            data-testid="confirm-all-btn"
            @click="confirmCase()"
          >全部确认</button>
        </div>
      </div>
      <div class="card-body">
        <div v-if="pending.length === 0" class="empty-state"><p>暂无待审重放用例</p></div>
        <div v-else class="table-wrap">
          <table>
            <thead><tr><th>用例名</th><th>任务（重放指令）</th><th>操作</th></tr></thead>
            <tbody>
              <tr v-for="c in pending" :key="c.name" :data-testid="'pending-' + c.name">
                <td style="font-weight: 500;">{{ c.name }}</td>
                <td style="max-width: 420px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--text-xs);">
                  {{ (c.turns || []).join(' / ') || '--' }}
                </td>
                <td>
                  <button class="btn" style="font-size: var(--text-xs);" :data-testid="'confirm-' + c.name" @click="confirmCase(c.name)">确认入库</button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>

    <div class="card">
      <div class="card-header">
        <h3>已入回归集</h3>
        <span style="font-size: var(--text-sm); color: var(--text-secondary);">{{ confirmed.length }} 条</span>
      </div>
      <div class="card-body">
        <div v-if="confirmed.length === 0" class="empty-state"><p>暂无已确认用例</p></div>
        <div v-else class="table-wrap">
          <table>
            <thead><tr><th>用例名</th><th>任务（重放指令）</th></tr></thead>
            <tbody>
              <tr v-for="c in confirmed" :key="c.name" :data-testid="'confirmed-' + c.name">
                <td style="font-weight: 500;">{{ c.name }}</td>
                <td style="max-width: 480px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--text-xs);">
                  {{ (c.turns || []).join(' / ') || '--' }}
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>
  </div>
</template>
