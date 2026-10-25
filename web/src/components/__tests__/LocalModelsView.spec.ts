import { mount } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// LocalModelsView 进度条（2026-10-25 运行库安装反馈修复）：
// - 点击【安装】立即进入安装态（按钮禁用 + 文案变化，不等 SSE）；
// - SSE voice-setup progress（phase=runtime）→ 行内进度文本 + 进度条宽度；
// - pct 缺席 → 不确定条；complete/error → 清理并刷新 status。

const sseHandlers = new Map<string, (data?: unknown) => void>()
const wsapiRequest = vi.fn()

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
    warn: vi.fn(),
    error: vi.fn(),
    remove: vi.fn(),
  }),
}))

import LocalModelsView from '../../views/LocalModelsView.vue'

function statusResponse(ready: boolean) {
  return {
    all_dlls_present: ready,
    config_exists: true,
    dlls: [
      { name: 'sherpa-onnx-c-api.dll', exists: ready, size_bytes: ready ? 1024 : null },
      { name: 'onnxruntime.dll', exists: ready, size_bytes: null },
      { name: 'onnxruntime_providers_shared.dll', exists: ready, size_bytes: null },
    ],
  }
}

// install_runtime 挂起（测试手动放行）；status 即返未就绪态。
let resolveInstall: (v: unknown) => void = () => {}
beforeEach(() => {
  sseHandlers.clear()
  resolveInstall = () => {}
  wsapiRequest.mockImplementation((_mod: string, cmd: string) => {
    if (cmd === 'status') return Promise.resolve(statusResponse(false))
    if (cmd === 'install_runtime')
      return new Promise((resolve) => {
        resolveInstall = resolve
      })
    return Promise.resolve({})
  })
})

async function mounted() {
  const w = mount(LocalModelsView)
  await vi.dynamicImportSettled()
  return w
}

describe('LocalModelsView 运行库安装进度', () => {
  it('点击【安装】立即进入安装态：按钮禁用 + 文案「安装中...」', async () => {
    const w = await mounted()
    const btn = w.findAll('button').find((b) => b.text().trim() === '安装')
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    await vi.dynamicImportSettled()

    const btnAfter = w.findAll('button').find((b) => b.text().includes('安装中'))
    expect(btnAfter).toBeTruthy()
    expect(btnAfter!.attributes('disabled')).toBeDefined()
    expect(w.html()).toContain('正在安装 onnx 运行库...')

    resolveInstall({ success: true })
    await vi.dynamicImportSettled()
    w.unmount()
  })

  it('SSE progress 事件 → 进度文本 + 进度条宽度（百分比）', async () => {
    const w = await mounted()
    await w.findAll('button').find((b) => b.text().trim() === '安装')!.trigger('click')
    await vi.dynamicImportSettled()

    sseHandlers.get('voice-setup')!({
      phase: 'runtime',
      status: 'progress',
      stage: 'download',
      pct: 42.5,
      message: '下载运行库 4.7 / 11.0 MB（43%）· 2.1 MB/s',
    })
    await vi.dynamicImportSettled()

    expect(w.html()).toContain('下载运行库 4.7 / 11.0 MB（43%）· 2.1 MB/s')
    const bar = w.find('.lm-progress-bar')
    expect(bar.exists()).toBe(true)
    expect(bar.attributes('style')).toContain('42.5%')
    expect(bar.classes()).not.toContain('indeterminate')

    resolveInstall({ success: true })
    await vi.dynamicImportSettled()
    w.unmount()
  })

  it('pct 缺席（解压/加载阶段）→ 不确定条；complete 后清理', async () => {
    const w = await mounted()
    await w.findAll('button').find((b) => b.text().trim() === '安装')!.trigger('click')
    await vi.dynamicImportSettled()

    sseHandlers.get('voice-setup')!({
      phase: 'runtime',
      status: 'progress',
      stage: 'extract',
      pct: null,
      message: '下载完成，解压运行库...',
    })
    await vi.dynamicImportSettled()

    expect(w.find('.lm-progress-bar').classes()).toContain('indeterminate')
    expect(w.html()).toContain('下载完成，解压运行库...')

    sseHandlers.get('voice-setup')!({
      phase: 'runtime',
      status: 'complete',
      message: '运行库安装完成',
    })
    await vi.dynamicImportSettled()
    // complete → 重新拉 status
    expect(wsapiRequest).toHaveBeenCalledWith('voice', 'status')
    resolveInstall({ success: true })
    await vi.dynamicImportSettled()
    w.unmount()
  })
})
