import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// 2026-08-29：PluginsView phase 1（只读总览）——plugins.list 回显渲染、
// 错误降级、契约（request('plugins','list') 精确形态）。
// 后端行为由 handlers/plugins/tests.rs（含 dispatch 级）钉住。

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
  features: [
    { id: 'memory', label: '强化记忆', enabled: true },
    { id: 'sandbox', label: '沙盒', enabled: false },
  ],
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'list') return Promise.resolve(LIST)
    return Promise.resolve({})
  })
})

async function mountView() {
  const w = mount(PluginsView)
  await flushPromises()
  return w
}

describe('PluginsView 插件状态总览（phase 1 只读）', () => {
  it('契约：挂载即 request("plugins","list")；渲染就绪状态与能力', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('plugins', 'list')
    expect(w.text()).toContain('1/2 已就绪')
    expect(w.text()).toContain('plugin_onnx')
    expect(w.text()).toContain('已就绪')
    expect(w.text()).toContain('C:\\bot\\plugins\\plugin_onnx.dll')
    expect(w.text()).toContain('embedding 推理')
    // onnx detail 行（模型就绪）
    expect(w.text()).toContain('模型就绪')
    // 未找到的 ui 插件
    expect(w.text()).toContain('plugin_ui')
    expect(w.text()).toContain('未找到')
    // feature 区块
    expect(w.text()).toContain('强化记忆 · 开')
    expect(w.text()).toContain('沙盒 · 关')
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
          features: LIST.features,
        })
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(w.text()).toContain('模型未安装')
    w.unmount()
  })

  it('list 失败 → 错误 toast、不崩溃', async () => {
    requestMock.mockRejectedValue(new Error('ws down'))
    const w = await mountView()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('加载插件状态失败'))).toBe(true)
    w.unmount()
  })

  it('管线插件：渲染启停开关；切换走 set_metrics_enabled', async () => {
    const w = await mountView()
    expect(w.text()).toContain('管线插件')
    expect(w.text()).toContain('metrics-pipeline')

    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(LIST)
      if (cmd === 'set_metrics_enabled') return Promise.resolve({ name: 'metrics-pipeline', enabled: false })
      return Promise.resolve({})
    })
    const boxes = w.findAll('input[type="checkbox"]')
    expect(boxes.length).toBe(1)
    await boxes[0]!.setValue(false)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('plugins', 'set_metrics_enabled', { enabled: false })
    expect(useToast().toasts.some(t => t.message.includes('已停用'))).toBe(true)
    w.unmount()
  })
})
