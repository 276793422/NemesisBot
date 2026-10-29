import { mount } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { ref } from 'vue'

// Sidebar L9 自更新三态长条（2026-10-23 apply 假挂死修复的行为契约）：
// - 受理即回的 apply 失败必须解挂「更新执行中」粘滞态（此前唯一失败信号
//   是 gateway 日志一行 warn，前端永远等不到收口 → 永久挂死）；
// - 顺序闸拒绝（downgrade_denied）→ 主动提供显式豁免重试（R1 语义反转的
//   Dashboard 面；豁免只放开顺序判定，签名验真照常）；
// - 首更形态（sequence_baseline_present=false）→ 确认框提前说明并随 apply
//   自带 allow_downgrade:true（存量安装首更默认必拒的提前告知）。
// 失败 toast/确认的唯一裁决点 = status 的 last_apply_error 记录（SSE 帧
// 没有 downgrade_denied，只做唤醒刷新）。

const sseHandlers = new Map<string, (data?: unknown) => void>()
const wsapiRequest = vi.fn()
const toastError = vi.fn()
const toastWarn = vi.fn()
const confirmMock = vi.fn()
// 收起态测试的可变开关（useAppStore 工厂在调用时读取；beforeEach 重置）
let mockCollapsed = false

vi.mock('../../composables/useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    sseHandlers.set(type, handler)
  }),
  off: vi.fn(),
}))

vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: wsapiRequest }),
}))

vi.mock('../../composables/useToast', () => ({
  useToast: () => ({
    toasts: [],
    info: vi.fn(),
    success: vi.fn(),
    warn: toastWarn,
    error: toastError,
    remove: vi.fn(),
  }),
}))

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn() }),
  useRoute: () => ({ path: '/' }),
}))

vi.mock('../../stores/app', () => ({
  useAppStore: () => ({
    connected: true,
    sidebarCollapsed: mockCollapsed,
    showMobileSidebar: false,
    toggleSidebar: vi.fn(),
  }),
}))

vi.mock('../../stores/auth', () => ({
  useAuthStore: () => ({ logout: vi.fn() }),
}))

vi.mock('../../composables/useTheme', () => ({
  useTheme: () => ({ theme: ref('dark'), toggleTheme: vi.fn() }),
}))

vi.mock('../../composables/useEditorMode', () => ({
  useEditorMode: () => ({ fullAccess: ref(false) }),
}))

vi.mock('../../skins/navModel', () => ({
  visibleNavGroups: () => [],
}))

import Sidebar from '../Sidebar.vue'

function statusPayload(overrides: Record<string, unknown> = {}) {
  return {
    wired: true,
    platform_supported: true,
    journal_phase: null,
    pending: null,
    download: { active: false, phase: 'idle' },
    cached_check: { update_available: true, latest_tag: 'v9.9.9' },
    last_apply_error: null,
    sequence_baseline_present: true,
    ...overrides,
  }
}

