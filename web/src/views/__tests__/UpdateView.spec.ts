import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// 【更新】页（2026-10-27）：版本对照卡（当前运行 vs 云端最新，更新成功的
// 终态证明面）+ 手动更新操作面。状态机与操作语义照抄 Sidebar 自更新长条
// （同源同命令），失败红条的唯一裁决点 = status.last_apply_error（后端在
// 新一轮 apply 受理时清空）；SSE 帧只做进度与收口唤醒。

const sseHandlers = new Map<string, (data?: unknown) => void>()
const wsapiRequest = vi.fn()
const toastError = vi.fn()
const toastWarn = vi.fn()
const confirmMock = vi.fn()

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

import UpdateView from '../UpdateView.vue'
import { off as offSSE } from '../../composables/useSSE'

// 与后端 selfupdate status 投影同构（handlers/selfupdate.rs + selfupdate.rs）。
function statusPayload(over: Record<string, unknown> = {}) {
  return {
    version: '1.2.2',
    commit: 'b'.repeat(40),
    commit_known: true,
    wired: true,
    platform_supported: true,
    journal_phase: null,
    pending: null,
    download: { active: false, phase: 'idle' },
    cached_check: {
      latest_tag: 'v1.2.3',
      latest_commit: 'c'.repeat(40),
      update_available: true,
      checked_at_ms: 1730000000000,
    },
    last_apply_error: null,
    sequence_baseline_present: true,
    ...over,
  }
}

