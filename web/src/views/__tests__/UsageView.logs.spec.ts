import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// A3 请求明细（2026-08-31）：使用统计页「请求明细」tab ——
// 惰性加载、表格渲染（时间/模型/状态/tokens/成本/延迟）、
// 模型/状态/会话过滤 + 分页、单条详情弹窗（分项成本 + firstTokenMs）。
// 后端 /api/usage/logs(/:id) 由 api_usage_extra_tests.rs 钉住；本 spec 钉展示契约。

const SUMMARY = {
  totalRequests: 1,
  successCount: 1,
  totalInputTokens: 100,
  totalOutputTokens: 50,
  totalCacheCreationTokens: 0,
  totalCacheReadTokens: 0,
  totalCostUsd: 0.01,
  avgLatencyMs: 120,
  cacheHitRate: 0,
}

function logRow(overrides: Record<string, unknown> = {}) {
  return {
    id: 1,
    traceId: 'trace-abc',
    model: 'zhipu/glm-4.7',
    providerType: 'openai',
    inputTokens: 1200,
    outputTokens: 340,
    cacheCreationTokens: 0,
    cacheReadTokens: 256,
    totalCostUsd: 0.0012,
    inputCostUsd: 0.0008,
    outputCostUsd: 0.0004,
    cacheCreationCostUsd: 0,
    cacheReadCostUsd: 0,
    pricingModel: 'glm-4.7',
    latencyMs: 1520,
    firstTokenMs: 320,
    statusCode: 200,
    errorMessage: null,
    isStreaming: false,
    sessionKey: 'direct-a',
    createdAt: 1756500000,
    ...overrides,
  }
}

const PAGE1 = {
  logs: [logRow(), logRow({ id: 2, traceId: 'trace-err', model: 'gpt-4', pricingModel: 'gpt-4', statusCode: 500, errorMessage: 'boom', firstTokenMs: null, sessionKey: 'rpc:node-b/x' })],
  total: 41,
  page: 1,
  pageSize: 20,
}

const fetchMock = vi.fn()

vi.mock('vue-echarts', () => ({
  default: { name: 'VChart', props: ['option', 'autoresize'], template: '<div class="v-chart-stub" />' },
}))

function jsonResponse(body: unknown, ok = true, status = 200) {
  return { ok, status, json: async () => body }
}

function route(url: string) {
  if (url.startsWith('/api/usage/logs?')) return jsonResponse({ status: 'success', data: PAGE1 })
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

async function clickLogsTab(w: ReturnType<typeof mount>) {
  const btn = w.findAll('button').find(b => b.text().includes('请求明细'))!
  await btn.trigger('click')
  await flushPromises()
}

function logRows(w: ReturnType<typeof mount>) {
  return w.find('[data-testid="logs-table"]').findAll('tbody tr')
}

describe('UsageView 请求明细 tab（A3）', () => {
  it('惰性加载：不切 tab 不拉 /api/usage/logs，切过去带时间范围参数', async () => {
    const w = await mountView()
    expect(fetchMock.mock.calls.some(c => String(c[0]).startsWith('/api/usage/logs'))).toBe(false)

    await clickLogsTab(w)
    const call = fetchMock.mock.calls.find(c => String(c[0]).startsWith('/api/usage/logs'))!
    const url = String(call[0])
    expect(url).toContain('page_size=20')
    expect(url).toContain('page=1')
    expect(url).toContain('start=')
    expect(url).toContain('end=')
  })

  it('表格渲染：时间/模型/计价名/状态徽标/tokens/成本/延迟', async () => {
    const w = await mountView()
    await clickLogsTab(w)

    const rows = logRows(w)
    expect(rows).toHaveLength(2)
    expect(rows[0].text()).toContain('zhipu/glm-4.7')
    expect(rows[0].text()).toContain('glm-4.7') // 计价名副行
    expect(rows[0].text()).toContain('200')
    expect(rows[0].text()).toContain('1.2K') // 输入 tokens 格式化
    expect(rows[0].text()).toContain('$0.001200') // 小成本 6 位小数
    expect(rows[0].text()).toContain('1520 ms')
    // 失败行红色徽标 + 未命中的 firstTokenMs 不影响列表。
    expect(rows[1].text()).toContain('500')
    // 分页信息。
    expect(w.find('[data-testid="logs-pagination"]').text()).toContain('共 41 条')
    expect(w.find('[data-testid="logs-pagination"]').text()).toContain('第 1 / 3 页')
  })

  it('过滤：模型/状态/会话参数进 query，筛选后回到第 1 页', async () => {
    const w = await mountView()
    await clickLogsTab(w)

    await w.find('[data-testid="logs-filter-model"]').setValue('gpt')
    await w.find('[data-testid="logs-filter-status"]').setValue('500')
    await w.find('[data-testid="logs-filter-session"]').setValue('node-b')
    await w.find('[data-testid="logs-apply"]').trigger('click')
    await flushPromises()

    const url = String(fetchMock.mock.calls.at(-1)![0])
    expect(url).toContain('model=gpt')
    expect(url).toContain('status=500')
    expect(url).toContain('session=node-b')
    expect(url).toContain('page=1')
  })

  it('分页：下一页 page=2，上一页受限，末页禁用下一页', async () => {
    const w = await mountView()
    await clickLogsTab(w)

    const btns = w.find('[data-testid="logs-pagination"]').findAll('button')
    expect((btns[0].element as HTMLButtonElement).disabled).toBe(true) // 首页无上一页
    await btns[1].trigger('click')
    await flushPromises()
    expect(String(fetchMock.mock.calls.at(-1)![0])).toContain('page=2')
    expect(w.find('[data-testid="logs-pagination"]').text()).toContain('第 2 / 3 页')
  })

  it('单条详情弹窗：行点击 → 分项成本/firstTokenMs/sessionKey 全渲染，关闭可退', async () => {
    const w = await mountView()
    await clickLogsTab(w)

    await logRows(w)[0].trigger('click')
    const modal = w.find('[data-testid="log-detail-modal"]')
    expect(modal.exists()).toBe(true)
    const text = modal.text()
    expect(text).toContain('glm-4.7') // 计价模型
    expect(text).toContain('direct-a') // 会话
    expect(text).toContain('trace-abc')
    expect(text).toContain('320 ms') // 首 Token
    expect(text).toContain('$0.000800') // 输入分项成本
    expect(text).not.toContain('未命中价目表') // 命中价目 → 不出现该文案

    // 关闭。
    await modal.findAll('button').find(b => b.text().includes('关闭'))!.trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="log-detail-modal"]').exists()).toBe(false)
  })

  it('失败态：错误提示 + 重试', async () => {
    fetchMock.mockImplementation((url: string) => {
      if (url.startsWith('/api/usage/logs')) return Promise.resolve(jsonResponse({}, false, 503))
      return Promise.resolve(route(url))
    })
    const w = await mountView()
    await clickLogsTab(w)
    expect(w.text()).toContain('加载失败：HTTP 503')

    fetchMock.mockImplementation((url: string) => Promise.resolve(route(url)))
    const retry = w.findAll('button').find(b => b.text().includes('重试'))!
    await retry.trigger('click')
    await flushPromises()
    expect(logRows(w)).toHaveLength(2)
  })
})
