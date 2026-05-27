<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import VChart from 'vue-echarts'
import { use } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { LineChart } from 'echarts/charts'
import {
  GridComponent,
  TooltipComponent,
  LegendComponent,
  DataZoomComponent,
} from 'echarts/components'

use([CanvasRenderer, LineChart, GridComponent, TooltipComponent, LegendComponent, DataZoomComponent])

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface UsageSummary {
  totalRequests: number
  totalInputTokens: number
  totalOutputTokens: number
  totalCacheCreationTokens: number
  totalCacheReadTokens: number
  totalCost: number
  successRate: number
  cacheHitRate: number
}

interface TrendPoint {
  date: string
  inputTokens: number
  outputTokens: number
  cacheCreationTokens: number
  cacheReadTokens: number
  cost: number
}

type RangePreset = 'today' | '1d' | '7d' | '14d' | '30d' | 'custom'
type TabId = 'usage' | 'settings'

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

const activeTab = ref<TabId>('usage')
const loading = ref(true)
const preset = ref<RangePreset>('today')
const showCustomRange = ref(false)
const customStart = ref('')
const customEnd = ref('')

const summary = ref<UsageSummary>({
  totalRequests: 0,
  totalInputTokens: 0,
  totalOutputTokens: 0,
  totalCacheCreationTokens: 0,
  totalCacheReadTokens: 0,
  totalCost: 0,
  successRate: 0,
  cacheHitRate: 0,
})
const trends = ref<TrendPoint[]>([])

const presets: { key: Exclude<RangePreset, 'custom'>; label: string }[] = [
  { key: 'today', label: '今天' },
  { key: '1d', label: '近 1 天' },
  { key: '7d', label: '近 7 天' },
  { key: '14d', label: '近 14 天' },
  { key: '30d', label: '近 30 天' },
]

// ---------------------------------------------------------------------------
// Computed
// ---------------------------------------------------------------------------

const inputTotal = computed(() =>
  summary.value.totalInputTokens +
  summary.value.totalCacheCreationTokens +
  summary.value.totalCacheReadTokens,
)

const outputTotal = computed(() =>
  summary.value.totalOutputTokens,
)

const hitPercent = computed(() => {
  const rate = summary.value.cacheHitRate
  return rate >= 0 ? Math.min(100, Math.max(0, rate * 100)) : 0
})

function formatTokens(n: number): string {
  if (n >= 1_000_000_000) return (n / 1_000_000_000).toFixed(1) + 'B'
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M'
  if (n >= 1_000) return (n / 1_000).toFixed(1) + 'K'
  return n.toLocaleString()
}

function formatCost(n: number): string {
  if (n === 0) return '$0'
  if (n < 0.01) return '$' + n.toFixed(4)
  return '$' + n.toFixed(2)
}

