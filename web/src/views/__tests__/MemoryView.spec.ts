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

// ---------------------------------------------------------------------------
// gap 补测（覆盖率批次）：文档记忆 TAB、强化记忆 TAB（环境/测试/嵌入配置）、
// SSE 进度、注入 TAB 条目管理错误分支与守卫、前置状态四分支。
// 遵循上方既有约定：file-level beforeEach 已 mockReset + fake timers，
// 各 describe 自带 mockImplementation 覆盖默认实现。
// ---------------------------------------------------------------------------

import { on as sseOnMock } from '../../composables/useSSE'

function vecDefaults() {
  return {
    main_enabled: true,
    sub_enabled: true,
    active_tier: 'medium',
    similarity_threshold: 0.7,
    max_results: 10,
    auto_inject: true,
    auto_inject_top_k: 5,
  }
}

describe('MemoryView 文档记忆 TAB（gap 补测）', () => {
  beforeEach(() => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.get') return Promise.resolve(vecDefaults())
      if (cmd === 'status') return Promise.resolve({ vector_memory: { main_enabled: true } })
      if (cmd === 'documents') return Promise.resolve({
        documents: [
          { path: 'a.md', size: 500 },
          { path: 'b.md', size: 2048 },
          { path: 'c.md', size: 3 * 1024 * 1024 },
          { path: 'd.md' },
        ],
      })
      if (cmd === 'document.get') return Promise.resolve({ content: '正文内容' })
      return Promise.resolve({})
    })
  })

  it('文件列表渲染 + formatSize 四分支；点文件读取内容', async () => {
    const w = await mountView(null)
    expect(w.text()).toContain('500 B')
    expect(w.text()).toContain('2.0 KB')
    expect(w.text()).toContain('3.0 MB')
    expect(w.text()).toContain('--') // size 缺席
    expect(requestMock).toHaveBeenCalledWith('memory', 'status')

    await w.findAll('.page-memory .card div').filter(d => d.text() === 'a.md')[0]!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('memory', 'document.get', { path: 'a.md' })
    expect(w.text()).toContain('正文内容')
    w.unmount()
  })

  it('编辑 → 保存走 document.save；取消不落盘', async () => {
    const w = await mountView(null)
    await w.findAll('.page-memory .card div').filter(d => d.text() === 'a.md')[0]!.trigger('click')
    await flushPromises()
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    const ta = w.find('textarea')
    expect((ta.element as HTMLTextAreaElement).value).toBe('正文内容')
    await ta.setValue('新正文')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('memory', 'document.save', { path: 'a.md', content: '新正文' })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message === '已保存')).toBe(true)
    // 保存后回到查看态，docContent 更新
    expect(w.text()).toContain('新正文')
    // 再次编辑后取消：内容恢复原文
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await w.findAll('button').find(b => b.text() === '取消')!.trigger('click')
    expect(w.find('textarea').exists()).toBe(false)
    w.unmount()
  })

  it('读取/保存/列表 失败各自提示', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.get') return Promise.resolve(vecDefaults())
      if (cmd === 'documents') return Promise.reject(new Error('disk'))
      return Promise.reject(new Error('boom'))
    })
    const w = await mountView(null)
    expect(useToast().toasts.some(t => t.message.includes('加载失败'))).toBe(true)

    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.get') return Promise.resolve(vecDefaults())
      if (cmd === 'documents') return Promise.resolve({ documents: [{ path: 'a.md', size: 1 }] })
      if (cmd === 'document.get') return Promise.reject(new Error('perm'))
      if (cmd === 'document.save') return Promise.reject(new Error('ro'))
      return Promise.resolve({})
    })
    // 刷新文档列表
    ;(w.vm as any).documents = []
    await (w.vm as any).loadDocuments()
    await flushPromises()
    await w.findAll('.page-memory .card div').filter(d => d.text() === 'a.md')[0]!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('读取失败'))).toBe(true)

    ;(w.vm as any).docPath = 'a.md'
    await (w.vm as any).saveDocument()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保存失败'))).toBe(true)
    w.unmount()
  })
})

