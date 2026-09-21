import { describe, it, expect, vi, beforeEach } from 'vitest'

// useEditorMode（2026-09-20 Full Access 放行开关状态单例）：
// - SSE `editor-mode` → refs 跟随（任一窗口翻转，所有窗口跟随）；
// - `editor.get` seed 对齐（后端运行时态：进程重启一律双关）；
// - setEditorAccess 以响应（服务端生效值）为准——联动收口在服务端；
// - 「未装配」错误 → editorAvailable=false（按钮诚实禁用，不重试）；
//   其余 seed 失败静默退避重试（上限 3 次，WS 未就绪只是暂时）；
//   set 失败 toast。
// useSSE / useWSAPI / useToast 全部打桩。

const sseHandlers = new Map<string, (data?: unknown) => void>()
const wsapiRequest = vi.fn()

vi.mock('../useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    sseHandlers.set(type, handler)
  }),
  off: vi.fn(),
}))

vi.mock('../useWSAPI', () => ({
  useWSAPI: () => ({ request: wsapiRequest }),
}))

const toasts: Array<{ msg: string; type: string }> = []
vi.mock('../useToast', () => ({
  useToast: () => ({
    toasts: [],
    info: (m: string) => toasts.push({ msg: m, type: 'info' }),
    success: (m: string) => toasts.push({ msg: m, type: 'success' }),
    warn: (m: string) => toasts.push({ msg: m, type: 'warn' }),
    error: (m: string) => toasts.push({ msg: m, type: 'error' }),
    remove: vi.fn(),
  }),
}))

import { useEditorMode, fullAccess, externalWrite, editorAvailable, _resetEditorModeForTest } from '../useEditorMode'

function fireEditorMode(data: Record<string, unknown>) {
  sseHandlers.get('editor-mode')!(data)
}

beforeEach(() => {
  _resetEditorModeForTest()
  sseHandlers.clear()
  wsapiRequest.mockReset()
  wsapiRequest.mockResolvedValue({})
  toasts.length = 0
})

describe('useEditorMode', () => {
  it('initEditorMode 订阅 SSE + editor.get seed 对齐', async () => {
    wsapiRequest.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'get') return Promise.resolve({ full_access: true, external_write: false })
      return Promise.resolve({})
    })
    const { initEditorMode } = useEditorMode()
    initEditorMode()
    expect(wsapiRequest).toHaveBeenCalledWith('editor', 'get', {}, 5000)
    await vi.waitFor(() => {
      expect(fullAccess.value).toBe(true)
      expect(externalWrite.value).toBe(false)
    })
  })

  it('initEditorMode 幂等：重复调用不重复 seed', () => {
    const { initEditorMode } = useEditorMode()
    initEditorMode()
    initEditorMode()
    expect(wsapiRequest).toHaveBeenCalledTimes(1)
  })

  it('SSE editor-mode 事件 → refs 跟随（跨窗口刷新通道）', () => {
    const { initEditorMode } = useEditorMode()
    initEditorMode()
    fireEditorMode({ full_access: true, external_write: true })
    expect(fullAccess.value).toBe(true)
    expect(externalWrite.value).toBe(true)
    fireEditorMode({ full_access: false, external_write: false })
    expect(fullAccess.value).toBe(false)
  })

  it('SSE 载荷缺键/非 bool 不覆盖对应位', () => {
    const { initEditorMode } = useEditorMode()
    initEditorMode()
    fireEditorMode({ full_access: true })
    expect(externalWrite.value).toBe(false)
    fireEditorMode({ full_access: 'yes' })
    expect(fullAccess.value).toBe(true)
  })

  it('setEditorAccess 以服务端响应为准（联动收口在服务端）', async () => {
    // 前端发 (true, false)，服务端返回生效值（此处模拟他人并发开 ext 的
    // latest-wins 结果）——本地必须以回包覆盖。
    wsapiRequest.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'set') return Promise.resolve({ full_access: true, external_write: true })
      return Promise.resolve({})
    })
    const { setEditorAccess } = useEditorMode()
    await setEditorAccess(true, false)
    expect(wsapiRequest).toHaveBeenCalledWith('editor', 'set', {
      full_access: true,
      external_write: false,
    })
    expect(fullAccess.value).toBe(true)
    expect(externalWrite.value).toBe(true)
  })

  it('setEditorAccess 失败 → toast + 本地不翻转', async () => {
    wsapiRequest.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'set') return Promise.reject(new Error('boom'))
      return Promise.resolve({})
    })
    const { setEditorAccess } = useEditorMode()
    await setEditorAccess(true, false)
    expect(fullAccess.value).toBe(false)
    expect(toasts.some(t => t.type === 'error' && t.msg.includes('boom'))).toBe(true)
  })

  it('「未装配」错误 → editorAvailable=false(诚实禁用,且不重试)', async () => {
    wsapiRequest.mockRejectedValue('编辑器放行未装配(security 未启用或旧装配)')
    const { initEditorMode, setEditorAccess } = useEditorMode()
    initEditorMode()
    await vi.waitFor(() => {
      expect(editorAvailable.value).toBe(false)
    })
    expect(wsapiRequest).toHaveBeenCalledTimes(1)
    await setEditorAccess(true, false)
    expect(fullAccess.value).toBe(false)
    expect(editorAvailable.value).toBe(false)
  })

  it('seed 暂时失败 → 退避重试后收敛(静默无 toast)', async () => {
    vi.useFakeTimers()
    try {
      // 前两次失败,第三次成功 → 必须收敛(否则晚连窗口停在旧态)。
      wsapiRequest
        .mockRejectedValueOnce(new Error('timeout'))
        .mockRejectedValueOnce(new Error('timeout'))
        .mockResolvedValue({ full_access: true, external_write: false })
      const { initEditorMode } = useEditorMode()
      initEditorMode()
      await vi.advanceTimersByTimeAsync(0) // 首次 seed 失败 → 挂 3s 重试
      expect(wsapiRequest).toHaveBeenCalledTimes(1)
      await vi.advanceTimersByTimeAsync(3000) // 重试1(3s×1)失败 → 挂 6s
      expect(wsapiRequest).toHaveBeenCalledTimes(2)
      await vi.advanceTimersByTimeAsync(6000) // 重试2(3s×2)成功 → 收敛
      expect(wsapiRequest).toHaveBeenCalledTimes(3)
      expect(fullAccess.value).toBe(true)
      expect(externalWrite.value).toBe(false)
      expect(toasts).toHaveLength(0)
    } finally {
      vi.useRealTimers()
    }
  })

  it('seed 连续失败超过上限(3 次)后放弃,available 保持 true', async () => {
    vi.useFakeTimers()
    try {
      wsapiRequest.mockRejectedValue(new Error('timeout'))
      const { initEditorMode } = useEditorMode()
      initEditorMode()
      await vi.advanceTimersByTimeAsync(0) // 初始失败
      await vi.advanceTimersByTimeAsync(3000) // 重试1(3s)
      await vi.advanceTimersByTimeAsync(6000) // 重试2(6s)
      await vi.advanceTimersByTimeAsync(9000) // 重试3(9s)
      expect(wsapiRequest).toHaveBeenCalledTimes(4)
      await vi.advanceTimersByTimeAsync(60_000)
      expect(wsapiRequest).toHaveBeenCalledTimes(4)
      expect(editorAvailable.value).toBe(true)
      expect(toasts).toHaveLength(0)
    } finally {
      vi.useRealTimers()
    }
  })
})