const chartOption = computed(() => {
  const data = trends.value
  if (!data.length) return {}

  const dates = data.map(d => d.date)
  return {
    tooltip: {
      trigger: 'axis',
      backgroundColor: 'rgba(15, 14, 14, 0.95)',
      borderColor: 'var(--border)',
      textStyle: { color: '#D1D1D1', fontSize: 12 },
    },
    legend: {
      data: ['输入 Tokens', '输出 Tokens', '缓存写入', '缓存命中'],
      textStyle: { color: 'var(--text-muted)', fontSize: 11 },
      top: 0,
      right: 0,
      itemWidth: 14,
      itemHeight: 8,
      itemGap: 16,
    },
    grid: { top: 40, right: 16, bottom: 24, left: 50 },
    xAxis: {
      type: 'category',
      data: dates,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: { color: 'var(--text-muted)', fontSize: 11, rotate: data.length > 48 ? 30 : 0 },
    },
    yAxis: {
      type: 'value',
      axisLine: { show: false },
      axisTick: { show: false },
      splitLine: { lineStyle: { color: 'var(--border-light)', type: 'dashed' } },
      axisLabel: {
        color: 'var(--text-muted)',
        fontSize: 11,
        formatter: (v: number) => {
          if (v >= 1000) return (v / 1000).toFixed(0) + 'k'
          return String(v)
        },
      },
    },
    series: [
      {
        name: '输入 Tokens',
        type: 'line',
        stack: 'tokens',
        areaStyle: { color: 'rgba(59, 130, 246, 0.15)' },
        lineStyle: { color: '#3B82F6', width: 2 },
        itemStyle: { color: '#3B82F6' },
        showSymbol: false,
        smooth: true,
        data: data.map(d => d.inputTokens),
      },
      {
        name: '输出 Tokens',
        type: 'line',
        stack: 'tokens',
        areaStyle: { color: 'rgba(34, 197, 94, 0.15)' },
        lineStyle: { color: '#22C55E', width: 2 },
        itemStyle: { color: '#22C55E' },
        showSymbol: false,
        smooth: true,
        data: data.map(d => d.outputTokens),
      },
      {
        name: '缓存写入',
        type: 'line',
        stack: 'tokens',
        areaStyle: { color: 'rgba(249, 115, 22, 0.12)' },
        lineStyle: { color: '#F97316', width: 2 },
        itemStyle: { color: '#F97316' },
        showSymbol: false,
        smooth: true,
        data: data.map(d => d.cacheCreationTokens),
      },
      {
        name: '缓存命中',
        type: 'line',
        stack: 'tokens',
        areaStyle: { color: 'rgba(168, 85, 247, 0.12)' },
        lineStyle: { color: '#A855F7', width: 2 },
        itemStyle: { color: '#A855F7' },
        showSymbol: false,
        smooth: true,
        data: data.map(d => d.cacheReadTokens),
      },
    ],
  }
})

// ---------------------------------------------------------------------------
// Data loading
// ---------------------------------------------------------------------------

function getTimeRange(): { start: number; end: number } {
  const end = Math.floor(Date.now() / 1000)
  if (preset.value === 'custom') {
    if (customStart.value && customEnd.value) {
      return {
        start: Math.floor(new Date(customStart.value).getTime() / 1000),
        end: Math.floor(new Date(customEnd.value).getTime() / 1000),
      }
    }
    return { start: end - 86400, end }
  }
  if (preset.value === 'today') {
    const now = new Date()
    const startOfDay = new Date(now.getFullYear(), now.getMonth(), now.getDate())
    return { start: Math.floor(startOfDay.getTime() / 1000), end }
  }
  const days = parseInt(preset.value)
  return { start: end - days * 86400, end }
}

async function fetchJSON<T>(url: string): Promise<T> {
  const resp = await fetch(url)
  if (!resp.ok) throw new Error(`HTTP ${resp.status}`)
  const json = await resp.json()
  if (json.error) throw new Error(json.error)
  return json.data as T
}

interface ApiSummary {
  totalRequests: number
  successCount: number
  totalInputTokens: number
  totalOutputTokens: number
  totalCacheCreationTokens: number
  totalCacheReadTokens: number
  totalCostUsd: number
  avgLatencyMs: number
  cacheHitRate: number
}

interface ApiTrendPoint {
  label: string
  timestamp: number
  inputTokens: number
  outputTokens: number
  cacheCreationTokens: number
  cacheReadTokens: number
  requestCount: number
  totalCostUsd: number
}