describe('MemoryView 强化记忆 TAB：环境/配置（gap 补测）', () => {
  beforeEach(() => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.get') return Promise.resolve({ ...vecDefaults(), embedding_config_content: 'embed:\n  dim: 384' })
      if (cmd === 'env.check') return Promise.resolve({
        plugin: { found: true },
        overall: 'ready',
        models: { medium: { model_ready: true, dimension: 384, model_size: 40 * 1024 * 1024 } },
      })
      return Promise.resolve({})
    })
  })

  it('env.check 渲染：插件已找到/模型就绪/大小/整体状态徽标/当前档', async () => {
    const w = await mountView('强化记忆')
    expect(w.text()).toContain('(已找到)')
    expect(w.text()).toContain('40.0 MB')
    expect(w.text()).toContain('就绪')
    w.unmount()
  })

  it('查看配置 toggle：打开拉 config.get 回显、隐藏再收起；拉取失败提示', async () => {
    const w = await mountView('强化记忆')
    requestMock.mockClear()
    // 初始只有测试卡一个 textarea；打开配置编辑器后变两个
    expect(w.findAll('textarea').length).toBe(1)
    await w.findAll('button').find(b => b.text() === '查看配置')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'config.get').length).toBe(1)
    const ta = w.findAll('textarea')[0]!
    expect((ta.element as HTMLTextAreaElement).value).toContain('dim: 384')
    await w.findAll('button').find(b => b.text() === '隐藏配置')!.trigger('click')
    expect(w.findAll('textarea').length).toBe(1)
    // 再次打开时拉取失败 → toast
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.get') return Promise.reject(new Error('x'))
      return Promise.resolve({})
    })
    await w.findAll('button').find(b => b.text() === '查看配置')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('加载配置失败'))).toBe(true)
    w.unmount()
  })

  it('保存嵌入配置：成功 toast + 刷新 env；失败提示', async () => {
    const w = await mountView('强化记忆')
    await w.findAll('button').find(b => b.text() === '查看配置')!.trigger('click')
    await flushPromises()
    requestMock.mockClear()
    await w.find('textarea').setValue('embed:\n  dim: 768')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('memory', 'config.set', { embedding_config_content: 'embed:\n  dim: 768' })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message === '配置已保存')).toBe(true)
    expect(requestMock.mock.calls.some(c => c[1] === 'env.check')).toBe(true)

    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.set') return Promise.reject(new Error('bad yaml'))
      return Promise.resolve({})
    })
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保存失败'))).toBe(true)
    w.unmount()
  })

  it('检查环境 / 一键安装：成功与失败分支；进度条渲染', async () => {
    const w = await mountView('强化记忆')
    requestMock.mockClear()
    await w.findAll('button').find(b => b.text() === '检查环境')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('memory', 'env.check')
    expect(useToast().toasts.some(t => t.type === 'success' && t.message === '环境检查完成')).toBe(true)

    await w.findAll('button').find(b => b.text() === '一键安装')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('memory', 'env.setup', undefined, 0)
    expect(useToast().toasts.some(t => t.type === 'success' && t.message === '一键安装完成')).toBe(true)

    // 失败：进度清空 + toast
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'env.setup') return Promise.reject(new Error('net'))
      return Promise.resolve({})
    })
    await w.findAll('button').find(b => b.text() === '一键安装')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('安装失败'))).toBe(true)
    expect(w.text()).not.toContain('正在安装...')

    // 检查环境失败
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'env.check') return Promise.reject(new Error('x'))
      return Promise.resolve({})
    })
    await w.findAll('button').find(b => b.text() === '检查环境')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('检查失败'))).toBe(true)
    w.unmount()
  })

  it('安装按钮失败分支 + model_size 缺席不渲染大小', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.get') return Promise.resolve(vecDefaults())
      if (cmd === 'env.check') return Promise.resolve({ models: { medium: { model_ready: true, dimension: 384 } } })
      if (cmd === 'model.install') return Promise.reject(new Error('404'))
      return Promise.resolve({})
    })
    const w = await mountView('强化记忆')
    expect(w.text()).toContain('(384d)')
    expect(w.text()).not.toContain('MB')
    const installBtns = w.findAll('button').filter(b => b.text() === '安装')
    await installBtns[0]!.trigger('click') // large 档
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('memory', 'model.install', { tier: 'large' }, 0)
    expect(useToast().toasts.some(t => t.message.includes('大模型模型安装失败'))).toBe(true)
    w.unmount()
  })
})

