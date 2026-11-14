import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { createRouter, createMemoryHistory } from 'vue-router'
import { useToast } from '../../composables/useToast'

// 安全总览页 spec（十层页面化 2026-11-10；⑪⑫⑬ 编号 2026-11-14 用户拍板）：
// 十三层状态表（行内容来自 security.overview 聚合快照）+ 运行状态区
// （只读）+ 链校验按钮 + JSON
// 编辑器（唯一任意键写入口）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import SecurityOverviewView from '../SecurityOverviewView.vue'

function overviewResult() {
  return {
    layers: {
      injection: { enabled: true, threshold: 0.7, threshold_configured: false },
      command_guard: { enabled: true },
      abac: { standalone_switch: false },
      credential: { enabled: true },
      dlp: { enabled: true, rules: ['email'], action: 'block', low_confidence_action: 'log', inbound_action: 'log' },
      ssrf: { enabled: true, allowed_hosts: ['127.0.0.1'] },
      audit: { log_file_enabled: true, chain_enabled: false },
      default_action: 'deny',
      effective: 'restart',
    },
    intercepts: { total: 42, denied: 7, by_policy: { abac: { total: 30, denied: 5, last_ts: '' }, command_guard: { total: 12, denied: 2, last_ts: '' } } },
    chain: { present: true, segments: 12 },
    estop: { injected: true, engaged: false },
    plan_mode: { available: true, mode: 'build' },
    editor: { injected: true, full_access: false, external_write: false },
    approvals: { injected: true, enabled: false, auto_approved_count: 0 },
    guardian: { mode: 'critical', failure_policy: 'ask' },
    vault: { exists: true },
    scanner: { enabled: true, total: 1, enabled_count: 1, ready_count: 1 },
    global: { available: true, enabled: true },
    executor: { available: true, enabled: false, sandbox: false, vm: false },
  }
}

function makeRouter() {
  return createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', component: { template: '<div />' } },
      { path: '/settings', component: { template: '<div />' } },
      { path: '/security/approval', component: { template: '<div />' } },
    ],
  })
}

async function mountView() {
  const router = makeRouter()
  const w = mount(SecurityOverviewView, { global: { plugins: [router] } })
  await flushPromises()
  return w
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string, _data: any) => {
    if (cmd === 'overview') return Promise.resolve(overviewResult())
    if (cmd === 'config.get') return Promise.resolve({ default_action: 'deny' })
    if (cmd === 'config.save') return Promise.resolve({ saved: true })
    if (cmd === 'chain_verify') return Promise.resolve({ valid: true, total_segments: 12, first_broken_index: null, broken_count: 0 })
    return Promise.resolve({})
  })
})

describe('SecurityOverviewView 十三层状态表', () => {
  it('渲染 13 行层状态；行文本带层号/摘要（阈值/DLP 处置/白名单/引擎计数/Guardian 档）', async () => {
    const w = await mountView()
    const rows = w.find('[data-test="layers-table"]').findAll('tbody tr')
    expect(rows).toHaveLength(13)
    expect(w.text()).toContain('判定阈值 0.7（默认）')
    expect(w.text()).toContain('处置 block')
    expect(w.text()).toContain('白名单 1 项')
    expect(w.text()).toContain('引擎 1 个')
    expect(w.text()).toContain('CRITICAL 全审')
    expect(w.text()).toContain('已初始化')
    w.unmount()
  })

  it('⑦ 病毒扫描行：enabled 但无就绪引擎 → 「未就绪」warn 徽标', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'overview') {
        const o = overviewResult()
        o.scanner = { enabled: true, total: 1, enabled_count: 1, ready_count: 0 }
        return Promise.resolve(o)
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(w.find('[data-test="layer-status-⑦"]').text()).toContain('未就绪')
    w.unmount()
  })

  it('行点击跳对应页（⑨ → /sandbox）', async () => {
    const router = makeRouter()
    const pushSpy = vi.spyOn(router, 'push')
    const w = mount(SecurityOverviewView, { global: { plugins: [router] } })
    await flushPromises()
    await w.find('[data-test="layer-row-⑨"]').trigger('click')
    await flushPromises()
    expect(pushSpy).toHaveBeenCalledWith('/sandbox')
    w.unmount()
  })
})

describe('SecurityOverviewView 运行状态区 + 链校验', () => {
  it('状态区只读渲染：estop 未触发 / Plan Build / Full Access 关 / 免打扰关 / 总闸开启', async () => {
    const w = await mountView()
    const states = w.find('[data-test="runtime-states"]').text()
    expect(states).toContain('未触发')
    expect(states).toContain('Build')
    expect(states).toContain('关闭')
    expect(states).toContain('总闸开启')
    w.unmount()
  })

  it('链校验按钮 → logs.chain_verify；结果展示通过文案', async () => {
    const w = await mountView()
    requestMock.mockClear()
    await w.find('[data-test="overview-chain-verify"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('logs', 'chain_verify')
    expect(w.find('[data-test="overview-chain-result"]').text()).toContain('通过')
    w.unmount()
  })
})

describe('SecurityOverviewView JSON 编辑器', () => {
  it('编辑 → textarea 出现；保存 → config.save 载荷为解析后 JSON', async () => {
    const w = await mountView()
    requestMock.mockClear()
    await w.find('[data-test="json-edit"]').trigger('click')
    const editor = w.find('[data-test="json-editor"]')
    expect(editor.exists()).toBe(true)
    await editor.setValue('{"default_action":"allow","layers":{}}')
    await w.find('[data-test="json-save"]').trigger('click')
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter(c => c[1] === 'config.save')
    expect(saveCalls.length).toBe(1)
    expect(saveCalls[0][2]).toEqual({ default_action: 'allow', layers: {} })
    w.unmount()
  })

  it('JSON 非法 → error toast 且不退出编辑态', async () => {
    const w = await mountView()
    await w.find('[data-test="json-edit"]').trigger('click')
    await w.find('[data-test="json-editor"]').setValue('{oops')
    await w.find('[data-test="json-save"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error')).toBe(true)
    expect(w.find('[data-test="json-editor"]').exists()).toBe(true)
    w.unmount()
  })
})