async function loadData() {
  loading.value = true
  try {
    const { start, end } = getTimeRange()
    const groupBy = (end - start) > 86400 ? 'day' : 'hour'

    const [summaryData, trendsData] = await Promise.all([
      fetchJSON<ApiSummary>(`/api/usage/summary?start=${start}&end=${end}`),
      fetchJSON<ApiTrendPoint[]>(`/api/usage/trends?start=${start}&end=${end}&group_by=${groupBy}`),
    ])

    summary.value = {
      totalRequests: summaryData.totalRequests,
      totalInputTokens: summaryData.totalInputTokens,
      totalOutputTokens: summaryData.totalOutputTokens,
      totalCacheCreationTokens: summaryData.totalCacheCreationTokens,
      totalCacheReadTokens: summaryData.totalCacheReadTokens,
      totalCost: summaryData.totalCostUsd,
      successRate: summaryData.totalRequests > 0
        ? (summaryData.successCount / summaryData.totalRequests) * 100
        : 0,
      cacheHitRate: summaryData.cacheHitRate / 100,
    }

    trends.value = trendsData.map(p => ({
      date: p.label,
      inputTokens: p.inputTokens,
      outputTokens: p.outputTokens,
      cacheCreationTokens: p.cacheCreationTokens,
      cacheReadTokens: p.cacheReadTokens,
      cost: p.totalCostUsd,
    }))
  } catch (err) {
    console.error('[UsageView] Failed to load data:', err)
  }
  loading.value = false
}

function setPreset(p: Exclude<RangePreset, 'custom'>) {
  preset.value = p
  showCustomRange.value = false
  loadData()
}

function openCustomRange() {
  showCustomRange.value = !showCustomRange.value
  if (showCustomRange.value) {
    // Default: last 7 days
    const now = new Date()
    const weekAgo = new Date(now.getTime() - 7 * 86400000)
    customEnd.value = now.toISOString().slice(0, 16)
    customStart.value = weekAgo.toISOString().slice(0, 16)
  }
}

function applyCustomRange() {
  if (!customStart.value || !customEnd.value) return
  if (new Date(customStart.value) >= new Date(customEnd.value)) return
  preset.value = 'custom'
  showCustomRange.value = false
  loadData()
}

function initDefaultDates() {
  const now = new Date()
  const weekAgo = new Date(now.getTime() - 7 * 86400000)
  customEnd.value = now.toISOString().slice(0, 16)
  customStart.value = weekAgo.toISOString().slice(0, 16)
}

onMounted(() => {
  initDefaultDates()
  loadData()
})
</script>