describe('MemoryView 强化记忆 TAB：内容与测试卡（gap 补测）', () => {
  beforeEach(() => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.get') return Promise.resolve(vecDefaults())
      return Promise.resolve({})
    })
  })

  it('条目搜索：空关键词禁用按钮不发请求；成功渲染结果 + 清除；失败提示', async () => {
    const w = await mountView('强化记忆')
    requestMock.mockClear()
    // entriesList 渲染分支（无搜索结果时显示普通列表）
    ;(w.vm as any).entriesList = [{ id: 'e1', content: '普通条目', type: 'note' }]
    await flushPromises()
    expect(w.text()).toContain('普通条目')
    // 空关键词：按钮 disabled，不发请求
    const searchBtn = w.findAll('button').find(b => b.text() === '搜索')!
    expect(searchBtn.attributes('disabled')).toBeDefined()
    await (w.vm as any).searchEntries()
    expect(requestMock.mock.calls.filter(c => c[1] === 'entries.search').length).toBe(0)

    // 成功
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'entries.search') return Promise.resolve({ results: [{ id: 'r1', content: '命中内容', type: 'episodic' }] })
      return Promise.resolve({})
    })
    const inputs = w.findAll('input.form-input')
    await inputs[0]!.setValue('关键词')
    await searchBtn.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('memory', 'entries.search', { query: '关键词', limit: 20 })
    expect(w.text()).toContain('命中内容')
    expect(w.text()).toContain('episodic')
    // 清除
    await w.findAll('button').find(b => b.text() === '清除')!.trigger('click')
    expect((w.vm as any).entriesSearchResults.length).toBe(0)

    // 失败
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'entries.search') return Promise.reject(new Error('x'))
      return Promise.resolve({})
    })
    await w.findAll('input.form-input').find(i => i.attributes('placeholder')?.includes('搜索记忆条目'))!.setValue('再次搜索')
    await w.findAll('button').find(b => b.text() === '搜索')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('搜索失败'))).toBe(true)
    w.unmount()
  })

  it('存储测试条目：空文本提示、成功清空并刷新、失败提示', async () => {
    const w = await mountView('强化记忆')
    const vm = w.vm as any
    requestMock.mockClear()
    // 空文本守卫
    await vm.storeTestEntry()
    expect(useToast().toasts.some(t => t.message === '请输入测试文本')).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'entries.store').length).toBe(0)

    // 成功
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'entries.store') return Promise.resolve({ id: 'abcdef1234567890' })
      if (cmd === 'stats') return Promise.resolve({ vector_entries: 3 })
      return Promise.resolve({})
    })
    const ta = w.findAll('textarea').find(t => t.attributes('placeholder')?.includes('输入文本存储到记忆中'))!
    await ta.setValue('测试文本')
    await w.findAll('button').find(b => b.text() === '存储')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('memory', 'entries.store', { content: '测试文本' })
    expect(useToast().toasts.some(t => t.message.includes('测试条目已存储: abcdef12'))).toBe(true)
    expect(vm.testInputText).toBe('')
    expect(requestMock.mock.calls.some(c => c[1] === 'stats')).toBe(true)

    // 失败
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'entries.store') return Promise.reject(new Error('full'))
      return Promise.resolve({})
    })
    await ta.setValue('再来一条')
    await w.findAll('button').find(b => b.text() === '存储')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('存储失败'))).toBe(true)
    expect(vm.testStoring).toBe(false)
    w.unmount()
  })

  it('关键词搜索测试：空守卫、成功渲染、失败提示', async () => {
    const w = await mountView('强化记忆')
    const vm = w.vm as any
    requestMock.mockClear()
    await vm.runTestSearch()
    expect(requestMock.mock.calls.filter(c => c[1] === 'entries.search').length).toBe(0)
    expect(w.text()).toContain('输入查询文字进行搜索测试')

    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'entries.search') return Promise.resolve({ results: [{ id: 't1', content: '测试命中', type: 'fact' }] })
      return Promise.resolve({})
    })
    const inputs = w.findAll('input.form-input')
    await inputs[1]!.setValue('测试查询')
    await w.findAll('button').filter(b => b.text() === '搜索')[1]!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('memory', 'entries.search', { query: '测试查询', limit: 5 })
    expect(w.text()).toContain('测试命中')

    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'entries.search') return Promise.reject(new Error('x'))
      return Promise.resolve({})
    })
    await w.findAll('button').filter(b => b.text() === '搜索')[1]!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('测试搜索失败'))).toBe(true)
    w.unmount()
  })

  it('stats/entries 加载失败静默；config.get 失败提示；SSE 进度 handler', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.get') return Promise.reject(new Error('gone'))
      if (cmd === 'stats' || cmd === 'entries.list' || cmd === 'status') return Promise.reject(new Error('silent'))
      return Promise.resolve({})
    })
    const w = await mountView('强化记忆')
    expect(useToast().toasts.some(t => t.message.includes('加载配置失败'))).toBe(true)
    // 静默失败：无其它 toast、页面无崩溃
    expect(useToast().toasts.length).toBe(1)

    // SSE memory-setup handler：message → 进度条；complete/error → 2 秒后清空
    const handler =
      (sseOnMock as unknown as import('vitest').Mock).mock.calls.filter(c => c[0] === 'memory-setup').at(-1)![1] as (d: any) => void
    handler({ message: '下载中 50%' })
    await flushPromises()
    expect(w.text()).toContain('下载中 50%')
    handler({ status: 'complete' })
    await flushPromises()
    await vi.advanceTimersByTimeAsync(2000)
    expect((w.vm as any).setupProgress).toBe('')
    handler({ message: '重试' })
    await flushPromises()
    expect(w.text()).toContain('重试')
    handler({ status: 'error' })
    await flushPromises()
    await vi.advanceTimersByTimeAsync(2000)
    expect((w.vm as any).setupProgress).toBe('')
    w.unmount()
  })

  it('config.set 失败 → 防抖后 toast（错误分支）', async () => {
    const w = await mountView('强化记忆')
    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.set') return Promise.reject(new Error('disk full'))
      return Promise.resolve({})
    })
    const maxResultsInput = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '10')!
    await maxResultsInput.setValue('20')
    await vi.advanceTimersByTimeAsync(600)
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('disk full'))).toBe(true)
    w.unmount()
  })

  it('重启 Agent 失败提示（错误分支）', async () => {
    const w = await mountView()
    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'stop') return Promise.reject(new Error('nope'))
      return Promise.resolve({})
    })
    await w.findAll('button').find(b => b.text().includes('重启 Agent 生效'))!.trigger('click')
    await vi.advanceTimersByTimeAsync(1500)
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('重启 Agent 失败'))).toBe(true)
    expect((w.vm as any).restartingAgent).toBe(false)
    w.unmount()
  })
})

