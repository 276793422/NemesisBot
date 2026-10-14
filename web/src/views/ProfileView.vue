<script setup lang="ts">
// 用户习惯画像面板（自进化升级 B 线，2026-10-14）。
// 展示 profile.list（enabled/enabled_at/habits）；后门动作 veto/forget/pin；
// 开关 profile.enable/disable（真相源 config.json user_profile.enabled +
// user_profile.json enabled_at 仅向前锚点）。生成侧免确认自动生效——本页
// 只做展示与人工后门，不做审批闸。硬边界：权限/安全语义内容在渲染层
// 过滤（后端 user_profile::violates_security_boundary），页面不重复判断。
import { ref, onMounted } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

const { request } = useWSAPI()
const toast = useToast()

const enabled = ref(false)
const enabledAt = ref<string | null>(null)
const habits = ref<any[]>([])
const loading = ref(true)

const CATEGORY_LABEL: Record<string, string> = {
  workflow: '工作流',
  correction: '纠正',
  tool_preference: '工具偏好',
  style: '风格',
}

function categoryLabel(c: string): string {
  return CATEGORY_LABEL[c] || c
}

function statusClass(s: string): string {
  const map: Record<string, string> = {
    active: 'badge-success',
    decaying: 'badge-warning',
    expired: 'badge-neutral',
    vetoed: 'badge-error',
  }
  return map[s] || 'badge-neutral'
}

function statusLabel(s: string): string {
  const map: Record<string, string> = {
    active: '活跃',
    decaying: '衰减中',
    expired: '已过期',
    vetoed: '已否决',
  }
  return map[s] || s
}

onMounted(load)

async function load() {
  loading.value = true
  try {
    const data = await request('profile', 'list')
    enabled.value = !!data?.enabled
    enabledAt.value = data?.enabled_at || null
    habits.value = data?.habits || []
  } catch { /* ignore */ } finally {
    loading.value = false
  }
}

async function enableProfile() {
  try {
    const data = await request('profile', 'enable')
    toast.success('用户习惯画像已开启（仅采集此后会话）')
    enabledAt.value = data?.enabled_at || enabledAt.value
    enabled.value = true
  } catch (e: any) {
    toast.error('开启失败: ' + e)
  }
}

async function disableProfile() {
  try {
    await request('profile', 'disable')
    toast.info('用户习惯画像已关闭（已有条目保留、不再采集）')
    enabled.value = false
  } catch (e: any) {
    toast.error('关闭失败: ' + e)
  }
}

async function act(cmd: string, content: string, tip: string) {
  try {
    const data = await request('profile', cmd, { content })
    if (!data?.vetoed && !data?.forgotten && !data?.updated) {
      toast.info('未命中任何条目')
    } else {
      toast.success(tip)
    }
    await load()
  } catch (e: any) {
    toast.error('操作失败: ' + e)
  }
}

function veto(h: any) { act('veto', h.content, '已否决（负样本，不再生成）') }
function forget(h: any) { act('forget', h.content, '已遗忘（同语义可重新生成）') }
function pin(h: any) { act('pin', h.content, h.pinned ? '已取消钉住' : '已钉住（不衰减、置顶注入）') }
</script>

<template>
  <div class="page-profile">
    <div class="page-header">
      <h2>用户习惯</h2>
      <div class="page-header-actions">
        <span v-if="enabled && enabledAt" style="font-size: var(--text-xs); color: var(--text-muted);" data-testid="enabled-at">
          采集起点：{{ new Date(enabledAt).toLocaleString('zh-CN') }}
        </span>
        <button v-if="!enabled" class="btn btn-primary" data-testid="enable-btn" @click="enableProfile">开启画像</button>
        <button v-else class="btn" data-testid="disable-btn" @click="disableProfile">关闭画像</button>
      </div>
    </div>

    <div class="page-body">
      <div class="card" style="margin-bottom: var(--space-4);">
        <div class="card-body">
          <p style="font-size: var(--text-sm); color: var(--text-secondary); margin: 0;">
            从会话中自动学习稳定偏好（工作流形态 / 风格 / 工具偏好），注入后续会话使回复更贴合。
            <strong>只调整风格与流程姿态，绝不触碰权限与安全语义</strong>；全本地采集，
            开启后只分析此后会话（仅向前）。生成免确认自动生效，下方提供否决 / 遗忘 / 钉住后门。
          </p>
        </div>
      </div>

      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
      </div>

      <div v-else class="card">
        <div class="card-header">
          <h3>习惯条目</h3>
          <span style="font-size: var(--text-sm); color: var(--text-secondary);">{{ habits.length }} 条</span>
        </div>
        <div class="card-body">
          <div v-if="habits.length === 0" class="empty-state" data-testid="empty-state">
            <p>{{ enabled ? '暂无习惯条目——正常使用若干会话后自动生成' : '画像未开启（config user_profile.enabled）' }}</p>
          </div>
          <div v-else class="table-wrap">
            <table>
              <thead>
                <tr><th>类别</th><th>习惯</th><th>置信度</th><th>命中</th><th>状态</th><th>操作</th></tr>
              </thead>
              <tbody>
                <tr v-for="(h, idx) in habits" :key="idx" :data-testid="'habit-row-' + idx">
                  <td><span class="badge badge-info">{{ categoryLabel(h.category) }}</span></td>
                  <td style="max-width: 420px;">
                    {{ h.content }}
                    <span v-if="h.pinned" class="badge badge-neutral" style="margin-left: var(--space-1);">📌 pinned</span>
                  </td>
                  <td style="font-variant-numeric: tabular-nums;">{{ (h.confidence * 100).toFixed(0) }}%</td>
                  <td>{{ h.hits }}</td>
                  <td><span class="badge" :class="statusClass(h.status)" :data-testid="'habit-status-' + idx">{{ statusLabel(h.status) }}</span></td>
                  <td style="white-space: nowrap;">
                    <button class="btn" style="font-size: var(--text-xs); margin-right: var(--space-1);" :data-testid="'pin-' + idx" @click="pin(h)">{{ h.pinned ? '取消钉住' : '钉住' }}</button>
                    <button class="btn" style="font-size: var(--text-xs); margin-right: var(--space-1);" :data-testid="'veto-' + idx" @click="veto(h)">否决</button>
                    <button class="btn" style="font-size: var(--text-xs);" :data-testid="'forget-' + idx" @click="forget(h)">遗忘</button>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