<template>
  <div class="page-usage">
    <div class="page-header">
      <h2>使用统计</h2>
    </div>

    <div class="page-body">
      <!-- Top-level tabs -->
      <div class="tab-bar">
        <button class="tab-btn" :class="{ active: activeTab === 'usage' }" @click="activeTab = 'usage'">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 20V10M12 20V4M6 20v-6"/></svg>
          使用量
        </button>
        <button class="tab-btn" :class="{ active: activeTab === 'settings' }" @click="activeTab = 'settings'">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/></svg>
          设置
        </button>
      </div>

      <!-- Settings tab -->
      <div v-if="activeTab === 'settings'" class="settings-placeholder">
        <div class="card">
          <div class="card-body" style="text-align: center; padding: var(--space-10); color: var(--text-muted);">
            <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" style="margin-bottom: var(--space-3); opacity: 0.3;"><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/></svg>
            <p>设置功能开发中...</p>
          </div>
        </div>
      </div>

      <!-- Usage tab -->
      <template v-if="activeTab === 'usage'">
        <!-- Time range selector -->
        <div class="usage-toolbar">
          <div class="preset-group">
            <button
              v-for="p in presets"
              :key="p.key"
              class="preset-btn"
              :class="{ active: preset === p.key }"
              @click="setPreset(p.key)"
            >{{ p.label }}</button>
            <button
              class="preset-btn"
              :class="{ active: preset === 'custom' }"
              @click="openCustomRange"
            >自定义</button>
          </div>
        </div>

        <!-- Custom range picker -->
        <div v-if="showCustomRange" class="custom-range-panel">
          <div class="custom-range-fields">
            <div class="custom-field">
              <label>开始时间</label>
              <input type="datetime-local" v-model="customStart" />
            </div>
            <div class="custom-range-arrow">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="m12 5 7 7-7 7"/></svg>
            </div>
            <div class="custom-field">
              <label>结束时间</label>
              <input type="datetime-local" v-model="customEnd" />
            </div>
          </div>
          <div class="custom-range-actions">
            <button class="btn-secondary" @click="showCustomRange = false">取消</button>
            <button class="btn-primary" @click="applyCustomRange">确认</button>
          </div>
        </div>

        <!-- Hero card -->
        <div class="hero-card">
          <div v-if="loading" class="hero-loading">
            <div class="spinner" style="width: 24px; height: 24px;"></div>
          </div>
          <template v-else>
            <!-- Header -->
            <div class="hero-header">
              <div class="hero-title">
                <div class="hero-icon">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/></svg>
                </div>
                <span class="hero-label">Token 消耗概览</span>
              </div>
            </div>

            <!-- Two-column big numbers -->
            <div class="hero-dual">
              <div class="hero-col">
                <div class="hero-col-label">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#3B82F6" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v18"/><path d="M5 12h14"/></svg>
                  <span>输入消耗</span>
                </div>
                <div class="hero-col-number">{{ inputTotal.toLocaleString() }}</div>
                <div class="hero-col-unit">tokens</div>
              </div>
              <div class="hero-col-divider"></div>
              <div class="hero-col">
                <div class="hero-col-label">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#22C55E" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14"/><path d="M5 12h14"/></svg>
                  <span>输出消耗</span>
                </div>
                <div class="hero-col-number green">{{ outputTotal.toLocaleString() }}</div>
                <div class="hero-col-unit">tokens</div>
              </div>
            </div>

            <!-- Row 1: Requests + Cost cards -->
            <div class="metric-row">
              <div class="metric-card">
                <div class="metric-card-header blue">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="22 12 18 12 15 21 9 3 6 12 2 12"/></svg>
                  <span>请求数</span>
                </div>
                <div class="metric-card-value">{{ summary.totalRequests.toLocaleString() }}</div>
              </div>
              <div class="metric-card">
                <div class="metric-card-header green">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" x2="12" y1="2" y2="22"/><path d="M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6"/></svg>
                  <span>总成本</span>
                </div>
                <div class="metric-card-value">{{ formatCost(summary.totalCost) }}</div>
              </div>
            </div>

            <!-- Row 2: 4 mini stats -->
            <div class="mini-stats">
              <div class="mini-stat">
                <div class="mini-stat-header blue">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14"/><path d="M5 12h14"/></svg>
                  <span>输入</span>
                </div>
                <div class="mini-stat-value">{{ formatTokens(summary.totalInputTokens) }}</div>
              </div>
              <div class="mini-stat">
                <div class="mini-stat-header purple">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M7 7h10M7 17h10"/><path d="m7 12 10 0"/></svg>
                  <span>输出</span>
                </div>
                <div class="mini-stat-value">{{ formatTokens(summary.totalOutputTokens) }}</div>
              </div>
              <div class="mini-stat">
                <div class="mini-stat-header amber">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M3 5v14c0 1.66 4.03 3 9 3s9-1.34 9-3V5"/><path d="M3 12c0 1.66 4.03 3 9 3s9-1.34 9-3"/></svg>
                  <span>缓存写入</span>
                </div>
                <div class="mini-stat-value">{{ formatTokens(summary.totalCacheCreationTokens) }}</div>
              </div>
              <div class="mini-stat">
                <div class="mini-stat-header emerald">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m12 3-1.912 5.813a2 2 0 0 1-1.275 1.275L3 12l5.813 1.912a2 2 0 0 1 1.275 1.275L12 21l1.912-5.813a2 2 0 0 1 1.275-1.275L21 12l-5.813-1.912a2 2 0 0 1-1.275-1.275L12 3Z"/></svg>
                  <span>缓存命中</span>
                </div>
                <div class="mini-stat-value">{{ formatTokens(summary.totalCacheReadTokens) }}</div>
              </div>
            </div>

            <!-- Cache hit rate bar -->
            <div class="hit-rate">
              <div class="hit-rate-header">
                <span class="hit-rate-label">缓存命中率</span>
                <span class="hit-rate-value">{{ hitPercent.toFixed(1) }}%</span>
              </div>
              <div class="hit-rate-track">
                <div class="hit-rate-fill" :style="{ width: hitPercent + '%' }"></div>
              </div>
            </div>
          </template>
        </div>

        <!-- Trend chart -->
        <div class="chart-card">
          <div class="chart-header">
            <h3>使用趋势</h3>
            <span class="chart-range">
              <template v-if="preset === 'custom'">{{ customStart.replace('T', ' ') }} ~ {{ customEnd.replace('T', ' ') }}</template>
              <template v-else>{{ presets.find(p => p.key === preset)?.label }}</template>
            </span>
          </div>
          <div v-if="loading" class="chart-loading">
            <div class="spinner" style="width: 32px; height: 32px;"></div>
          </div>
          <div v-else class="chart-container">
            <v-chart :option="chartOption" autoresize />
          </div>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
