import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useToast } from '../../composables/useToast'

// 使用统计设置卡（批次 2 配置页补全）：usage.retention_days / usage.max_rows
// 数据保留策略。usage 页数据面走 HTTP fetch（authedFetch），配置面走 WSAPI
// config.get/set_field——本 spec 双 mock 并行。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

const fetchMock = vi.fn()

// 默认 usage tab 渲染 echarts 图表——jsdom 无 ResizeObserver，mock 掉
// （与 UsageView.logs.spec 同款；不 mock 则 3 个 unhandled rejection 污染全量跑）。
vi.mock('vue-echarts', () => ({
  default: { name: 'VChart', props: ['option', 'autoresize'], template: '<div class="v-chart-stub" />' },
}))

import UsageView from '../../views/UsageView.vue'

beforeEach(() => {
  setActivePinia(createPinia()) // authedFetch 读 auth store
  fetchMock.mockReset()
  fetchMock.mockImplementation((url: string) =>
    Promise.resolve({ ok: true, status: 200, json: async () => ({ data: null }) }),
  )
  vi.stubGlobal('fetch', fetchMock)
  requestMock.mockReset()
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'get') return Promise.resolve({})
    if (cmd === 'set_field') return Promise.resolve({ ok: true })
    return Promise.resolve({})
  })
  useToast().toasts.splice(0)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

async function mountView() {
  const w = mount(UsageView)
  await flushPromises()
  return w
}

async function openSettingsTab(w: ReturnType<typeof mount>) {
  await w.findAll('button').find(b => b.text().includes('设置'))!.trigger('click')
  await flushPromises()
}

describe('UsageView 数据保留策略（批次 2）', () => {
  it('config 缺 usage 段 → 缺省 30 / 500000；改 retention_days → set_field {path, value:90}', async () => {
    const w = await mountView()
    await openSettingsTab(w)
    const days = w.find('[data-test="usage-retention-input"]')
    const rows = w.find('[data-test="usage-max-rows-input"]')
    expect((days.element as HTMLInputElement).value).toBe('30')
    expect((rows.element as HTMLInputElement).value).toBe('500000')
    await days.setValue('90')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'usage.retention_days', value: 90 })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('最迟 6 小时'))).toBe(true)
    w.unmount()
  })

  it('已配置 60 / 100000 → 照实渲染；max_rows 改动 → set_field usage.max_rows', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') return Promise.resolve({ usage: { retention_days: 60, max_rows: 100000 } })
      if (cmd === 'set_field') return Promise.resolve({ ok: true })
      return Promise.resolve({})
    })
    const w = await mountView()
    await openSettingsTab(w)
    expect((w.find('[data-test="usage-retention-input"]').element as HTMLInputElement).value).toBe('60')
    const rows = w.find('[data-test="usage-max-rows-input"]')
    expect((rows.element as HTMLInputElement).value).toBe('100000')
    await rows.setValue('200000')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'usage.max_rows', value: 200000 })
    w.unmount()
  })

  it('非法输入（NaN）→ 本地拦截不写盘 + error toast', async () => {
    const w = await mountView()
    await openSettingsTab(w)
    await w.find('[data-test="usage-retention-input"]').setValue('abc')
    await flushPromises()
    expect(requestMock).not.toHaveBeenCalledWith('config', 'set_field', expect.anything())
    expect(useToast().toasts.some(t => t.type === 'error')).toBe(true)
    w.unmount()
  })
})
