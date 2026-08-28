import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// M6 补测（quality-hardening goal 2026-08-25）：P5 沙盒页平台自适应层 ——
// overview 真相源（Windows 追加 status/pending，用户态不拉）、
// 后端探测渲染、联网/严格/联动开关 set_config、confirm 门、restart_hint。
// 后端语义由 handlers/sandbox/tests.rs + nemesis-sandbox 单测钉住。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import SandboxView from '../SandboxView.vue'

// G7 (D1) 构建期门控：VITE_USERLAND_SANDBOX=1 的运行（env-on 单跑）才加载
// UserlandSandbox 子组件；默认运行（env-off）断言诚实降级卡。
// 子组件自身行为由 UserlandSandbox.spec.ts 直挂覆盖（与门控无关）。
const GATE_ON = import.meta.env.VITE_USERLAND_SANDBOX === '1'
// 门控相关用例仅在 env-on 运行时生效（默认运行 skip，不算失败）。
const describeGate = GATE_ON ? describe : describe.skip

const confirmMock = vi.fn(() => true)

function linuxOverview(over: Record<string, unknown> = {}) {
  return {
    platform: 'linux',
    executor: { enabled: true, sandbox: true, strict: false, allow_network: false },
    backend_probe: {
      backends: [
        { name: 'landlock', form: 'SelfApply', availability: 'full', detail: [] },
        { name: 'bwrap', form: 'WrapCommand', availability: 'none', detail: ['未安装'] },
      ],
      selected: 'landlock',
    },
    ready: true,
    ...over,
  }
}

function windowsOverview() {
  return {
    platform: 'windows',
    executor: { enabled: true, sandbox: false, strict: false, allow_network: false },
    backend_probe: { backends: [], selected: null },
    ready: false,
  }
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  vi.stubGlobal('confirm', confirmMock)
  confirmMock.mockReset().mockReturnValue(true)
})

// env-on 运行下 UserlandSandbox 经 defineAsyncComponent 动态 import，
// vite-node 解析 chunk 需要真实墙钟时间（~40ms），flushPromises 等不到 ——
// 轮询直到用户态区段落位（gate-on → 子组件；gate-off → 降级卡）。
async function waitUserlandSettled(w: { find(s: string): { exists(): boolean } }, isLinux = true) {
  if (!GATE_ON || !isLinux) return
  await vi.waitFor(
    () => {
      if (
        !w.find('[data-test="userland-refresh"]').exists() &&
        !w.find('[data-test="userland-degrade"]').exists()
      ) {
        throw new Error('userland section not settled yet')
      }
    },
    { timeout: 5_000, interval: 20 },
  )
}

async function mountView(overview: unknown) {
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'overview') return Promise.resolve(overview)
    if (cmd === 'status') return Promise.resolve({ ready: true, allow_network: false })
    if (cmd === 'pending') return Promise.resolve([])
    if (cmd === 'set_config') return Promise.resolve({ restart_hint: '需重启 Agent' })
    return Promise.resolve({})
  })
  const w = mount(SandboxView)
  await flushPromises()
  await waitUserlandSettled(w, (overview as any)?.platform === 'linux')
  await flushPromises()
  return w
}

describe('SandboxView 平台自适应加载', () => {
  it('Windows：overview 后追加 status + pending', async () => {
    const w = await mountView(windowsOverview())
    const cmds = requestMock.mock.calls.map(c => c[1])
    expect(cmds).toContain('overview')
    expect(cmds).toContain('status')
    expect(cmds).toContain('pending')
    // executor.sandbox=false → 未启用态
    expect(w.text()).toContain('Windows')
  })

  it('Linux：只拉 overview（不发 status/pending），渲染后端探测表', async () => {
    const w = await mountView(linuxOverview())
    const cmds = requestMock.mock.calls.map(c => c[1])
    expect(cmds).toEqual(['overview'])

    // 两种门控形态都必须：用户态面板不触发 status/pending（平台真相源 behavior 不变）
    if (GATE_ON) {
      expect(w.text()).toContain('landlock')
      expect(w.text()).toContain('可用')
      expect(w.text()).toContain('bwrap')
      expect(w.text()).toContain('不可用')
      expect(w.text()).toContain('未安装')
      expect(w.text()).toContain('✓ 已选用')
      expect(w.text()).toContain('实际选用：')
      // executorOn（enabled+sandbox 均 true）
      expect(w.text()).toContain('● 已启用')
      // strictHint：selectedBackend 可用
      expect(w.text()).toContain('landlock 可用 — 严格模式可兑现')
    } else {
      // env-off（Windows/Android 工具链构建）：诚实降级卡，无探测表
      expect(w.find('[data-test="userland-degrade"]').exists()).toBe(true)
      expect(w.text()).toContain('此构建未包含用户态沙盒 UI')
      expect(w.text()).not.toContain('✓ 已选用')
    }
  })

  it('overview 失败 → 静默容错（.catch(null)），页面不崩', async () => {
    const w = await mountView(null)
    expect(w.text()).toBeTruthy()
    // 平台未知：两套布局都不进
    expect(w.text()).not.toContain('● 已启用')
  })
})