/* Tab bar */
.tab-bar {
  display: flex;
  gap: var(--space-1);
  background: var(--bg-secondary);
  padding: 3px;
  border-radius: var(--radius-md);
  margin-bottom: var(--space-5);
  width: fit-content;
}

.tab-btn {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: 8px 20px;
  border: none;
  border-radius: 6px;
  font-size: var(--text-sm);
  font-weight: 500;
  color: var(--text-muted);
  background: transparent;
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}

.tab-btn:hover {
  color: var(--text-secondary);
  background: var(--surface);
}

.tab-btn.active {
  color: var(--text);
  background: var(--surface);
  box-shadow: var(--shadow-xs);
}

.settings-placeholder .card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-xl);
}

/* Toolbar */
.usage-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: var(--space-3);
}

.preset-group {
  display: flex;
  gap: var(--space-1);
  background: var(--bg-secondary);
  padding: 3px;
  border-radius: var(--radius-md);
}

.preset-btn {
  padding: 6px 14px;
  border: none;
  border-radius: 6px;
  font-size: var(--text-sm);
  font-weight: 500;
  color: var(--text-muted);
  background: transparent;
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}

.preset-btn:hover {
  color: var(--text-secondary);
  background: var(--surface);
}

.preset-btn.active {
  color: var(--text);
  background: var(--surface);
  box-shadow: var(--shadow-xs);
}

/* Custom range picker */
.custom-range-panel {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  padding: var(--space-4);
  margin-bottom: var(--space-4);
}

.custom-range-fields {
  display: flex;
  align-items: flex-end;
  gap: var(--space-3);
  margin-bottom: var(--space-3);
}

.custom-field {
  flex: 1;
}

.custom-field label {
  display: block;
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-bottom: var(--space-1);
}

.custom-field input {
  width: 100%;
  padding: 8px 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--bg-secondary);
  color: var(--text);
  font-size: var(--text-sm);
  font-family: var(--font-sans);
  outline: none;
  transition: border-color var(--duration-fast);
}

.custom-field input:focus {
  border-color: var(--accent);
}

.custom-range-arrow {
  display: flex;
  align-items: center;
  justify-content: center;
  padding-bottom: 4px;
  color: var(--text-muted);
}

.custom-range-actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
}

.btn-primary, .btn-secondary {
  padding: 6px 16px;
  border-radius: var(--radius-md);
  font-size: var(--text-sm);
  font-weight: 500;
  cursor: pointer;
  border: none;
  transition: all var(--duration-fast) var(--ease-out);
}

.btn-primary {
  background: var(--accent);
  color: white;
}

.btn-primary:hover {
  background: var(--accent-hover);
}

.btn-secondary {
  background: var(--bg-secondary);
  color: var(--text-secondary);
  border: 1px solid var(--border);
}

.btn-secondary:hover {
  background: var(--surface);
}

/* Hero card */
.hero-card {
  background: linear-gradient(135deg, rgba(255, 92, 0, 0.04), var(--surface) 60%);
  border: 1px solid var(--border);
  border-radius: var(--radius-xl);
  padding: var(--space-6);
  margin-bottom: var(--space-4);
  position: relative;
  overflow: hidden;
}

.hero-loading {
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 200px;
}

