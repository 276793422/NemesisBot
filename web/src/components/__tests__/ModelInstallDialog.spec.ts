import { mount } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// ModelInstallDialog（2026-10-25 模型设置弹窗）：
// - 渲染该类全部可装变体（catalog 数据 + 描述回退）；
// - 安装流：install_model 带 name → SSE voice-setup 行内进度 → 完成刷新；
// - 单飞：一行安装中其余【安装】禁用；
// - 设为当前：set_model 成功 → 徽标切换 + 「语音引擎重启后生效」行内提示；
// - 唯一关闭途径 = 显式【关闭】按钮：backdrop 点击不关（硬需求）。

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

import ModelInstallDialog from '../ModelInstallDialog.vue'

const VARIANTS = [
  { name: 'kokoro-a', repo: 'u/a', description: 'A 模型描述', file_count: 2, installed: true, is_current: true },
  { name: 'kokoro-b', repo: 'u/b', description: 'B 模型描述', file_count: 3, installed: true, is_current: false },
  { name: 'kokoro-c', repo: 'u/c', description: '', file_count: 1, installed: false, is_current: false },
]

function catalogResponse() {
  return { categories: { tts: VARIANTS } }
}

// 默认 request 路由：catalog 即返；install_model 挂起（测试手动放行）；
// set_model 即返成功。
let resolveInstall: (v: unknown) => void = () => {}
function defaultRequestRoute() {
  wsapiRequest.mockImplementation((_mod: string, cmd: string) => {
    if (cmd === 'model_catalog') return Promise.resolve(catalogResponse())
    if (cmd === 'install_model')
      return new Promise((resolve) => {
        resolveInstall = resolve
      })
    if (cmd === 'set_model') return Promise.resolve({ success: true })
    return Promise.resolve({})
  })
}

async function mounted() {
  const w = mount(ModelInstallDialog, { props: { category: 'tts', label: 'TTS' } })
  await vi.dynamicImportSettled()
  return w
}

function buttonByText(w: ReturnType<typeof mount>, text: string) {
  return w.findAll('button').find((b) => b.text().trim() === text)
}

beforeEach(() => {
  sseHandlers.clear()
  resolveInstall = () => {}
  defaultRequestRoute()
})

describe('ModelInstallDialog', () => {
  it('渲染全部变体：名称/描述/当前徽标/未安装状态', async () => {
    const w = await mounted()
    const html = w.html()
    expect(html).toContain('kokoro-a')
    expect(html).toContain('A 模型描述')
    expect(html).toContain('当前使用')
    // 空描述回退显示 repo
    expect(html).toContain('u/c')
    w.unmount()
  })

  it('安装点击带 name 调 install_model；SSE 进度行内可见', async () => {
    const w = await mounted()
    const installC = buttonByText(w, '安装')
    expect(installC).toBeTruthy()
    await installC!.trigger('click')
    expect(wsapiRequest).toHaveBeenCalledWith(
      'voice',
      'install_model',
      { model: 'tts', name: 'kokoro-c' },
      0,
    )

    // SSE progress（phase=model + name 匹配行）→ 行内进度文本
    sseHandlers.get('voice-setup')!({
      phase: 'model',
      status: 'progress',
      name: 'kokoro-c',
      message: '42% · 下载中',
    })
    await vi.dynamicImportSettled()
    expect(w.html()).toContain('42% · 下载中')

    // 别的行/别的 phase 的进度不串台
    sseHandlers.get('voice-setup')!({
      phase: 'model',
      status: 'progress',
      name: 'kokoro-a',
      message: '不该出现',
    })
    await vi.dynamicImportSettled()
    expect(w.html()).not.toContain('不该出现')

    resolveInstall({ success: true })
    await vi.dynamicImportSettled()
    // 完成后重新拉目录
    expect(wsapiRequest).toHaveBeenCalledWith('voice', 'model_catalog')
    w.unmount()
  })

  it('单飞：一行安装中，其余【安装】按钮禁用', async () => {
    const w = await mounted()
    await buttonByText(w, '安装')!.trigger('click')
    await vi.dynamicImportSettled()

    // 唯一未安装行正在安装（文本变「安装中」）；其余安装按钮不存在了，
    // 但【设为当前】不受安装锁影响（仍可点）。
    expect(w.html()).toContain('安装中')
    const setCurrent = buttonByText(w, '设为当前')
    expect(setCurrent).toBeTruthy()
    expect(setCurrent!.attributes('disabled')).toBeUndefined()

    resolveInstall({ success: true })
    await vi.dynamicImportSettled()
    w.unmount()
  })

  it('已安装非当前行【设为当前】：set_model 调用 + 重启生效提示', async () => {
    const w = await mounted()
    await buttonByText(w, '设为当前')!.trigger('click')
    await vi.dynamicImportSettled()

    expect(wsapiRequest).toHaveBeenCalledWith('voice', 'set_model', {
      category: 'tts',
      name: 'kokoro-b',
    })
    expect(w.html()).toContain('语音引擎重启后生效')
    w.unmount()
  })

  it('点 backdrop（失焦）不关闭', async () => {
    const w = await mounted()
    await w.find('.modal-backdrop').trigger('click')
    expect(w.emitted('close')).toBeFalsy()
    w.unmount()
  })

  it('仅显式【关闭】按钮可关闭（header 与 footer 两处）', async () => {
    const w = await mounted()
    const closeButtons = w.findAll('button').filter((b) => b.text().trim() === '关闭')
    expect(closeButtons.length).toBe(2)

    await closeButtons[0].trigger('click')
    expect(w.emitted('close')).toBeTruthy()

    const w2 = await mounted()
    const close2 = w2.findAll('button').filter((b) => b.text().trim() === '关闭')
    await close2[1].trigger('click')
    expect(w2.emitted('close')).toBeTruthy()
    w.unmount()
    w2.unmount()
  })
})
