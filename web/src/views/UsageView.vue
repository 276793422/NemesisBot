<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useToast } from '../composables/useToast'
import { useUsageChanged } from '../composables/useUsageChanged'
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
type TabId = 'usage' | 'pricing' | 'logs' | 'settings'

// 请求明细行（/api/usage/logs，A3 明细表）。
interface LogRow {
  id: number
  traceId: string
  model: string
  providerType: string
  inputTokens: number
  outputTokens: number
  cacheCreationTokens: number
  cacheReadTokens: number
  totalCostUsd: number
  inputCostUsd: number
  outputCostUsd: number
  cacheCreationCostUsd: number
  cacheReadCostUsd: number
  pricingModel: string
  latencyMs: number
  firstTokenMs: number | null
  statusCode: number
  errorMessage: string | null
  isStreaming: boolean
  sessionKey: string
  createdAt: number
}

interface LogsPage {
  logs: LogRow[]
  total: number
  page: number
  pageSize: number
}

// 价目表行（/api/usage/pricing，分层合并视图：custom > downloaded > embedded）。
interface PricingRow {
  modelId: string
  displayName: string
  inputCostPerMillion: number
  outputCostPerMillion: number
  cacheReadCostPerMillion: number
  cacheCreationCostPerMillion: number
  maxInputTokens: number | null
  maxOutputTokens: number | null
  aliases: string[]
  source: 'custom' | 'downloaded' | 'embedded'
}

// 下载元数据（在线更新状态展示）。
interface PricingMeta {
  etag: string | null
  fetchedAt: number | null
  sourceUrl: string | null
  entryCount: number | null
  // 内置兜底表的编译期来源（快照 vs 构建期下载，含日期/条目数）。
  embeddedSource?: string | null
}

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

// 价格 tab（惰性加载一次；在线更新 / 自定义条目变更后 force 重载）
const pricingRows = ref<PricingRow[]>([])
const pricingLoaded = ref(false)
const pricingLoading = ref(false)
const pricingError = ref('')
const pricingQuery = ref('')
const pricingMeta = ref<PricingMeta | null>(null)
// 当前激活模型（/api/status，best-effort），命中行高亮。
const activeModel = ref('')

// —— 价目表在线更新 + 自定义条目（A2） ——
const toast = useToast()
const pricingUpdating = ref(false)
const showCustomModal = ref(false)
const customEditingId = ref('') // 非空 = 编辑已有自定义条目
const customForm = ref({
  modelId: '',
  displayName: '',
  input: 0,
  output: 0,
  cacheRead: 0,
  cacheCreation: 0,
})
const customSaving = ref(false)

// —— 请求明细 tab（A3，2026-08-31） ——
const logsLoading = ref(false)
const logsError = ref('')
const logRows = ref<LogRow[]>([])
const logTotal = ref(0)
const logPage = ref(1)
const LOG_PAGE_SIZE = 20
// 时间范围与使用量 tab 独立（默认近 1 天，明细看近期请求）。
const logPreset = ref<Exclude<RangePreset, 'custom'>>('1d')
const logModel = ref('')
const logStatus = ref('')
const logSession = ref('')
// 点击行打开的单条详情（null = 关闭）。
const selectedLog = ref<LogRow | null>(null)

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

// —— 价格 tab ——

function switchTab(t: TabId) {
  activeTab.value = t
  if (t === 'pricing') loadPricing()
  if (t === 'logs') loadLogs()
}

// —— 请求明细 tab（A3） ——

