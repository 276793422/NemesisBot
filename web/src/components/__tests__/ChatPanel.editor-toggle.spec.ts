import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// ChatPanel Full Access 放行开关（2026-09-20 用户裁决，仿 codex）：
// - 工具栏两按钮（⚡ Full Access / 外部写删）+ editor-strip 常驻条；
// - 点击发 editor.set（关 FA 随关 ext；开 ext 确保 full 开）；
// - 以服务端生效值呈现（active 态 / disabled 联动）；
// - 「未装配」→ 按钮诚实禁用。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn(),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: ref('connected'),
  }
})

import ChatPanel from '../ChatPanel.vue'
import { useEditorMode, _resetEditorModeForTest } from '../../composables/useEditorMode'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  useSessionStore().currentId = 's1'
  _resetEditorModeForTest()
  requestMock.mockReset()
  requestMock.mockImplementation((_mod: string, cmd: string) => {
    if (cmd === 'get_mode') return Promise.resolve({ mode: 'build' })
    if (cmd === 'get') return Promise.resolve({ full_access: false, external_write: false })
    return Promise.resolve({})
  })
})

async function mountPanel() {
  const w = mount(ChatPanel)
  await flushPromises()
  return w
}

function fullBtn(w: ReturnType<typeof mount>) {
  return w.find('.editor-full-btn')
}
function extBtn(w: ReturnType<typeof mount>) {
  return w.find('.editor-ext-btn')
}

describe('ChatPanel Full Access 开关', () => {
  it('初始双关：按钮渲染但不 active，常驻条不出现，ext 禁用', async () => {
    const w = await mountPanel()
    expect(fullBtn(w).exists()).toBe(true)
    expect(fullBtn(w).classes()).not.toContain('active')
    expect(extBtn(w).attributes('disabled')).toBeDefined()
    expect(w.find('.editor-strip').exists()).toBe(false)
    w.unmount()
  })

  it('点击 FA → editor.set (true, false) → active + 常驻条出现', async () => {
    const w = await mountPanel()
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'set') return Promise.resolve({ full_access: true, external_write: false })
      if (cmd === 'get_mode') return Promise.resolve({ mode: 'build' })
      return Promise.resolve({})
    })
    await fullBtn(w).trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('editor', 'set', {
      full_access: true,
      external_write: false,
    })
    expect(fullBtn(w).classes()).toContain('active')
    expect(w.find('.editor-strip').exists()).toBe(true)
    expect(w.find('.editor-strip').text()).toContain('项目外写删仍审批')
    // ext 解禁
    expect(extBtn(w).attributes('disabled')).toBeUndefined()
    w.unmount()
  })

  it('开 ext → editor.set (true, true)，常驻条切「全放行」文案', async () => {
    const w = await mountPanel()
    // 服务端 echo 请求参数为生效值（真实语义：set_flags 内嵌联动后返回生效值）
    requestMock.mockImplementation((_mod: string, cmd: string, data?: any) => {
      if (cmd === 'set') return Promise.resolve({ full_access: data?.full_access ?? false, external_write: data?.external_write ?? false })
      if (cmd === 'get_mode') return Promise.resolve({ mode: 'build' })
      return Promise.resolve({})
    })
    await fullBtn(w).trigger('click')
    await flushPromises()
    await extBtn(w).trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenLastCalledWith('editor', 'set', {
      full_access: true,
      external_write: true,
    })
    expect(extBtn(w).classes()).toContain('active')
    expect(w.find('.editor-strip').text()).toContain('全放行中')
    w.unmount()
  })

  it('再点 FA（开→关）→ editor.set (false, false)，常驻条消失', async () => {
    const w = await mountPanel()
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'set') return Promise.resolve({ full_access: false, external_write: false })
      if (cmd === 'get_mode') return Promise.resolve({ mode: 'build' })
      return Promise.resolve({})
    })
    // 先经 SSE 通道把状态置开（composable 单例直写），再点关。
    useEditorMode().fullAccess.value = true
    await flushPromises()
    expect(fullBtn(w).classes()).toContain('active')
    await fullBtn(w).trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('editor', 'set', {
      full_access: false,
      external_write: false,
    })
    expect(fullBtn(w).classes()).not.toContain('active')
    expect(w.find('.editor-strip').exists()).toBe(false)
    w.unmount()
  })

  it('set 失败 → 以服务端回包为准的本地态不翻转', async () => {
    const w = await mountPanel()
    requestMock.mockImplementation((_mod: string, cmd: string) => {
      if (cmd === 'set') return Promise.reject(new Error('boom'))
      if (cmd === 'get_mode') return Promise.resolve({ mode: 'build' })
      return Promise.resolve({})
    })
    await fullBtn(w).trigger('click')
    await flushPromises()
    expect(fullBtn(w).classes()).not.toContain('active')
    w.unmount()
  })

  it('「未装配」→ editorAvailable=false → 按钮诚实禁用', async () => {
    // seed 前先触发 init（AppLayout 生产时序），get 报未装配。
    requestMock.mockRejectedValue('编辑器放行未装配(security 未启用或旧装配)')
    useEditorMode().initEditorMode()
    await flushPromises()
    const w = await mountPanel()
    expect(fullBtn(w).attributes('disabled')).toBeDefined()
    w.unmount()
  })
})