beforeEach(() => {
  mockCollapsed = false
  sseHandlers.clear()
  wsapiRequest.mockReset()
  wsapiRequest.mockImplementation((mod: string, cmd: string) => {
    if (mod === 'estop' && cmd === 'status') return Promise.resolve({ engaged: false })
    if (mod === 'security' && cmd === 'signature_verify_status')
      return Promise.resolve({ injected: false })
    return Promise.resolve({})
  })
  toastError.mockReset()
  toastWarn.mockReset()
  confirmMock.mockReset()
  vi.spyOn(window, 'confirm').mockImplementation((...args) => confirmMock(...args))
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

async function mounted() {
  const w = mount(Sidebar)
  await vi.advanceTimersByTimeAsync(0) // onMounted 首轮刷新（estop/sig/status）
  return w
}

function suText(w: ReturnType<typeof mount>) {
  const el = w.find('.selfupdate-item')
  return el.exists() ? el.text() : ''
}

describe('Sidebar 自更新长条（apply 失败反馈面）', () => {
  it('受理后失败：粘滞态解挂回 ready + toast 一次 + 豁免确认一次', async () => {
    // 第 1 轮：pending 就绪、无失败 → ready 态。
    wsapiRequest.mockImplementation((mod: string, cmd: string, _d?: unknown) => {
      if (mod === 'selfupdate' && cmd === 'status')
        return Promise.resolve(statusPayload({ pending: { release_tag: 'v9.9.9' } }))
      if (mod === 'selfupdate' && cmd === 'apply') return Promise.resolve({ accepted: true })
      if (mod === 'estop' && cmd === 'status') return Promise.resolve({ engaged: false })
      if (mod === 'security') return Promise.resolve({ injected: false })
      return Promise.resolve({})
    })
    const w = await mounted()
    expect(suText(w)).toContain('开始更新')

    // 用户点开始更新 → 确认 → 受理 → applying 粘滞。
    confirmMock.mockReturnValue(true)
    const applyBtn = w.find('.su-action')
    await applyBtn.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('selfupdate', 'apply', {}, 8000)
    expect(suText(w)).toContain('更新执行中')

    // 第 2 轮 status：失败在案（顺序闸拒绝类）→ 解挂回 ready + toast +
    // 豁免确认（用户同意 → 带 allow_downgrade 重试）。
    wsapiRequest.mockImplementation((mod: string, cmd: string) => {
      if (mod === 'selfupdate' && cmd === 'status')
        return Promise.resolve(
          statusPayload({
            pending: { release_tag: 'v9.9.9' },
            last_apply_error: {
              message: '无法证明这次更新不是降级（本机无已安装版本的清单基准）',
              downgrade_denied: true,
              at_ms: 1,
            },
          }),
        )
      if (mod === 'selfupdate' && cmd === 'apply') return Promise.resolve({ accepted: true })
      if (mod === 'estop' && cmd === 'status') return Promise.resolve({ engaged: false })
      if (mod === 'security') return Promise.resolve({ injected: false })
      return Promise.resolve({})
    })
    confirmMock.mockReturnValue(true)
    await vi.advanceTimersByTimeAsync(10000)
    expect(toastError).toHaveBeenCalledTimes(1)
    expect(toastError.mock.calls[0][0]).toContain('无法证明这次更新不是降级')
    expect(confirmMock).toHaveBeenCalledTimes(2) // 开始更新确认 + 豁免确认
    expect(wsapiRequest).toHaveBeenCalledWith(
      'selfupdate',
      'apply',
      { allow_downgrade: true },
      8000,
    )
    // 豁免重试受理成功 → 重新进入编排粘滞（真实后端在 apply 发起时已清旧错，
    // 后续轮询不会再弹这条失败）。
    await vi.advanceTimersByTimeAsync(0)
    expect(suText(w)).toContain('更新执行中')
    w.unmount()
  })

  it('用户拒绝豁免：不重试，同一条失败不重复 toast/确认', async () => {
    wsapiRequest.mockImplementation((mod: string, cmd: string) => {
      if (mod === 'selfupdate' && cmd === 'status')
        return Promise.resolve(
          statusPayload({
            last_apply_error: { message: 'M-拒绝场景', downgrade_denied: true, at_ms: 1 },
          }),
        )
      if (mod === 'estop' && cmd === 'status') return Promise.resolve({ engaged: false })
      if (mod === 'security') return Promise.resolve({ injected: false })
      return Promise.resolve({})
    })
    confirmMock.mockReturnValue(false)
    const w = await mounted()
    expect(toastError).toHaveBeenCalledTimes(1)
    expect(confirmMock).toHaveBeenCalledTimes(1)
    expect(wsapiRequest).not.toHaveBeenCalledWith(
      'selfupdate',
      'apply',
      { allow_downgrade: true },
      8000,
    )
    // 后续轮询：去重哨兵拦住，不再弹。
    await vi.advanceTimersByTimeAsync(30000)
    expect(toastError).toHaveBeenCalledTimes(1)
    expect(confirmMock).toHaveBeenCalledTimes(1)
    w.unmount()
  })

  it('首更形态：确认框说明跳过比对，apply 自带 allow_downgrade:true', async () => {
    wsapiRequest.mockImplementation((mod: string, cmd: string) => {
      if (mod === 'selfupdate' && cmd === 'status')
        return Promise.resolve(
          statusPayload({
            sequence_baseline_present: false,
            cached_check: { update_available: true, latest_tag: 'v9.9.9' },
          }),
        )
      if (mod === 'selfupdate' && cmd === 'apply') return Promise.resolve({ accepted: true })
      if (mod === 'estop' && cmd === 'status') return Promise.resolve({ engaged: false })
      if (mod === 'security') return Promise.resolve({ injected: false })
      return Promise.resolve({})
    })
    const w = await mounted()
    expect(suText(w)).toContain('下载更新') // idle 态（无 pending，有更新）

    confirmMock.mockReturnValue(true)
    const idleBtn = w.findAll('.su-action').find(a => a.text().includes('下载更新'))
    await idleBtn!.trigger('click') // 下载
    await vi.advanceTimersByTimeAsync(0)

    // 下载受理后切 status：pending 就绪（仍首更形态）→ 点开始更新。
    wsapiRequest.mockImplementation((mod: string, cmd: string) => {
      if (mod === 'selfupdate' && cmd === 'status')
        return Promise.resolve(
          statusPayload({
            sequence_baseline_present: false,
            pending: { release_tag: 'v9.9.9' },
          }),
        )
      if (mod === 'selfupdate' && cmd === 'apply') return Promise.resolve({ accepted: true })
      if (mod === 'estop' && cmd === 'status') return Promise.resolve({ engaged: false })
      if (mod === 'security') return Promise.resolve({ injected: false })
      return Promise.resolve({})
    })
    await vi.advanceTimersByTimeAsync(10000)
    const readyBtn = w.find('.su-action')
    expect(readyBtn.text()).toContain('开始更新')
    await readyBtn.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    // 确认框提前说明首更豁免语义；apply 直接带豁免（用户在确认框完成授权）。
    expect(confirmMock).toHaveBeenCalled()
    expect(String(confirmMock.mock.calls.at(-1)![0])).toContain('首次更新')
    expect(wsapiRequest).toHaveBeenLastCalledWith(
      'selfupdate',
      'apply',
      { allow_downgrade: true },
      8000,
    )
    w.unmount()
  })

  it('有基准的正常更新：确认框不提首更，apply 不带豁免键', async () => {
    wsapiRequest.mockImplementation((mod: string, cmd: string) => {
      if (mod === 'selfupdate' && cmd === 'status')
        return Promise.resolve(
          statusPayload({ pending: { release_tag: 'v9.9.9' } }),
        )
      if (mod === 'selfupdate' && cmd === 'apply') return Promise.resolve({ accepted: true })
      if (mod === 'estop' && cmd === 'status') return Promise.resolve({ engaged: false })
      if (mod === 'security') return Promise.resolve({ injected: false })
      return Promise.resolve({})
    })
    const w = await mounted()
    confirmMock.mockReturnValue(true)
    await w.find('.su-action').trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(String(confirmMock.mock.calls[0][0])).not.toContain('首次更新')
    expect(wsapiRequest).toHaveBeenCalledWith('selfupdate', 'apply', {}, 8000)
    w.unmount()
  })

  it('编排进行中（无失败有 pending）：粘滞保持不解挂', async () => {
    wsapiRequest.mockImplementation((mod: string, cmd: string) => {
      if (mod === 'selfupdate' && cmd === 'status')
        return Promise.resolve(statusPayload({ pending: { release_tag: 'v9.9.9' } }))
      if (mod === 'selfupdate' && cmd === 'apply') return Promise.resolve({ accepted: true })
      if (mod === 'estop' && cmd === 'status') return Promise.resolve({ engaged: false })
      if (mod === 'security') return Promise.resolve({ injected: false })
      return Promise.resolve({})
    })
    const w = await mounted()
    confirmMock.mockReturnValue(true)
    await w.find('.su-action').trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(suText(w)).toContain('更新执行中')
    // 多轮轮询仍无失败信号 → 保持粘滞（真实成功路径由 gateway 重启收口）。
    await vi.advanceTimersByTimeAsync(30000)
    expect(suText(w)).toContain('更新执行中')
    w.unmount()
  })

  it('收起态下载中：进度条 DOM 契约不卸载（压缩由 collapsed CSS 承担）', async () => {
    // 侧栏收起 + 下载进行中并存（BUG-20261030：进度态文本类名不带
    // nav-label，逃过收起态 display:none 规则，曾溢出 56px 栏压盖主内容）。
    // 修复 = Sidebar.vue scoped 收起态 CSS 藏两行文本保留 ✕ + 窄轨道；
    // jsdom 不算样式表，这里钉 DOM 契约——元素照常渲染、类名集合不变
    // （类名是收起态 CSS 规则的锚点，改名必须同步规则）。
    mockCollapsed = true
    wsapiRequest.mockImplementation((mod: string, cmd: string) => {
      if (mod === 'selfupdate' && cmd === 'status')
        return Promise.resolve(
          statusPayload({
            download: { active: true, phase: 'downloading', downloaded_bytes: 45000000, total_bytes: 12000000 * 10 },
          }),
        )
      if (mod === 'estop' && cmd === 'status') return Promise.resolve({ engaged: false })
      if (mod === 'security') return Promise.resolve({ injected: false })
      return Promise.resolve({})
    })
    const w = await mounted()
    expect(w.find('aside.sidebar').classes()).toContain('collapsed')
    // downloading 态整棵结构照常在 DOM（收起不卸载，视觉压缩交给 CSS）
    expect(w.find('.su-progress').exists()).toBe(true)
    expect(w.find('.su-progress-title').exists()).toBe(true)
    expect(w.find('.su-progress-text').exists()).toBe(true)
    expect(w.find('.su-progress-track').exists()).toBe(true)
    expect(w.find('.su-cancel').exists()).toBe(true)
    // 进行中形态：百分比标签 + 确定宽度填充（有 total_bytes）
    expect(suText(w)).toContain('%')
    w.unmount()
  })
})