function formatTs(ts: number): string {
  const d = new Date(ts * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

// 明细场景成本数值小，保 6 位小数才不显示 $0。
function formatCostPrecise(n: number): string {
  if (n === 0) return '$0'
  if (n < 0.01) return '$' + n.toFixed(6)
  return '$' + n.toFixed(2)
}

const totalPages = computed(() => Math.max(1, Math.ceil(logTotal.value / LOG_PAGE_SIZE)))

async function loadLogs(silent = false) {
  if (logsLoading.value) return
  logsLoading.value = true
  if (!silent) logsError.value = ''
  try {
    const { start, end } = getTimeRange(logPreset.value)
    const params = new URLSearchParams({
      start: String(start),
      end: String(end),
      page: String(logPage.value),
      page_size: String(LOG_PAGE_SIZE),
    })
    if (logModel.value.trim()) params.set('model', logModel.value.trim())
    if (logStatus.value) params.set('status', logStatus.value)
    if (logSession.value.trim()) params.set('session', logSession.value.trim())
    const page = await fetchJSON<LogsPage>(`/api/usage/logs?${params}`)
    logRows.value = page.logs
    logTotal.value = page.total
  } catch (err: any) {
    logsError.value = err?.message || String(err)
  }
  logsLoading.value = false
}

function setLogPreset(p: Exclude<RangePreset, 'custom'>) {
  logPreset.value = p
  logPage.value = 1
  loadLogs()
}

// 筛选条件变化 → 回到第 1 页重查。
function applyLogFilters() {
  logPage.value = 1
  loadLogs()
}

function prevLogPage() {
  if (logPage.value > 1) {
    logPage.value--
    loadLogs()
  }
}

function nextLogPage() {
  if (logPage.value < totalPages.value) {
    logPage.value++
    loadLogs()
  }
}

// 明细行有新写入（gateway 轮询 usage.db data_version → SSE usage-changed）→
// 静默刷新当前 tab；使用量 tab 同步静默重拉。
useUsageChanged(() => {
  if (activeTab.value === 'logs') loadLogs(true)
  else if (activeTab.value === 'usage') loadData(true)
})

async function loadPricing(force = false) {
  if ((pricingLoaded.value && !force) || pricingLoading.value) return
  pricingLoading.value = true
  pricingError.value = ''
  try {
    const [status, pricingResp] = await Promise.all([
      fetch('/api/status')
        .then(r => (r.ok ? r.json() : {}))
        .catch(() => ({}) as Record<string, unknown>),
      fetch('/api/usage/pricing').then(async r => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`)
        const j = await r.json()
        if (j.error) throw new Error(j.error)
        return j as { data: PricingRow[]; meta: PricingMeta | null }
      }),
    ])
    pricingRows.value = [...pricingResp.data].sort((a, b) => a.modelId.localeCompare(b.modelId))
    pricingMeta.value = pricingResp.meta
    const m = (status as Record<string, unknown>)?.model_name
    activeModel.value = typeof m === 'string' ? m : ''
    pricingLoaded.value = true
  } catch (err: any) {
    pricingError.value = err?.message || String(err)
  }
  pricingLoading.value = false
}

// —— 在线更新（LiteLLM 主源；失败保留旧表） ——

async function updatePricing() {
  if (pricingUpdating.value) return
  pricingUpdating.value = true
  try {
    const r = await fetch('/api/usage/pricing/update', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: '{}',
    })
    const j = await r.json()
    if (j.error) throw new Error(j.error)
    const d = j.data as { updated: boolean; entryCount: number }
    toast.success(d.updated ? `价目表已更新（${d.entryCount} 个模型）` : '表已是最新（304 NotModified）')
    await loadPricing(true)
  } catch (err: any) {
    toast.error('价目表更新失败：' + (err?.message || String(err)))
  }
  pricingUpdating.value = false
}

// —— 自定义条目（最高查表优先级） ——

function openCustomCreate() {
  customEditingId.value = ''
  customForm.value = { modelId: '', displayName: '', input: 0, output: 0, cacheRead: 0, cacheCreation: 0 }
  showCustomModal.value = true
}

function openCustomEdit(row: PricingRow) {
  customEditingId.value = row.modelId
  customForm.value = {
    modelId: row.modelId,
    displayName: row.displayName,
    input: row.inputCostPerMillion,
    output: row.outputCostPerMillion,
    cacheRead: row.cacheReadCostPerMillion,
    cacheCreation: row.cacheCreationCostPerMillion,
  }
  showCustomModal.value = true
}

async function saveCustom() {
  if (customSaving.value) return
  const f = customForm.value
  if (!f.modelId.trim()) {
    toast.error('模型名不能为空')
    return
  }
  customSaving.value = true
  try {
    const r = await fetch('/api/usage/pricing/custom', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        model_id: f.modelId.trim(),
        display_name: f.displayName.trim() || f.modelId.trim(),
        input_cost_per_million: f.input,
        output_cost_per_million: f.output,
        cache_read_cost_per_million: f.cacheRead,
        cache_creation_cost_per_million: f.cacheCreation,
        max_input_tokens: null,
        max_output_tokens: null,
        aliases: [],
      }),
    })
    const j = await r.json()
    if (j.error) throw new Error(j.error)
    toast.success(`自定义条目已保存：${f.modelId.trim()}`)
    showCustomModal.value = false
    await loadPricing(true)
  } catch (err: any) {
    toast.error('保存失败：' + (err?.message || String(err)))
  }
  customSaving.value = false
}

async function removeCustom(row: PricingRow) {
  if (!confirm(`删除自定义条目 ${row.modelId}？（下载层/内置层继续兜底）`)) return
  try {
    const r = await fetch('/api/usage/pricing/custom/remove', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ model_id: row.modelId }),
    })
    const j = await r.json()
    if (j.error) throw new Error(j.error)
    toast.success(`已删除：${row.modelId}`)
    await loadPricing(true)
  } catch (err: any) {
    toast.error('删除失败：' + (err?.message || String(err)))
  }
}

const SOURCE_LABELS: Record<string, string> = {
  custom: '自定义',
  downloaded: '下载',
  embedded: '内置',
}

function formatFetchedAt(ts: number | null): string {
  if (!ts) return '从未更新'
  const d = new Date(ts * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

// 与后端 PricingTable::lookup 同序的匹配：精确 id → 别名 → 去掉 provider 前缀的裸名。
function matchesActiveModel(row: PricingRow): boolean {
  if (!activeModel.value) return false
  const m = activeModel.value.trim()
  if (!m) return false
  if (m === row.modelId || row.aliases.includes(m)) return true
  const bare = m.split('/').pop() || m
  return bare === row.modelId || row.aliases.includes(bare)
}

// 全量精简表后内置层 ~2800 行：无分页全量渲染会拖垮 DOM——搜索过滤
// 在全量上算，但只渲染前 PRICING_RENDER_CAP 行，尾部给诚实注记。
const PRICING_RENDER_CAP = 200

const filteredPricingAll = computed(() => {
  const q = pricingQuery.value.trim().toLowerCase()
  if (!q) return pricingRows.value
  return pricingRows.value.filter(
    r =>
      r.modelId.toLowerCase().includes(q) ||
      r.displayName.toLowerCase().includes(q) ||
      r.aliases.some(a => a.toLowerCase().includes(q)),
  )
})
const filteredPricing = computed(() => filteredPricingAll.value.slice(0, PRICING_RENDER_CAP))
const pricingHiddenCount = computed(() =>
  Math.max(0, filteredPricingAll.value.length - filteredPricing.value.length),
)

// embeddedSource 是构建期溯源长文案（快照描述/下载 URL + 细节）——工具栏
// 只放短摘要（类型 + 日期）防 UI 变形，完整原文进 title 悬浮提示。
const pricingSourceShort = computed(() => {
  const src = pricingMeta.value?.embeddedSource || ''
  if (!src) return '内置价目表'
  const date = src.match(/\d{4}-\d{2}-\d{2}/)?.[0]
  if (src.startsWith('bundled')) return date ? `内置快照 · ${date}` : '内置快照'
  if (src.startsWith('downloaded')) return date ? `在线更新 · ${date}` : '在线更新'
  return src.length <= 40 ? src : `${src.slice(0, 37)}…`
})

function formatPrice(n: number): string {
  if (n === 0) return '$0'
  if (n < 0.01) return '$' + n.toFixed(3)
  return '$' + n.toFixed(2)
}

function formatContext(n: number | null): string {
  if (!n) return '—'
  return n >= 1000 ? `${Math.round(n / 1000)}K` : String(n)
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

function getTimeRange(forPreset: RangePreset = preset.value): { start: number; end: number } {
  const end = Math.floor(Date.now() / 1000)
  if (forPreset === 'custom') {
    if (customStart.value && customEnd.value) {
      return {
        start: Math.floor(new Date(customStart.value).getTime() / 1000),
        end: Math.floor(new Date(customEnd.value).getTime() / 1000),
      }
    }
    return { start: end - 86400, end }
  }
  if (forPreset === 'today') {
    const now = new Date()
    const startOfDay = new Date(now.getFullYear(), now.getMonth(), now.getDate())
    return { start: Math.floor(startOfDay.getTime() / 1000), end }
  }
  const days = parseInt(forPreset)
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

async function loadData(silent = false) {
  if (!silent) loading.value = true
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
      cacheHitRate: summaryData.cacheHitRate,
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
  if (!silent) loading.value = false
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
        <button class="tab-btn" :class="{ active: activeTab === 'usage' }" @click="switchTab('usage')">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 20V10M12 20V4M6 20v-6"/></svg>
          使用量
        </button>
        <button class="tab-btn" :class="{ active: activeTab === 'pricing' }" @click="switchTab('pricing')">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 1v22M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6"/></svg>
          价格
        </button>
        <button class="tab-btn" :class="{ active: activeTab === 'logs' }" @click="switchTab('logs')">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12h.01M3 18h.01M3 6h.01M8 12h13M8 18h13M8 6h13"/></svg>
          请求明细
        </button>
        <button class="tab-btn" :class="{ active: activeTab === 'settings' }" @click="switchTab('settings')">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/></svg>
          设置
        </button>
      </div>

      <!-- Pricing tab：嵌入式价目表（静态数据，惰性加载一次） -->
      <div v-if="activeTab === 'pricing'" class="pricing-tab">
        <div class="usage-toolbar">
          <div class="pricing-filter">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/></svg>
            <input
              v-model="pricingQuery"
              class="form-input pricing-search"
              type="text"
              placeholder="搜索模型 / 别名…"
              data-testid="pricing-search"
            />
          </div>
          <div class="pricing-toolbar-right">
            <span v-if="activeModel" class="pricing-active-model">
              当前模型：<strong>{{ activeModel }}</strong>
            </span>
            <span
              class="pricing-meta"
              data-testid="pricing-meta"
              :title="pricingMeta?.embeddedSource || '内置价目表'"
            >
              {{ pricingMeta && pricingMeta.entryCount ? `${pricingMeta.entryCount} 条 · ${formatFetchedAt(pricingMeta.fetchedAt)}` : pricingSourceShort }}
            </span>
            <button
              type="button"
              class="btn-primary"
              data-testid="pricing-update"
              :disabled="pricingUpdating"
              @click="updatePricing()"
            >{{ pricingUpdating ? '更新中…' : '在线更新' }}</button>
            <button type="button" class="btn-secondary" data-testid="pricing-add-custom" @click="openCustomCreate()">+ 自定义</button>
          </div>
        </div>

        <div v-if="pricingLoading" class="pricing-state">加载中…</div>
        <div v-else-if="pricingError" class="pricing-state is-error">
          加载失败：{{ pricingError }}
          <button type="button" class="btn btn-sm" style="margin-left: var(--space-3)" @click="pricingLoaded = false; loadPricing()">重试</button>
        </div>
        <div v-else-if="!filteredPricing.length" class="pricing-state">
          {{ pricingRows.length ? '没有匹配的模型' : '价目表为空' }}
        </div>
        <div v-else class="card">
          <div class="pricing-table-wrap">
            <table class="pricing-table" data-testid="pricing-table">
              <thead>
                <tr>
                  <th>模型</th>
                  <th>来源</th>
                  <th class="num">输入 $/M</th>
                  <th class="num">输出 $/M</th>
                  <th class="num">缓存读 $/M</th>
                  <th class="num">缓存写 $/M</th>
                  <th class="num">上下文</th>
                  <th>别名</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="row in filteredPricing"
                  :key="row.modelId"
                  :class="{ 'is-active-model': matchesActiveModel(row) }"
                >
                  <td>
                    <div class="pricing-model-name">
                      {{ row.displayName }}
                      <span v-if="matchesActiveModel(row)" class="pricing-active-badge">当前</span>
                    </div>
                    <div class="pricing-model-id">{{ row.modelId }}</div>
                  </td>
                  <td>
                    <span class="pricing-source" :class="`is-${row.source}`">{{ SOURCE_LABELS[row.source] || row.source }}</span>
                  </td>
                  <td class="num">{{ formatPrice(row.inputCostPerMillion) }}</td>
                  <td class="num">{{ formatPrice(row.outputCostPerMillion) }}</td>
                  <td class="num">{{ formatPrice(row.cacheReadCostPerMillion) }}</td>
                  <td class="num">{{ formatPrice(row.cacheCreationCostPerMillion) }}</td>
                  <td class="num">{{ formatContext(row.maxInputTokens) }}</td>
                  <td class="pricing-aliases">
                    <span v-for="a in row.aliases" :key="a" class="pricing-alias">{{ a }}</span>
                    <span v-if="!row.aliases.length" class="text-muted">—</span>
                  </td>
                  <td class="pricing-actions">
                    <template v-if="row.source === 'custom'">
                      <button type="button" class="pricing-action-btn" @click="openCustomEdit(row)">编辑</button>
                      <button type="button" class="pricing-action-btn is-danger" @click="removeCustom(row)">删除</button>
                    </template>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
          <div v-if="pricingHiddenCount > 0" class="pricing-state" data-testid="pricing-cap-note">
            已显示前 {{ filteredPricing.length }} 条，还有 {{ pricingHiddenCount }} 条未列出——用上方搜索缩小范围。
          </div>
        </div>

        <!-- 自定义条目编辑弹窗 -->
        <div v-if="showCustomModal" class="modal-backdrop" @click.self="showCustomModal = false">
          <div class="modal" style="max-width: 480px;">
            <div class="modal-header"><h3>{{ customEditingId ? '编辑自定义条目' : '新增自定义条目' }}</h3></div>
            <div class="modal-body">
              <div class="custom-form">
                <label class="custom-form-field">
                  <span>模型名（与 model add 一致，必填）</span>
                  <input v-model="customForm.modelId" type="text" :disabled="!!customEditingId" placeholder="如 zhipu/glm-4.7" data-testid="custom-model-id" />
                </label>
                <label class="custom-form-field">
                  <span>显示名（可选）</span>
                  <input v-model="customForm.displayName" type="text" placeholder="缺省用模型名" />
                </label>
                <div class="custom-form-grid">
                  <label class="custom-form-field">
                    <span>输入 $/M</span>
                    <input v-model.number="customForm.input" type="number" min="0" step="any" data-testid="custom-input-price" />
                  </label>
                  <label class="custom-form-field">
                    <span>输出 $/M</span>
                    <input v-model.number="customForm.output" type="number" min="0" step="any" data-testid="custom-output-price" />
                  </label>
                  <label class="custom-form-field">
                    <span>缓存读 $/M</span>
                    <input v-model.number="customForm.cacheRead" type="number" min="0" step="any" />
                  </label>
                  <label class="custom-form-field">
                    <span>缓存写 $/M</span>
                    <input v-model.number="customForm.cacheCreation" type="number" min="0" step="any" />
                  </label>
                </div>
                <p class="custom-form-hint">自定义条目查表优先级最高；同名会覆盖下载层/内置层。</p>
              </div>
            </div>
            <div class="modal-footer">
              <button type="button" class="btn-secondary" @click="showCustomModal = false">取消</button>
              <button type="button" class="btn-primary" :disabled="customSaving" data-testid="custom-save" @click="saveCustom()">
                {{ customSaving ? '保存中…' : '保存' }}
              </button>
            </div>
          </div>
        </div>
      </div>

      <!-- Logs tab：请求明细（A3，时间/模型/状态/会话过滤 + 单条详情） -->
      <div v-if="activeTab === 'logs'" class="logs-tab">
        <div class="usage-toolbar">
          <div class="preset-group">
            <button
              v-for="p in presets"
              :key="p.key"
              class="preset-btn"
              :class="{ active: logPreset === p.key }"
              @click="setLogPreset(p.key)"
            >{{ p.label }}</button>
          </div>
          <div class="logs-filters">
            <input
              v-model="logModel"
              class="form-input logs-filter-input"
              type="text"
              placeholder="模型（子串）"
              data-testid="logs-filter-model"
              @keyup.enter="applyLogFilters"
            />
            <select v-model="logStatus" class="form-input logs-filter-select" data-testid="logs-filter-status" @change="applyLogFilters">
              <option value="">全部状态</option>
              <option v-for="s in [200, 400, 401, 403, 429, 500, 502, 503]" :key="s" :value="String(s)">{{ s }}</option>
            </select>
            <input
              v-model="logSession"
              class="form-input logs-filter-input"
              type="text"
              placeholder="会话（子串）"
              data-testid="logs-filter-session"
              @keyup.enter="applyLogFilters"
            />
            <button type="button" class="btn-secondary" data-testid="logs-apply" @click="applyLogFilters">筛选</button>
          </div>
        </div>

        <div v-if="logsLoading && !logRows.length" class="pricing-state">加载中…</div>
        <div v-else-if="logsError" class="pricing-state is-error">
          加载失败：{{ logsError }}
          <button type="button" class="btn btn-sm" style="margin-left: var(--space-3)" @click="logsError = ''; loadLogs()">重试</button>
        </div>
        <div v-else-if="!logRows.length" class="pricing-state">所选范围内没有请求记录</div>
        <div v-else class="card">
          <div class="pricing-table-wrap">
            <table class="pricing-table" data-testid="logs-table">
              <thead>
                <tr>
                  <th>时间</th>
                  <th>模型</th>
                  <th>状态</th>
                  <th class="num">输入</th>
                  <th class="num">输出</th>
                  <th class="num">成本</th>
                  <th class="num">延迟</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="row in logRows"
                  :key="row.id"
                  class="logs-row"
                  data-testid="logs-row"
                  @click="selectedLog = row"
                >
                  <td class="logs-ts">{{ formatTs(row.createdAt) }}</td>
                  <td>
                    <div class="pricing-model-name">{{ row.model }}</div>
                    <div class="pricing-model-id">{{ row.pricingModel || '未计价' }}</div>
                  </td>
                  <td>
                    <span class="logs-status" :class="row.statusCode === 200 ? 'is-ok' : 'is-fail'">{{ row.statusCode }}</span>
                  </td>
                  <td class="num">{{ formatTokens(row.inputTokens) }}</td>
                  <td class="num">{{ formatTokens(row.outputTokens) }}</td>
                  <td class="num">{{ formatCostPrecise(row.totalCostUsd) }}</td>
                  <td class="num">{{ row.latencyMs }} ms</td>
                </tr>
              </tbody>
            </table>
          </div>
          <div class="logs-pagination" data-testid="logs-pagination">
            <span class="logs-pagination-info">共 {{ logTotal }} 条</span>
            <div class="logs-pagination-ctrl">
              <button type="button" class="btn-secondary" :disabled="logPage <= 1" @click="prevLogPage()">上一页</button>
              <span>第 {{ logPage }} / {{ totalPages }} 页</span>
              <button type="button" class="btn-secondary" :disabled="logPage >= totalPages" @click="nextLogPage()">下一页</button>
            </div>
          </div>
        </div>

        <!-- 单条详情弹窗 -->
        <div v-if="selectedLog" class="modal-backdrop" @click.self="selectedLog = null">
          <div class="modal" style="max-width: 560px;" data-testid="log-detail-modal">
            <div class="modal-header"><h3>请求明细</h3></div>
            <div class="modal-body">
              <div class="log-detail-grid">
                <div class="log-detail-row">
                  <span class="log-detail-label">模型</span>
                  <span class="log-detail-value">{{ selectedLog.model }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">计价模型</span>
                  <span class="log-detail-value">{{ selectedLog.pricingModel || '未命中价目表' }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">会话</span>
                  <span class="log-detail-value">{{ selectedLog.sessionKey || '—' }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">Trace</span>
                  <span class="log-detail-value log-detail-mono">{{ selectedLog.traceId }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">时间</span>
                  <span class="log-detail-value">{{ formatTs(selectedLog.createdAt) }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">状态</span>
                  <span class="log-detail-value">
                    <span class="logs-status" :class="selectedLog.statusCode === 200 ? 'is-ok' : 'is-fail'">{{ selectedLog.statusCode }}</span>
                  </span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">延迟</span>
                  <span class="log-detail-value">{{ selectedLog.latencyMs }} ms</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">首 Token</span>
                  <span class="log-detail-value">
                    <template v-if="selectedLog.firstTokenMs !== null">{{ selectedLog.firstTokenMs }} ms</template>
                    <template v-else>—</template>
                  </span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">Tokens 输入</span>
                  <span class="log-detail-value log-detail-mono">{{ selectedLog.inputTokens.toLocaleString() }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">Tokens 输出</span>
                  <span class="log-detail-value log-detail-mono">{{ selectedLog.outputTokens.toLocaleString() }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">Tokens 缓存写</span>
                  <span class="log-detail-value log-detail-mono">{{ selectedLog.cacheCreationTokens.toLocaleString() }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">Tokens 缓存读</span>
                  <span class="log-detail-value log-detail-mono">{{ selectedLog.cacheReadTokens.toLocaleString() }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">成本 总计</span>
                  <span class="log-detail-value log-detail-mono">{{ formatCostPrecise(selectedLog.totalCostUsd) }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">成本 输入</span>
                  <span class="log-detail-value log-detail-mono">{{ formatCostPrecise(selectedLog.inputCostUsd) }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">成本 输出</span>
                  <span class="log-detail-value log-detail-mono">{{ formatCostPrecise(selectedLog.outputCostUsd) }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">成本 缓存写</span>
                  <span class="log-detail-value log-detail-mono">{{ formatCostPrecise(selectedLog.cacheCreationCostUsd) }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">成本 缓存读</span>
                  <span class="log-detail-value log-detail-mono">{{ formatCostPrecise(selectedLog.cacheReadCostUsd) }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">流式</span>
                  <span class="log-detail-value">{{ selectedLog.isStreaming ? '是' : '否' }}</span>
                </div>
                <div class="log-detail-row">
                  <span class="log-detail-label">Provider</span>
                  <span class="log-detail-value">{{ selectedLog.providerType || '—' }}</span>
                </div>
                <div v-if="selectedLog.errorMessage" class="log-detail-row">
                  <span class="log-detail-label">错误信息</span>
                  <span class="log-detail-value log-detail-error">{{ selectedLog.errorMessage }}</span>
                </div>
              </div>
            </div>
            <div class="modal-footer">
              <button type="button" class="btn-secondary" @click="selectedLog = null">关闭</button>
            </div>
          </div>
        </div>
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

/* —— 价格 tab —— */
.pricing-tab {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.pricing-filter {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  color: var(--text-muted);
}

.pricing-search {
  width: 260px;
}

.pricing-active-model {
  font-size: var(--text-sm);
  color: var(--text-muted);
}

.pricing-active-model strong {
  color: var(--accent);
}

.pricing-state {
  padding: var(--space-8);
  text-align: center;
  color: var(--text-muted);
}

.pricing-state.is-error {
  color: var(--danger, #d64545);
}

.pricing-table-wrap {
  overflow-x: auto;
}

.pricing-table {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--text-sm);
}

.pricing-table th,
.pricing-table td {
  text-align: left;
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--border-light);
  white-space: nowrap;
}

.pricing-table th {
  color: var(--text-muted);
  font-weight: 500;
  font-size: var(--text-xs);
}

.pricing-table td.num,
.pricing-table th.num {
  text-align: right;
  font-variant-numeric: tabular-nums;
}

.pricing-table tbody tr:hover {
  background: var(--surface-alt);
}

.pricing-table tbody tr.is-active-model {
  background: color-mix(in srgb, var(--accent) 8%, transparent);
}

.pricing-table tbody tr.is-active-model .pricing-model-name {
  color: var(--accent);
}

.pricing-active-badge {
  display: inline-block;
  margin-left: var(--space-2);
  font-size: var(--text-xs);
  color: var(--accent);
  border: 1px solid var(--accent);
  border-radius: var(--radius-full, 999px);
  padding: 0 var(--space-2);
  line-height: 1.4;
}

.pricing-model-name {
  font-weight: 500;
}

.pricing-model-id {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.pricing-aliases {
  max-width: 280px;
  overflow: hidden;
  text-overflow: ellipsis;
}

.pricing-alias {
  display: inline-block;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--text-muted);
  background: var(--surface-alt);
  border-radius: var(--radius-sm);
  padding: 0 var(--space-2);
  margin-right: var(--space-1);
}

/* —— 价目表在线更新 + 自定义条目（A2） —— */
.pricing-toolbar-right {
  display: flex;
  align-items: center;
  gap: var(--space-3);
}

.pricing-meta {
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.pricing-source {
  display: inline-block;
  font-size: var(--text-xs);
  border-radius: var(--radius-full, 999px);
  padding: 0 var(--space-2);
  line-height: 1.6;
  border: 1px solid var(--border);
  color: var(--text-muted);
}

.pricing-source.is-custom {
  color: var(--accent);
  border-color: var(--accent);
}

.pricing-source.is-downloaded {
  color: #3B82F6;
  border-color: rgba(59, 130, 246, 0.5);
}

.pricing-actions {
  white-space: nowrap;
}

.pricing-action-btn {
  border: none;
  background: transparent;
  color: var(--text-muted);
  font-size: var(--text-xs);
  cursor: pointer;
  padding: 2px var(--space-2);
  border-radius: var(--radius-sm);
  transition: all var(--duration-fast) var(--ease-out);
}

.pricing-action-btn:hover {
  color: var(--text);
  background: var(--surface-alt);
}

.pricing-action-btn.is-danger:hover {
  color: var(--danger, #d64545);
}

.custom-form {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.custom-form-field {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.custom-form-field span {
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.custom-form-field input {
  padding: 8px 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--bg-secondary);
  color: var(--text);
  font-size: var(--text-sm);
  outline: none;
  transition: border-color var(--duration-fast);
}

.custom-form-field input:focus {
  border-color: var(--accent);
}

.custom-form-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--space-3);
}

.custom-form-hint {
  font-size: var(--text-xs);
  color: var(--text-muted);
  margin: 0;
}

/* —— 请求明细 tab（A3） —— */
.logs-tab {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.logs-filters {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.logs-filter-input {
  width: 180px;
}

.logs-filter-select {
  width: 110px;
}

.logs-row {
  cursor: pointer;
}

.logs-ts {
  font-variant-numeric: tabular-nums;
  color: var(--text-muted);
}

.logs-status {
  display: inline-block;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  border-radius: var(--radius-full, 999px);
  padding: 0 var(--space-2);
  line-height: 1.6;
}

.logs-status.is-ok {
  color: #22C55E;
  border: 1px solid rgba(34, 197, 94, 0.4);
}

.logs-status.is-fail {
  color: var(--danger, #d64545);
  border: 1px solid rgba(214, 69, 69, 0.4);
}

.logs-pagination {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding-top: var(--space-3);
  font-size: var(--text-sm);
  color: var(--text-muted);
}

.logs-pagination-ctrl {
  display: flex;
  align-items: center;
  gap: var(--space-3);
}

.logs-pagination button:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.log-detail-grid {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.log-detail-row {
  display: flex;
  flex-direction: row;
  align-items: baseline;
  gap: var(--space-4);
}

.log-detail-label {
  flex: 0 0 96px;
  font-size: var(--text-xs);
  color: var(--text-muted);
}

.log-detail-value {
  font-size: var(--text-sm);
  color: var(--text);
  word-break: break-all;
}

.log-detail-mono {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
}

.log-detail-error {
  color: var(--danger, #d64545);
}
</style>