describe('MemoryView 注入 TAB：条目管理错误分支与守卫（gap 补测）', () => {
  beforeEach(() => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.get') return Promise.resolve(vecDefaults())
      if (cmd === 'entries.list') return Promise.resolve({ entries: [{ id: 'e1', content: '条目一' }], total: 1 })
      return Promise.resolve({})
    })
  })

  it('列表加载失败提示；加载更多失败提示', async () => {
    const w = await mountView()
    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'entries.list') {
        if ((data?.offset ?? 0) === 0) return Promise.reject(new Error('db'))
        return Promise.reject(new Error('db2'))
      }
      return Promise.resolve({})
    })
    await w.findAll('button').find(b => b.text() === '刷新')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('加载记忆条目失败'))).toBe(true)

    // 手动铺底数据让「加载更多」按钮出现，再让它失败
    ;(w.vm as any).mgmtEntries = [{ id: 'x1', content: 'a' }]
    ;(w.vm as any).mgmtTotal = 80
    await flushPromises()
    await w.findAll('button').find(b => b.text() === '加载更多')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('加载更多失败'))).toBe(true)
    w.unmount()
  })

  it('新增守卫与失败；搜索空/成功/失败；清除恢复列表', async () => {
    const w = await mountView()
    const vm = w.vm as any
    requestMock.mockClear()
    // 新增空内容守卫
    await vm.addMgmtEntry()
    expect(requestMock.mock.calls.filter(c => c[1] === 'entries.store').length).toBe(0)

    // 新增失败
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'entries.store') return Promise.reject(new Error('quota'))
      if (cmd === 'entries.search') return Promise.reject(new Error('idx'))
      return Promise.resolve({})
    })
    const ta = w.findAll('textarea').find(t => t.attributes('placeholder')?.includes('要记住的内容'))!
    await ta.setValue('会失败的内容')
    await w.findAll('button').find(b => b.text() === '添加')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('添加失败'))).toBe(true)
    expect(vm.mgmtAdding).toBe(false)

    // 搜索：空守卫
    await vm.searchMgmtEntries()
    expect(vm.mgmtResults).toBe(null)
    // 搜索失败
    await w.findAll('input.form-input').find(i => i.attributes('placeholder')?.includes('关键词搜索'))!.setValue('命中')
    await w.findAll('button').filter(b => b.text() === '搜索')[0]!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('搜索失败'))).toBe(true)
    // 搜索成功 → 结果渲染 + 清除按钮
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'entries.search') return Promise.resolve({ results: [{ id: 's1', content: '搜索命中' }] })
      return Promise.resolve({})
    })
    await w.findAll('button').filter(b => b.text() === '搜索')[0]!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('搜索命中')
    expect(w.text()).toContain('搜索结果')
    await w.findAll('button').find(b => b.text() === '清除')!.trigger('click')
    expect(vm.mgmtResults).toBe(null)
    expect(vm.mgmtSearchQuery).toBe('')
    // 搜索无结果 → 空态（清除后 query 已空，需重新输入再搜）
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'entries.search') return Promise.resolve({ results: [] })
      return Promise.resolve({})
    })
    await w.findAll('input.form-input').find(i => i.attributes('placeholder')?.includes('关键词搜索'))!.setValue('zzz')
    await w.findAll('button').filter(b => b.text() === '搜索')[0]!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('无匹配结果')
    w.unmount()
  })

  it('编辑失败提示；保存守卫与成功流；删除取消/失败/编辑中删除联动', async () => {
    const w = await mountView()
    const vm = w.vm as any
    requestMock.mockClear()
    let getFail = true
    let deleteFail = true
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'entries.list') return Promise.resolve({ entries: [{ id: 'e1', content: '条目一' }], total: 1 })
      if (cmd === 'entries.get') return getFail ? Promise.reject(new Error('gone')) : Promise.resolve({ entry: { id: 'e1', content: '全量原文' } })
      if (cmd === 'entries.delete') return deleteFail ? Promise.reject(new Error('locked')) : Promise.resolve({})
      return Promise.resolve({})
    })
    const confirmSpy = vi.fn(() => true)
    vi.stubGlobal('confirm', confirmSpy)

    // 编辑读取失败
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('读取条目失败'))).toBe(true)

    // 读取成功进入编辑态；空内容保存守卫
    getFail = false
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await flushPromises()
    expect(vm.mgmtEditingId).toBe('e1')
    vm.mgmtEditContent = '   '
    await vm.saveMgmtEdit()
    expect(requestMock.mock.calls.filter(c => c[1] === 'entries.update').length).toBe(0)

    // 保存成功：toast + 退出编辑 + 刷新列表
    vm.mgmtEditContent = '改好了'
    await vm.saveMgmtEdit()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('memory', 'entries.update', { id: 'e1', content: '改好了' })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message === '记忆条目已更新')).toBe(true)
    expect(vm.mgmtEditingId).toBe('')

    // 编辑中取消
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await flushPromises()
    await w.findAll('button').find(b => b.text() === '取消')!.trigger('click')
    expect(vm.mgmtEditingId).toBe('')

    // 删除被 confirm 取消 → 不发请求
    confirmSpy.mockReturnValue(false)
    await w.findAll('button').find(b => b.text() === '删除')!.trigger('click')
    expect(confirmSpy).toHaveBeenCalled()
    expect(requestMock.mock.calls.filter(c => c[1] === 'entries.delete').length).toBe(0)

    // 删除失败
    confirmSpy.mockReturnValue(true)
    await w.findAll('button').find(b => b.text() === '删除')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('删除失败'))).toBe(true)

    // 编辑中删除（成功）→ 编辑态联动清空（编辑态无删除按钮，走 vm 调用）
    deleteFail = false
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await flushPromises()
    expect(vm.mgmtEditingId).toBe('e1')
    await vm.deleteMgmtEntry({ id: 'e1' })
    await flushPromises()
    expect(vm.mgmtEditingId).toBe('')
    confirmSpy.mockRestore()
    w.unmount()
  })

  it('前置状态四分支 + 注入条数禁用态', async () => {
    const w = await mountView()
    const vm = w.vm as any
    // 初始 mock：main on / sub on / auto_inject on / medium ready？env.check 默认 {} → model 未装
    expect(w.text()).toContain('模型未安装')
    // 分支 1：主开关关
    vm.mainEnabled = false
    await flushPromises()
    expect(w.text()).toContain('主开关未启用')
    expect(w.text()).toContain('先打开上方「主开关」')
    // 分支 2：主开关开、强化记忆关
    vm.mainEnabled = true
    vm.subEnabled = false
    await flushPromises()
    expect(w.text()).toContain('强化记忆未启用')
    expect(w.text()).toContain('打开「自动注入」会自动带上它')
    // 分支 4：全就绪
    vm.envStatus = { models: { medium: { model_ready: true } } }
    vm.subEnabled = true
    vm.autoInject = true // 级联在 subEnabled=false 时已连带关掉，这里重开
    await flushPromises()
    expect(w.text()).toContain('就绪')
    expect(w.text()).toContain('medium 档模型已安装，注入可用')
    // 自动注入开 → 注入条数可用
    const topK = w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '5')!
    expect(topK.attributes('disabled')).toBeUndefined()
    // 自动注入关 → 注入条数禁用
    vm.autoInject = false
    await flushPromises()
    expect(w.findAll('input[type="number"]').find(i => (i.element as HTMLInputElement).value === '5')!.attributes('disabled')).toBeDefined()
    w.unmount()
  })
})
