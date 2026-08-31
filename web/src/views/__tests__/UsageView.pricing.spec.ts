import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// A⑥/A⑦（usage-pricing goal，2026-08-31）：使用统计页「价格」tab ——
// 嵌入式价目表渲染、当前模型高亮（alias / provider 前缀匹配）、
// 搜索过滤、惰性加载（不切 tab 不拉取）、失败态重试。
// 后端 /api/usage/pricing 由 api_usage_extra_tests.rs 钉住；本 spec 钉展示契约。

const PRICING = [
  {
    modelId: 'deepseek-chat',
    displayName: 'DeepSeek V3',
    inputCostPerMillion: 0.28,
    outputCostPerMillion: 0.42,
    cacheReadCostPerMillion: 0.03,
    cacheCreationCostPerMillion: 0,
    maxInputTokens: 65536,
    maxOutputTokens: 8192,
    aliases: ['deepseek/deepseek-chat'],
  },
  {
    modelId: 'gpt-4o',
    displayName: 'GPT-4o',
    inputCostPerMillion: 2.5,
    outputCostPerMillion: 10,
    cacheReadCostPerMillion: 1.25,
    cacheCreationCostPerMillion: 0,
    maxInputTokens: null,
    maxOutputTokens: null,
    aliases: [],
  },
]

// loadData 期望的 /api/usage/summary 响应形状（ApiSummary，注意 totalCostUsd）。
const SUMMARY = {
  totalRequests: 3,
  successCount: 3,
  totalInputTokens: 100,
  totalOutputTokens: 50,
  totalCacheCreationTokens: 0,
  totalCacheReadTokens: 0,
  totalCostUsd: 0.01,
  avgLatencyMs: 120,
  cacheHitRate: 0.5,
}

const fetchMock = vi.fn()

// vue-echarts 在 <script setup> 中是直接引用，字符串 stub 拦不住；
// 直接 mock 掉（jsdom 无 ResizeObserver / canvas）。
vi.mock('vue-echarts', () => ({
  default: { name: 'VChart', props: ['option', 'autoresize'], template: '<div class="v-chart-stub" />' },
}))

function jsonResponse(body: unknown, ok = true, status = 200) {
  return { ok, status, json: async () => body }
}

function route(url: string) {
  if (url.startsWith('/api/usage/pricing')) return jsonResponse({ status: 'success', data: PRICING })
  if (url.startsWith('/api/status')) return jsonResponse({ model_name: 'deepseek/deepseek-chat', version: 'test' })
  if (url.startsWith('/api/usage/summary')) return jsonResponse({ data: SUMMARY })
  if (url.startsWith('/api/usage/trends')) return jsonResponse({ data: [] })
  return jsonResponse({ data: null })
}

import UsageView from '../UsageView.vue'

beforeEach(() => {
  fetchMock.mockReset()
  fetchMock.mockImplementation((url: string) => Promise.resolve(route(url)))
  vi.stubGlobal('fetch', fetchMock)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

async function mountView() {
  const w = mount(UsageView)
  await flushPromises()
  return w
}

function clickPricingTab(w: ReturnType<typeof mount>) {
  const btn = w.findAll('button').find(b => b.text().includes('价格'))!
  return btn.trigger('click').then(() => flushPromises())
}

function pricedRows(w: ReturnType<typeof mount>) {
  return w.find('[data-testid="pricing-table"]').findAll('tbody tr')
}

describe('UsageView 价格 tab（A⑥）', () => {
  it('惰性加载：不切到价格 tab 不拉取价目表', async () => {
    const w = await mountView()
    const urls = fetchMock.mock.calls.map(c => c[0])
    expect(urls.some((u: string) => u.startsWith('/api/usage/pricing'))).toBe(false)
    // 切过去才拉。
    await clickPricingTab(w)
    const after = fetchMock.mock.calls.map(c => c[0])
    expect(after.some((u: string) => u.startsWith('/api/usage/pricing'))).toBe(true)
    expect(after.some((u: string) => u.startsWith('/api/status'))).toBe(true)
  })

  it('渲染价目表 + 当前模型高亮（alias 匹配带 provider 前缀）', async () => {
    const w = await mountView()
    await clickPricingTab(w)

    const table = w.find('[data-testid="pricing-table"]')
    expect(table.exists()).toBe(true)
    // /api/status 的 model_name 通过别名命中 deepseek 行。
    const rows = pricedRows(w)
    expect(rows).toHaveLength(2)
    const dsRow = rows[0]
    expect(dsRow.classes()).toContain('is-active-model')
    expect(dsRow.text()).toContain('当前')
    expect(dsRow.text()).toContain('$0.28')
    expect(dsRow.text()).toContain('$0.03')
    expect(dsRow.text()).toContain('66K')
    // gpt-4o 行不高亮；空上下文显示 —；$2.50 两位小数。
    const gptRow = rows[1]
    expect(gptRow.classes()).not.toContain('is-active-model')
    expect(gptRow.text()).toContain('$2.50')
    expect(gptRow.text()).toContain('—')
    // 头部标明当前模型。
    expect(w.text()).toContain('deepseek/deepseek-chat')
  })

  it('搜索过滤：按 displayName 命中，别名也参与匹配', async () => {
    const w = await mountView()
    await clickPricingTab(w)
    const search = w.find('[data-testid="pricing-search"]')

    await search.setValue('gpt')
    let rows = pricedRows(w)
    expect(rows).toHaveLength(1)
    expect(rows[0].text()).toContain('GPT-4o')
    expect(w.text()).not.toContain('DeepSeek V3')

    // 别名反查。
    await search.setValue('deepseek/deepseek-chat')
    rows = pricedRows(w)
    expect(rows).toHaveLength(1)
    expect(rows[0].text()).toContain('DeepSeek V3')

    // 无命中提示。
    await search.setValue('zzzqqqxxx')
    expect(w.text()).toContain('没有匹配的模型')
  })

  it('失败态：显示错误 + 重试成功后渲染', async () => {
    fetchMock.mockImplementation((url: string) => {
      if (url.startsWith('/api/usage/pricing')) return Promise.resolve(jsonResponse({}, false, 503))
      return Promise.resolve(route(url))
    })
    const w = await mountView()
    await clickPricingTab(w)
    expect(w.text()).toContain('加载失败：HTTP 503')
    expect(w.find('[data-testid="pricing-table"]').exists()).toBe(false)

    // 修好端点 → 重试成功。
    fetchMock.mockImplementation((url: string) => Promise.resolve(route(url)))
    const retry = w.findAll('button').find(b => b.text().includes('重试'))!
    await retry.trigger('click')
    await flushPromises()
    expect(pricedRows(w)).toHaveLength(2)
  })
})
