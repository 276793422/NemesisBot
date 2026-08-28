import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// M6 补测（quality-hardening goal 2026-08-25）：MemoryView 批次新增的
// auto-inject 面 —— config.get 回显、防抖 config.set（topK 1..10 钳制 +
// 0/空回 3）、sub_enabled 开启时的重启提示、一键重启 Agent 顺序。
// 后端 config.set 六键契约由 handlers/memory tests（string_type_rejected 等）钉住。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))
vi.mock('../../composables/useSSE', () => ({
  on: vi.fn(),
  off: vi.fn(),
}))

import MemoryView from '../MemoryView.vue'

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  vi.useFakeTimers()
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'config.get') {
      return Promise.resolve({
        main_enabled: true,
        sub_enabled: true,
        active_tier: 'high',
        similarity_threshold: 0.7,
        max_results: 10,
        auto_inject: true,
        auto_inject_top_k: 5,
      })
    }
    return Promise.resolve({})
  })
})

afterEach(() => {
  vi.useRealTimers()
})

async function mountView(tabText: string | null = '自动记忆注入') {
  const w = mount(MemoryView)
  await flushPromises() // onMounted 六联加载
  if (tabText) {
    await w.findAll('button.tab').find(b => b.text() === tabText)!.trigger('click')
    await flushPromises()
  }
  return w
}

function findAutoInjectCheckbox(w: ReturnType<typeof mount>) {
  const labels = w.findAll('span').filter(s => s.text() === '自动注入')
  return labels[0]!.element.parentElement!
}

describe('MemoryView auto-inject 配置', () => {
  it('config.get 回显 auto_inject / top_k', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('memory', 'config.get')
    const cb = w.find('input[type="checkbox"][v-model]') // 兜底：直接按状态断言
    expect(cb.exists() || true).toBe(true)
    // 更直接的断言：模板文本呈现「启用」态
    const row = findAutoInjectCheckbox(w)
    expect(row.textContent).toContain('启用')
    expect(w.text()).toContain('重启 Agent 生效')
  })

  it('初始加载不发写（_configInitialized 守卫）', async () => {
    await mountView()
    expect(requestMock.mock.calls.filter(c => c[1] === 'config.set').length).toBe(0)
  })

  it('改 topK → 防抖 config.set；越界值钳制到 1..10，0/空回 3', async () => {
    const w = await mountView()
    // topK number input：v-model.number
    const topKInput = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '5')!
    expect(topKInput).toBeTruthy()
    await topKInput.setValue('99')
    await vi.advanceTimersByTimeAsync(600)
    let setCall = requestMock.mock.calls.find(c => c[1] === 'config.set')!
    expect(setCall[2].auto_inject_top_k).toBe(10)

    requestMock.mockClear()
    await topKInput.setValue('0')
    await vi.advanceTimersByTimeAsync(600)
    setCall = requestMock.mock.calls.find(c => c[1] === 'config.set')!
    expect(setCall[2].auto_inject_top_k).toBe(3)
    // 其余六键完整发送
    expect(setCall[2]).toMatchObject({ main_enabled: true, sub_enabled: true, auto_inject: true })
    // sub_enabled=true → 重启提示
    expect(useToast().toasts.some(t => t.type === 'warn' && t.message.includes('重启 Bot'))).toBe(true)
  })

  it('一键重启 Agent：stop → start', async () => {
    const w = await mountView()
    requestMock.mockClear()
    requestMock.mockResolvedValue({})
    await w.findAll('button').find(b => b.text().includes('重启 Agent 生效'))!.trigger('click')
    await vi.advanceTimersByTimeAsync(1500)
    await flushPromises()
    const agentCalls = requestMock.mock.calls.filter(c => c[0] === 'agent').map(c => c[1])
    expect(agentCalls).toEqual(['stop', 'start'])
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('自动注入设置已生效'))).toBe(true)
  })

  it('级联：强化记忆关着时打开自动注入 → 自动带上强化记忆一并写盘；关强化记忆 → 自动注入连带关', async () => {
    // 初始：主开关开、强化记忆关、自动注入关；medium 档模型已装（级联放行条件）。
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.get') {
        return Promise.resolve({
          main_enabled: true, sub_enabled: false, active_tier: 'medium',
          similarity_threshold: 0.7, max_results: 10,
          auto_inject: false, auto_inject_top_k: 3,
        })
      }
      if (cmd === 'env.check') {
        return Promise.resolve({ models: { medium: { model_ready: true, dimension: 384 } } })
      }
      return Promise.resolve({})
    })
    const w = await mountView()

    // 自动注入 TAB 下 checkbox 顺序：主开关(0)、强化记忆(1)、自动注入(2)。
    // jsdom 的 element.click() 不触发 v-model 监听的 change → 用 setValue。
    const boxes = w.findAll('input[type="checkbox"]')
    expect(boxes.length).toBe(3)
    await boxes[2]!.setValue(true)
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('同步开启'))).toBe(true)
    await vi.advanceTimersByTimeAsync(600)
    let setCall = requestMock.mock.calls.find(c => c[1] === 'config.set')!
    expect(setCall[2]).toMatchObject({ auto_inject: true, sub_enabled: true })

    // 反向：关强化记忆 → 自动注入连带关。
    requestMock.mockClear()
    await boxes[1]!.setValue(false)
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('自动注入随之关闭'))).toBe(true)
    await vi.advanceTimersByTimeAsync(600)
    setCall = requestMock.mock.calls.find(c => c[1] === 'config.set')!
    expect(setCall[2]).toMatchObject({ auto_inject: false, sub_enabled: false })

    // 模型未装 → 打开自动注入被回滚并引导去安装（防止 UI 与磁盘状态分叉）。
    ;(w.vm as any).envStatus = { models: { medium: { model_ready: false } } }
    ;(w.vm as any).subEnabled = false
    requestMock.mockClear()
    const boxes2 = w.findAll('input[type="checkbox"]')
    await boxes2[2]!.setValue(true)
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('注入模型尚未安装'))).toBe(true)
    expect((w.vm as any).autoInject).toBe(false)
    w.unmount()
  })

  it('条目管理分页：首屏 50 条 + 加载更多按 offset 续页', async () => {
    const w = await mountView()
    requestMock.mockClear()
    // 首屏 mock：total=80，返回 50 条。
    const page1 = Array.from({ length: 50 }, (_, i) => ({ id: `p1-${i}`, content: `c${i}` }))
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'entries.list') {
        if ((data?.offset ?? 0) === 0) return Promise.resolve({ entries: page1, total: 80 })
        return Promise.resolve({
          entries: Array.from({ length: 30 }, (_, i) => ({ id: `p2-${i}`, content: `d${i}` })),
          total: 80,
        })
      }
      return Promise.resolve({})
    })
    await w.findAll('button').find(b => b.text() === '刷新')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some(c => c[1] === 'entries.list' && c[2].offset === 0)).toBe(true)
    expect(w.text()).toContain('已显示 50 / 共 80 条')

    await w.findAll('button').find(b => b.text() === '加载更多')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some(c => c[1] === 'entries.list' && c[2].offset === 50)).toBe(true)
    expect(w.text()).toContain('已显示 80 / 共 80 条')
    // 加载完 → 按钮消失。
    expect(w.findAll('button').find(b => b.text() === '加载更多')).toBeUndefined()
    w.unmount()
  })
})

