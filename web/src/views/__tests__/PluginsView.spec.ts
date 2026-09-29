import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// 2026-09-29 三 Tab 重构：默认落 WASM Tab（主角：是什么/怎么做引导 +
// devkit 下载 + 管理面）；本地插件库 / 管线插件各自成 Tab。契约（
// request('plugins','list') 精确形态）与错误降级不变。后端行为由
// handlers/plugins/tests.rs（含 dispatch 级）钉住。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import PluginsView from '../PluginsView.vue'

const LIST = {
  pipeline_plugins: [
    { name: 'metrics-pipeline', scope: null, enabled: true, description: '每工具调用计时（around 段参考实现）' },
  ],
  plugins: [
    {
      id: 'plugin_onnx',
      label: 'ONNX 嵌入推理',
      used_by: '强化记忆 / 自动记忆注入',
      found: true,
      filename: 'plugin_onnx.dll',
      path: 'C:\\bot\\plugins\\plugin_onnx.dll',
      capabilities: ['embedding 推理'],
      detail: {
        enhanced_memory_enabled: true,
        active_tier: 'medium',
        active_model: 'all-MiniLM-L6-v2',
        model_ready: true,
      },
    },
    {
      id: 'plugin_ui',
      label: 'WebView UI / 系统托盘',
      used_by: 'desktop 集成',
      found: false,
      filename: 'plugin_ui.dll',
    },
  ],
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'list') return Promise.resolve(LIST)
    if (cmd === 'wasm.list') return Promise.resolve({ plugins: [] })
    return Promise.resolve({})
  })
})

async function mountView() {
  const w = mount(PluginsView)
  await flushPromises()
  return w
}

async function switchTab(w: ReturnType<typeof mount>, label: string) {
  const tab = w.findAll('.tab').find(b => b.text() === label)
  expect(tab, `找不到 Tab「${label}」`).toBeTruthy()
  await tab!.trigger('click')
  await flushPromises()
}

describe('PluginsView 三 Tab 重构（WASM 主角 + 本地库 + 管线）', () => {
  it('契约：挂载即 request("plugins","list")；默认 Tab = WASM（引导 + devkit 下载钮）', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('plugins', 'list')
    expect(requestMock).toHaveBeenCalledWith('plugins', 'wasm.list')
    // 默认 Tab 是 WASM：引导文案与下载按钮在场
    expect(w.text()).toContain('WASM 插件是什么')
    expect(w.text()).toContain('怎么开发一个插件')
    expect(w.text()).toContain('下载插件开发包')
    // 本地库内容在另一 Tab，默认不渲染
    expect(w.text()).not.toContain('1/2 已就绪')
    w.unmount()
  })

  it('devkit 下载：点击走 wasm.devkit_download（overwrite=false），结果面板展示解压根', async () => {
    const w = await mountView()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(LIST)
      if (cmd === 'wasm.list') return Promise.resolve({ plugins: [] })
      if (cmd === 'wasm.devkit_download')
        return Promise.resolve({ path: 'C:\\ws\\wasm-plugin-devkit\\nightly-wasm-devkit.zip', dir: 'C:\\ws\\wasm-plugin-devkit\\devkit', size: 2048, files: 42 })
      return Promise.resolve({})
    })
    const btn = w.findAll('button').find(b => b.text().includes('下载插件开发包'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('plugins', 'wasm.devkit_download', { overwrite: false })
    expect(w.text()).toContain('C:\\ws\\wasm-plugin-devkit\\devkit')
    expect(w.text()).toContain('2.0 KB')
    // 二次下载带覆盖复选
    const box = w.find('input[type="checkbox"]')
    await box.setValue(true)
    await btn!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenLastCalledWith('plugins', 'wasm.devkit_download', { overwrite: true })
    w.unmount()
  })

  it('本地插件库 Tab：切换后渲染就绪状态、能力与 onnx detail', async () => {
    const w = await mountView()
    await switchTab(w as any, '本地插件库')
    expect(w.text()).toContain('1/2 已就绪')
    expect(w.text()).toContain('plugin_onnx')
    expect(w.text()).toContain('已就绪')
    expect(w.text()).toContain('C:\\bot\\plugins\\plugin_onnx.dll')
    expect(w.text()).toContain('embedding 推理')
    expect(w.text()).toContain('模型就绪')
    // 未找到的 ui 插件
    expect(w.text()).toContain('plugin_ui')
    expect(w.text()).toContain('未找到')
    w.unmount()
  })

  it('onnx detail：模型未安装 → 红字提示', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') {
        return Promise.resolve({
          plugins: [
            {
              ...LIST.plugins[0],
              detail: { enhanced_memory_enabled: true, active_tier: 'medium', active_model: 'all-MiniLM-L6-v2', model_ready: false },
            },
            LIST.plugins[1],
          ],
        })
      }
      if (cmd === 'wasm.list') return Promise.resolve({ plugins: [] })
      return Promise.resolve({})
    })
    const w = await mountView()
    await switchTab(w as any, '本地插件库')
    expect(w.text()).toContain('模型未安装')
    w.unmount()
  })

  it('list 失败 → 错误 toast、不崩溃', async () => {
    requestMock.mockRejectedValue(new Error('ws down'))
    const w = await mountView()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('加载插件状态失败'))).toBe(true)
    w.unmount()
  })

  it('管线插件 Tab：切换后渲染启停开关；切换走 set_metrics_enabled', async () => {
    const w = await mountView()
    await switchTab(w as any, '管线插件')
    expect(w.text()).toContain('管线插件')
    expect(w.text()).toContain('metrics-pipeline')

    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(LIST)
      if (cmd === 'set_metrics_enabled') return Promise.resolve({ name: 'metrics-pipeline', enabled: false })
      return Promise.resolve({})
    })
    const card = w.findAll('.card').find(c => c.text().includes('管线插件'))
    expect(card).toBeTruthy()
    const box = card!.find('input[type="checkbox"]')
    expect(box.exists()).toBe(true)
    await box.setValue(false)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('plugins', 'set_metrics_enabled', { enabled: false })
    expect(useToast().toasts.some(t => t.message.includes('已停用'))).toBe(true)
    w.unmount()
  })
})