.hero-header {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: var(--space-5);
}

.hero-title {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.hero-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: var(--radius-md);
  background: var(--accent-muted);
  color: var(--accent);
}

.hero-label {
  font-size: var(--text-sm);
  color: var(--text-muted);
  font-weight: 500;
}

/* Two-column big numbers */
.hero-dual {
  display: flex;
  align-items: stretch;
  margin-bottom: var(--space-5);
}

.hero-col {
  flex: 1;
}

.hero-col-label {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-sm);
  color: var(--text-muted);
  margin-bottom: var(--space-2);
}

.hero-col-label span {
  font-weight: 500;
}

.hero-col-number {
  font-size: 2.5rem;
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1.1;
  font-variant-numeric: tabular-nums;
  color: var(--text);
}

.hero-col-number.green {
  color: #22C55E;
}

.hero-col-unit {
  font-size: var(--text-sm);
  color: var(--text-muted);
  margin-top: 2px;
}

.hero-col-divider {
  width: 1px;
  background: var(--border);
  margin: 0 var(--space-6);
  align-self: stretch;
}

/* Metric row (requests + cost) */
.metric-row {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--space-3);
  margin-bottom: var(--space-3);
}

.metric-card {
  background: var(--bg-secondary);
  border: 1px solid var(--border-light);
  border-radius: var(--radius-md);
  padding: var(--space-3) var(--space-4);
}

.metric-card-header {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: var(--text-sm);
  color: var(--text-muted);
  margin-bottom: var(--space-1);
}

.metric-card-header.blue { color: #3B82F6; }
.metric-card-header.green { color: #22C55E; }

.metric-card-header span {
  color: var(--text-secondary);
}

.metric-card-value {
  font-size: var(--text-2xl);
  font-weight: 700;
  font-variant-numeric: tabular-nums;
}

/* Mini stats */
.mini-stats {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: var(--space-3);
  margin-bottom: var(--space-5);
}

@media (max-width: 640px) {
  .mini-stats { grid-template-columns: repeat(2, 1fr); }
  .metric-row { grid-template-columns: 1fr; }
  .hero-dual { flex-direction: column; gap: var(--space-4); }
  .hero-col-divider { width: 100%; height: 1px; margin: 0; }
}

.mini-stat {
  background: var(--bg-secondary);
  border: 1px solid var(--border-light);
  border-radius: var(--radius-md);
  padding: var(--space-3);
}

.mini-stat-header {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin-bottom: 6px;
}

.mini-stat-header.blue { color: #3B82F6; }
.mini-stat-header.purple { color: #A855F7; }
.mini-stat-header.amber { color: #F97316; }
.mini-stat-header.emerald { color: #22C55E; }

.mini-stat-header span {
  color: var(--text-secondary);
}

.mini-stat-value {
  font-size: var(--text-lg);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}

/* Hit rate */
.hit-rate-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: var(--space-2);
}

.hit-rate-label {
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.hit-rate-value {
  font-size: var(--text-sm);
  font-weight: 600;
  color: #22C55E;
  font-variant-numeric: tabular-nums;
}

.hit-rate-track {
  height: 8px;
  background: var(--bg-secondary);
  border-radius: var(--radius-full);
  overflow: hidden;
}

.hit-rate-fill {
  height: 100%;
  border-radius: var(--radius-full);
  background: linear-gradient(90deg, rgba(34, 197, 94, 0.8), #22C55E);
  transition: width 0.8s cubic-bezier(0.16, 1, 0.3, 1);
}

/* Chart card */
.chart-card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-xl);
  padding: var(--space-5);
}

.chart-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: var(--space-4);
}

.chart-header h3 {
  font-size: var(--text-lg);
  font-weight: 600;
  margin: 0;
}

.chart-range {
  font-size: var(--text-sm);
  color: var(--text-muted);
}

.chart-loading {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 320px;
}

.chart-container {
  height: 320px;
  width: 100%;
}

.chart-container :deep(div) {
  /* Let echarts manage its own sizing */
}
</style>