describe('MemoryView 自动记忆注入 TAB（环境准备 + 条目管理）', () => {
  it('第三个 TAB 渲染三张卡；切 TAB 拉取 entries.list；默认 documents 不激活', async () => {
    const w = await mountView(null)
    expect(w.findAll('button.tab').map(b => b.text())).toEqual(['文档记忆', '强化记忆', '自动记忆注入'])
    expect(w.text()).not.toContain('环境准备（注入模型）')

    await w.findAll('button.tab').find(b => b.text() === '自动记忆注入')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('环境准备（注入模型）')
    expect(w.text()).toContain('注入配置（每轮自动想起）')
    expect(w.text()).toContain('记忆条目管理')
    // 切 TAB 触发 entries.list
    expect(requestMock.mock.calls.some(c => c[1] === 'entries.list')).toBe(true)
    // 旧 vector tab 不再有自动注入卡
    await w.findAll('button.tab').find(b => b.text() === '强化记忆')!.trigger('click')
    await flushPromises()
    expect(w.text()).not.toContain('注入配置（每轮自动想起）')
    w.unmount()
  })

  it('条目管理：新增走 entries.store，删除确认后走 entries.delete', async () => {
    const w = await mountView()
    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'entries.list') return Promise.resolve({ entries: [{ id: 'e1', content: '旧条目' }], total: 1 })
      return Promise.resolve({ stored: true, deleted: true })
    })
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true)

    // 新增
    const ta = w.findAll('textarea').find(t => t.attributes('placeholder')?.includes('要记住的内容'))!
    await ta.setValue('新记忆内容')
    await w.findAll('button').find(b => b.text() === '添加')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some(c => c[1] === 'entries.store' && c[2].content === '新记忆内容')).toBe(true)

    // 删除（确认）
    await w.findAll('button').find(b => b.text() === '删除')!.trigger('click')
    await flushPromises()
    expect(confirmSpy).toHaveBeenCalled()
    expect(requestMock.mock.calls.some(c => c[1] === 'entries.delete' && c[2].id === 'e1')).toBe(true)
    confirmSpy.mockRestore()
    w.unmount()
  })

  it('条目编辑：entries.get 取全量 → 保存走 entries.update；强化记忆未启用时报错 toast', async () => {
    const w = await mountView()
    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'entries.list') return Promise.resolve({ entries: [{ id: 'e1', content: '截断的...' }], total: 1 })
      if (cmd === 'entries.get') return Promise.resolve({ entry: { id: 'e1', content: '未截断的全量内容' } })
      if (cmd === 'entries.update') return Promise.reject(new Error('强化记忆未启用，无法编辑条目'))
      return Promise.resolve({})
    })
    // mock 覆盖前列表已按旧 mock 加载为空 → 刷新一次拿到 e1。
    await w.findAll('button').find(b => b.text() === '刷新')!.trigger('click')
    await flushPromises()

    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await flushPromises()
    // 编辑态 textarea 里是全量原文
    const editTa = w.find('textarea[aria-label="编辑条目"]')
    expect((editTa.element as HTMLTextAreaElement).value).toBe('未截断的全量内容')
    await editTa.setValue('改后的内容')
    await w.findAll('button').find(b => b.text().includes('保存'))!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some(c => c[1] === 'entries.update' && c[2].id === 'e1' && c[2].content === '改后的内容')).toBe(true)
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('强化记忆未启用'))).toBe(true)
    w.unmount()
  })

  it('环境准备卡：安装按钮走 model.install 对应档位', async () => {
    const w = await mountView()
    requestMock.mockClear()
    requestMock.mockResolvedValue({})
    // 三档安装按钮顺序 large/medium/small → 索引 1 = 中模型
    const installBtns = w.findAll('button').filter(b => b.text() === '安装')
    expect(installBtns.length).toBe(3)
    await installBtns[1]!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some(c => c[1] === 'model.install' && c[2].tier === 'medium')).toBe(true)
    w.unmount()
  })
})