beforeEach(() => {
  sseHandlers.clear()
  wsapiRequest.mockReset()
  wsapiRequest.mockImplementation((mod: string, cmd: string) => {
    if (mod === 'selfupdate' && cmd === 'status') return Promise.resolve(statusPayload())
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

async function mounted(impl?: (mod: string, cmd: string) => Promise<unknown>) {
  // 始终重设实现：同测试内多次 mount 时，不传 impl 必须回到默认 fixture
  //（mockImplementation 是整体覆盖，不传会沿用上一个用例的 custom impl）。
  wsapiRequest.mockImplementation(
    impl ??
      ((mod: string, cmd: string) =>
        mod === 'selfupdate' && cmd === 'status' ? Promise.resolve(statusPayload()) : Promise.resolve({})),
  )
  const w = mount(UpdateView)
  await vi.advanceTimersByTimeAsync(0) // onMounted 首轮 status 刷新
  return w
}

describe('UpdateView 版本对照卡', () => {
  it('当前运行 + 云端最新渲染：版本串 / commit 前 12 位短码 / 检查时间', async () => {
    const w = await mounted()
    const cur = w.find('[data-test="cmp-current"]').text()
    expect(cur).toContain('1.2.2')
    expect(cur).toContain('commit ' + 'b'.repeat(12))
    expect(cur).not.toContain('commit 未知')
    const cloud = w.find('[data-test="cmp-cloud"]').text()
    expect(cloud).toContain('v1.2.3')
    expect(cloud).toContain('commit ' + 'c'.repeat(12))
    // cached_check 存在 → 云端侧不落「版本未知」注记
    expect(cloud).not.toContain('版本未知')
    w.unmount()
  })

  it('commit_known=false：当前侧诚实标注（旧版本/开发构建），不渲染假短码', async () => {
    const w = await mounted((m, c) =>
      c === 'status'
        ? Promise.resolve(statusPayload({ commit_known: false, commit: '' }))
        : Promise.resolve({}),
    )
    expect(w.find('[data-test="cmp-current"]').text()).toContain('commit 未知')
    w.unmount()
  })

  it('徽标三态：✅已是最新 / ⬆有新版本 / ？版本未知', async () => {
    // 已是最新：cached_check.update_available=false
    const wOk = await mounted((m, c) =>
      c === 'status'
        ? Promise.resolve(
            statusPayload({
              cached_check: { latest_tag: 'v1.2.2', latest_commit: '', update_available: false, checked_at_ms: 1 },
            }),
          )
        : Promise.resolve({}),
    )
    expect(wOk.find('[data-test="badge"]').text()).toBe('已是最新')
    wOk.unmount()

    // 有新版本（默认 fixture update_available=true）
    const wNew = await mounted()
    expect(wNew.find('[data-test="badge"]').text()).toBe('有新版本 v1.2.3')
    wNew.unmount()

    // 版本未知：无缓存且未检查
    const wUnk = await mounted((m, c) =>
      c === 'status' ? Promise.resolve(statusPayload({ cached_check: null })) : Promise.resolve({}),
    )
    expect(wUnk.find('[data-test="badge"]').text()).toBe('版本未知')
    expect(wUnk.find('[data-test="cmp-cloud"]').text()).toContain('版本未知')
    wUnk.unmount()
  })
})

describe('UpdateView 检查更新', () => {
  it('点击转发 selfupdate.check（15s 预算）→ 对照卡与徽标刷新', async () => {
    const w = await mounted((m, c) => {
      if (m === 'selfupdate' && c === 'status') return Promise.resolve(statusPayload({ cached_check: null }))
      if (m === 'selfupdate' && c === 'check')
        return Promise.resolve({ latest_tag: 'v2.0.0', latest_commit: 'd'.repeat(40), update_available: true })
      return Promise.resolve({})
    })
    expect(w.find('[data-test="badge"]').text()).toBe('版本未知')
    await w.find('[data-test="check-btn"]').trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('selfupdate', 'check', {}, 15000)
    expect(w.find('[data-test="badge"]').text()).toBe('有新版本 v2.0.0')
    // check 后有更新 → 操作区进 idle（可下载）
    expect(w.find('[data-test="op-idle"]').exists()).toBe(true)
    w.unmount()
  })

  it('check 进行中按钮禁用；失败 → 错误 toast，徽标维持原状', async () => {
    let resolveCheck: (v: unknown) => void = () => {}
    const w = await mounted((m, c) => {
      if (m === 'selfupdate' && c === 'status') return Promise.resolve(statusPayload({ cached_check: null }))
      if (m === 'selfupdate' && c === 'check')
        return new Promise((res) => {
          resolveCheck = res
        })
      return Promise.resolve({})
    })
    const btn = w.find('[data-test="check-btn"]')
    await btn.trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect((btn.element as HTMLButtonElement).disabled).toBe(true)
    resolveCheck({ latest_tag: 'v2.0.0', update_available: true })
    await vi.advanceTimersByTimeAsync(0)
    expect((w.find('[data-test="check-btn"]').element as HTMLButtonElement).disabled).toBe(false)
    w.unmount()

    const w2 = await mounted((m, c) => {
      if (m === 'selfupdate' && c === 'status') return Promise.resolve(statusPayload({ cached_check: null }))
      if (m === 'selfupdate' && c === 'check') return Promise.reject(new Error('网络不可达'))
      return Promise.resolve({})
    })
    await w2.find('[data-test="check-btn"]').trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(toastError).toHaveBeenCalledTimes(1)
    expect(toastError.mock.calls[0][0]).toContain('网络不可达')
    expect(w2.find('[data-test="badge"]').text()).toBe('版本未知')
    w2.unmount()
  })
})

describe('UpdateView 空态与闸面', () => {
  it('wired=false：未装配诚实空态，操作区缺席', async () => {
    const w = await mounted((m, c) =>
      c === 'status' ? Promise.resolve(statusPayload({ wired: false })) : Promise.resolve({}),
    )
    expect(w.find('[data-test="empty-unwired"]').exists()).toBe(true)
    expect(w.text()).toContain('未装配自更新通道')
    expect(w.find('[data-test="op-idle"]').exists()).toBe(false)
    w.unmount()
  })

  it('platform_supported=false：平台空态', async () => {
    const w = await mounted((m, c) =>
      c === 'status' ? Promise.resolve(statusPayload({ platform_supported: false })) : Promise.resolve({}),
    )
    expect(w.find('[data-test="empty-platform"]').exists()).toBe(true)
    expect(w.find('[data-test="op-idle"]').exists()).toBe(false)
    w.unmount()
  })

  it('journal_phase：未收口事务提示 + 操作区收口中（更新操作不可用）', async () => {
    const w = await mounted((m, c) =>
      c === 'status' ? Promise.resolve(statusPayload({ journal_phase: 'staging' })) : Promise.resolve({}),
    )
    expect(w.find('[data-test="journal-note"]').text()).toContain('staging')
    expect(w.find('[data-test="op-idle"]').exists()).toBe(false)
    expect(w.text()).toContain('操作暂不可用')
    w.unmount()
  })
})

describe('UpdateView 操作状态机（idle→downloading→ready→applying）', () => {
  it('idle：发现新版本 + 下载按钮；点击转发 download（8s 预算）→ downloading', async () => {
    const w = await mounted()
    expect(w.find('[data-test="op-idle"]').text()).toContain('发现新版本 v1.2.3')
    await w.find('[data-test="dl-btn"]').trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('selfupdate', 'download', {}, 8000)
    expect(w.find('[data-test="op-downloading"]').exists()).toBe(true)
    w.unmount()
  })

  it('downloading：SSE 帧驱动进度（确定态 %/MB；无 total 时只显 MB）；取消转发 cancel_download 回 idle', async () => {
    const w = await mounted()
    await w.find('[data-test="dl-btn"]').trigger('click')
    await vi.advanceTimersByTimeAsync(0)

    const onProgress = sseHandlers.get('selfupdate-progress')!
    expect(onProgress).toBeTruthy()
    onProgress({ phase: 'downloading', downloaded_bytes: 50 * 1024 * 1024, total_bytes: 200 * 1024 * 1024 })
    await flushPromises() // ref 直改后模板重渲异步落定
    expect(w.find('[data-test="progress-label"]').text()).toContain('25%')
    expect(w.find('[data-test="progress-label"]').text()).toContain('50.0MB')
    expect(w.find('[data-test="progress-label"]').text()).toContain('200.0MB')
    onProgress({ phase: 'downloading', downloaded_bytes: 30 * 1024 * 1024, total_bytes: null })
    await flushPromises()
    expect(w.find('[data-test="progress-label"]').text()).toContain('30.0MB')
    expect(w.find('[data-test="progress-label"]').text()).not.toContain('%')

    await w.find('[data-test="dl-cancel"]').trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('selfupdate', 'cancel_download', {}, 5000)
    expect(w.find('[data-test="op-idle"]').exists()).toBe(true)
    w.unmount()
  })

  it('SSE ready 帧：收口唤醒刷新 → status pending 就绪 → ready 态', async () => {
    const w = await mounted()
    await w.find('[data-test="dl-btn"]').trigger('click')
    await vi.advanceTimersByTimeAsync(0)

    // ready 帧到达时 status 已切 pending 就绪
    wsapiRequest.mockImplementation((mod: string, cmd: string) => {
      if (mod === 'selfupdate' && cmd === 'status')
        return Promise.resolve(statusPayload({ pending: { release_tag: 'v1.2.3' } }))
      return Promise.resolve({})
    })
    sseHandlers.get('selfupdate-progress')!({ phase: 'ready', downloaded_bytes: 200 * 1024 * 1024 })
    await vi.advanceTimersByTimeAsync(0)
    expect(w.find('[data-test="op-ready"]').exists()).toBe(true)
    expect(w.find('[data-test="op-ready"]').text()).toContain('v1.2.3')
    w.unmount()
  })

  it('ready → 开始更新：确认框 → apply（无豁免键）→ applying 粘滞保持', async () => {
    const w = await mounted((m, c) => {
      if (m === 'selfupdate' && c === 'status')
        return Promise.resolve(statusPayload({ pending: { release_tag: 'v1.2.3' } }))
      return Promise.resolve({})
    })
    confirmMock.mockReturnValue(true)
    await w.find('[data-test="apply-btn"]').trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(String(confirmMock.mock.calls[0][0])).toContain('确定更新到 v1.2.3')
    expect(String(confirmMock.mock.calls[0][0])).not.toContain('首次更新')
    expect(wsapiRequest).toHaveBeenCalledWith('selfupdate', 'apply', {}, 8000)
    expect(w.find('[data-test="op-applying"]').exists()).toBe(true)
    // 后续轮询（无失败有 pending）→ 粘滞保持（真实成功路径由重启收口）
    await vi.advanceTimersByTimeAsync(30000)
    expect(w.find('[data-test="op-applying"]').exists()).toBe(true)
    w.unmount()
  })

  it('首更形态（无清单基准）：确认框提前说明 + apply 自带 allow_downgrade:true', async () => {
    const w = await mounted((m, c) => {
      if (m === 'selfupdate' && c === 'status')
        return Promise.resolve(
          statusPayload({ pending: { release_tag: 'v1.2.3' }, sequence_baseline_present: false }),
        )
      return Promise.resolve({})
    })
    confirmMock.mockReturnValue(true)
    await w.find('[data-test="apply-btn"]').trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(String(confirmMock.mock.calls[0][0])).toContain('首次更新')
    expect(wsapiRequest).toHaveBeenCalledWith('selfupdate', 'apply', { allow_downgrade: true }, 8000)
    expect(w.find('[data-test="op-applying"]').exists()).toBe(true)
    w.unmount()
  })

  it('apply 失败红条：last_apply_error 渲染；downgrade_denied → 内联豁免按钮 → 点击带豁免重试；受理后红条消失', async () => {
    let withErr = true
    const w = await mounted((m, c) => {
      if (m === 'selfupdate' && c === 'status')
        return Promise.resolve(
          statusPayload({
            pending: { release_tag: 'v1.2.3' },
            last_apply_error: withErr
              ? { message: '无法证明这次更新不是降级', downgrade_denied: true, at_ms: 1 }
              : null,
          }),
        )
      return Promise.resolve({})
    })
    // 红条在场（不 toast——侧栏 owns toast，页面内联单点）
    expect(w.find('[data-test="apply-error"]').text()).toContain('无法证明这次更新不是降级')
    expect(toastError).not.toHaveBeenCalled()
    // 豁免重试（内联按钮，无二次确认框）
    confirmMock.mockReturnValue(true)
    await w.find('[data-test="exempt-retry"]').trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    expect(wsapiRequest).toHaveBeenCalledWith('selfupdate', 'apply', { allow_downgrade: true }, 8000)
    // 受理成功 → 后端已清旧错 → 下一轮轮询红条消失、回到 applying 粘滞
    withErr = false
    await vi.advanceTimersByTimeAsync(10000)
    expect(w.find('[data-test="apply-error"]').exists()).toBe(false)
    expect(w.find('[data-test="op-applying"]').exists()).toBe(true)
    w.unmount()
  })

  it('非豁免类失败：红条在场但无豁免按钮', async () => {
    const w = await mounted((m, c) =>
      c === 'status'
        ? Promise.resolve(
            statusPayload({ last_apply_error: { message: '签名验签未通过', downgrade_denied: false, at_ms: 1 } }),
          )
        : Promise.resolve({}),
    )
    expect(w.find('[data-test="apply-error"]').exists()).toBe(true)
    expect(w.find('[data-test="exempt-retry"]').exists()).toBe(false)
    w.unmount()
  })

  it('下载失败（download.phase=error）：idle 行内联上次失败原因 + 重试下载', async () => {
    const w = await mounted((m, c) =>
      c === 'status'
        ? Promise.resolve(
            statusPayload({ download: { active: false, phase: 'error', error: 'SHA-256 不匹配' } }),
          )
        : Promise.resolve({}),
    )
    const idle = w.find('[data-test="op-idle"]')
    expect(idle.text()).toContain('上次下载失败')
    expect(idle.text()).toContain('SHA-256 不匹配')
    expect(w.find('[data-test="dl-btn"]').text()).toBe('重试下载')
    w.unmount()
  })
})

describe('UpdateView SSE 与生命周期', () => {
  it('SSE error 帧不带 message（独立下载失败）→ warn toast + 刷新', async () => {
    const w = await mounted()
    await w.find('[data-test="dl-btn"]').trigger('click')
    await vi.advanceTimersByTimeAsync(0)
    toastWarn.mockClear()
    sseHandlers.get('selfupdate-progress')!({ phase: 'error', downloaded_bytes: 0, message: null })
    await vi.advanceTimersByTimeAsync(0)
    expect(toastWarn).toHaveBeenCalledTimes(1)
    expect(toastWarn.mock.calls[0][0]).toContain('下载失败')
    // 回 idle 可重下（status 无 pending/download.active）
    expect(w.find('[data-test="op-idle"]').exists()).toBe(true)
    w.unmount()
  })

  it('SSE error 帧带 message（apply 编排失败）→ 不 toast，红条由 status 单点呈现', async () => {
    const w = await mounted((m, c) =>
      c === 'status'
        ? Promise.resolve(
            statusPayload({ last_apply_error: { message: '替换失败', downgrade_denied: false, at_ms: 1 } }),
          )
        : Promise.resolve({}),
    )
    toastWarn.mockClear()
    toastError.mockClear()
    sseHandlers.get('selfupdate-progress')!({ phase: 'error', downloaded_bytes: 0, message: '替换失败' })
    await vi.advanceTimersByTimeAsync(0)
    expect(toastWarn).not.toHaveBeenCalled()
    expect(toastError).not.toHaveBeenCalled()
    expect(w.find('[data-test="apply-error"]').text()).toContain('替换失败')
    w.unmount()
  })

  it('订阅生命周期：挂载注册 selfupdate-progress，卸载解绑 + 轮询停止', async () => {
    const w = await mounted()
    expect(sseHandlers.has('selfupdate-progress')).toBe(true)
    w.unmount()
    expect(offSSE).toHaveBeenCalledWith('selfupdate-progress', sseHandlers.get('selfupdate-progress'))
    // 卸载后推进时钟不炸（定时器已清）
    await vi.advanceTimersByTimeAsync(60000)
  })
})