describeGate('SandboxView 用户态开关（P5，env-on 构建）', () => {
  it('沙盒内联网：allow_network false → 点击发 {allow_network:true}，toast 带 restart_hint', async () => {
    const w = await mountView(linuxOverview())
    const btn = w.findAll('button').find(b => b.text() === '已关闭')!
    await btn.trigger('click')
    await flushPromises()

    const call = requestMock.mock.calls.find(c => c[1] === 'set_config')!
    expect(call[2]).toEqual({ allow_network: true })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('已开启') && t.message.includes('需重启 Agent'))).toBe(true)
    // 写后刷新
    expect(requestMock.mock.calls.filter(c => c[1] === 'overview').length).toBe(2)
  })

  it('严格模式：后端可用（ready）→ 无 confirm 直接开；文案 fail-closed', async () => {
    const w = await mountView(linuxOverview())
    const btn = w.findAll('button').find(b => b.text() === '已关闭' && b.classes().includes('btn-sm'))!
    // 第一个「已关闭」按钮是联网（allow_network），严格模式是第二个
    const strictBtn = w.findAll('button').filter(b => b.text() === '已关闭')[1]!
    await strictBtn.trigger('click')
    await flushPromises()
    expect(confirmMock).not.toHaveBeenCalled() // ready=true 不需要确认
    const call = requestMock.mock.calls.find(c => c[1] === 'set_config')!
    expect(call[2]).toEqual({ strict: true })
    expect(useToast().toasts.some(t => t.message.includes('fail-closed'))).toBe(true)
    expect(btn).toBeTruthy()
  })

  it('严格模式：后端不可用开启 → 必须过 confirm；取消则不发', async () => {
    const ov = linuxOverview({ ready: false, backend_probe: { backends: [{ name: 'landlock', form: 'SelfApply', availability: 'none', detail: [] }], selected: null } })
    const w = await mountView(ov)
    const strictBtn = w.findAll('button').filter(b => b.text() === '已关闭')[1]!

    confirmMock.mockReturnValueOnce(false) // 取消
    await strictBtn.trigger('click')
    expect(confirmMock).toHaveBeenCalled()
    expect(requestMock.mock.calls.find(c => c[1] === 'set_config')).toBeUndefined()

    confirmMock.mockReturnValueOnce(true) // 确认
    await strictBtn.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.find(c => c[1] === 'set_config')![2]).toEqual({ strict: true })
  })

  it('启用沙盒执行：confirm 门 + enabled/sandbox 联动', async () => {
    let executor = { enabled: false, sandbox: false, strict: false, allow_network: false }
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'overview') return Promise.resolve(linuxOverview({ executor: { ...executor } }))
      if (cmd === 'set_config') {
        // 简化模拟：任一联动请求即视为翻转 enabled/sandbox
        if (data && 'enabled' in data) executor = { ...executor, ...data }
        return Promise.resolve({ restart_hint: '需重启 Agent' })
      }
      return Promise.resolve({})
    })
    const w = mount(SandboxView)
    await flushPromises()
    await waitUserlandSettled(w)

    await w.findAll('button').find(b => b.text() === '启用沙盒执行')!.trigger('click')
    expect(confirmMock).toHaveBeenCalled()
    await flushPromises()
    expect(requestMock.mock.calls.find(c => c[1] === 'set_config')![2]).toEqual({ enabled: true, sandbox: true })
    // 刷新后 executorOn=true → 按钮翻转为停用
    expect(w.text()).toContain('● 已启用')

    await w.findAll('button').find(b => b.text() === '停用沙盒执行')!.trigger('click')
    await flushPromises()
    const calls = requestMock.mock.calls.filter(c => c[1] === 'set_config').map(c => c[2])
    expect(calls).toContainEqual({ enabled: false, sandbox: false })
  })

  it('set_config 失败 → 错误 toast，busy 复位', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'overview') return Promise.resolve(linuxOverview())
      if (cmd === 'set_config') return Promise.reject(new Error('config 只读'))
      return Promise.resolve({})
    })
    const w = mount(SandboxView)
    await flushPromises()
    await waitUserlandSettled(w)
    await w.findAll('button').filter(b => b.text() === '已关闭')[1].trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('config 只读'))).toBe(true)
    const strictBtn = w.findAll('button').filter(b => b.text() === '已关闭')[1]!
    expect(strictBtn.attributes('disabled')).toBeUndefined()
  })
})
