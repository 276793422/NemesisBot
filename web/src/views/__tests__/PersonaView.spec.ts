import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'
import { useToast } from '../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))
vi.mock('../../composables/useSSE', () => ({
  on: vi.fn(),
  off: vi.fn(),
}))

import PersonaView from '../PersonaView.vue'
import { useChatStore } from '../../stores/chat'

const CURRENT = {
  emoji: '🤖',
  name: 'Nemesis',
  active_dir: 'nemesis',
  files: ['IDENTITY.md', 'SOUL.md'],
}

function persona(dir: string, opts: Partial<any> = {}) {
  return {
    dir,
    name: dir,
    emoji: '👤',
    description: `${dir} 的人格`,
    is_default: false,
    is_active: false,
    files: ['IDENTITY.md'],
    ...opts,
  }
}

function defaultImpl(_m: string, cmd: string, p?: any) {
  switch (cmd) {
    case 'current': return Promise.resolve(CURRENT)
    case 'file.get': return Promise.resolve({ content: `# ${p?.file || 'x'} 内容` })
    case 'list': return Promise.resolve({ personas: [persona('default', { is_default: true }), persona('custom1'), persona('custom2', { is_active: true })] })
    default: return Promise.resolve({})
  }
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockImplementation(defaultImpl)
  useToast().toasts.splice(0)
})

async function mountView() {
  const w = mount(PersonaView)
  await flushPromises()
  return w
}

describe('PersonaView 当前人格', () => {
  it('加载当前人格与文件内容，渲染状态栏与文件卡', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('persona', 'current')
    expect(w.text()).toContain('🤖')
    expect(w.text()).toContain('当前：Nemesis')
    expect(w.text()).toContain('IDENTITY.md')
    expect(w.text()).toContain('# IDENTITY.md 内容')
    // file.get 每文件一次
    expect(requestMock.mock.calls.filter(c => c[1] === 'file.get').length).toBeGreaterThanOrEqual(2)
  })

  it('current 加载失败提示；文件读取失败回退空串', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'current' ? Promise.reject(new Error('offline')) : defaultImpl(_m, cmd))
    await mountView()
    expect(useToast().toasts.some(t => t.message.includes('加载当前人格失败'))).toBe(true)

    requestMock.mockImplementation((_m, cmd, p) => {
      if (cmd === 'file.get' && p?.file === 'SOUL.md') return Promise.reject(new Error('gone'))
      return defaultImpl(_m, cmd, p)
    })
    const w = await mountView()
    expect((w.vm as any).fileContents['SOUL.md']).toBe('')
    expect((w.vm as any).fileContents['IDENTITY.md']).toContain('IDENTITY.md')
  })

  it('编辑保存成功；失败提示；守卫分支不发送', async () => {
    const w = await mountView()
    const vm = w.vm as any
    // 守卫：未进入编辑态不发送
    await vm.saveFile()
    expect(requestMock.mock.calls.filter(c => c[1] === 'file.save').length).toBe(0)

    vm.startEdit('IDENTITY.md')
    expect(vm.editingFile).toBe('IDENTITY.md')
    expect(vm.editContent).toContain('IDENTITY.md')
    vm.editContent = '# 新内容'
    await vm.saveFile()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'file.save', {
      name: 'nemesis', file: 'IDENTITY.md', content: '# 新内容',
    })
    expect(useToast().toasts.some(t => t.message === '保存成功')).toBe(true)
    expect(vm.editingFile).toBe('')

    // 失败路径
    vm.startEdit('SOUL.md')
    requestMock.mockImplementation((_m, cmd) => cmd === 'file.save' ? Promise.reject(new Error('ro')) : defaultImpl(_m, cmd))
    await vm.saveFile()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保存失败'))).toBe(true)
  })

  it('取消编辑清空状态', async () => {
    const w = await mountView()
    const vm = w.vm as any
    vm.startEdit('IDENTITY.md')
    vm.cancelEdit()
    expect(vm.editingFile).toBe('')
    expect(vm.editContent).toBe('')
  })
})

describe('PersonaView 本地人格', () => {
  async function gotoLocal(w: ReturnType<typeof mount>) {
    await w.findAll('button.tab').find(b => b.text() === '本地')!.trigger('click')
    await flushPromises()
    return w
  }

  it('列表渲染：使用中徽章、默认人格还原按钮', async () => {
    const w = await mountView()
    await gotoLocal(w)
    expect(w.text()).toContain('custom1')
    expect(w.text()).toContain('使用中')
    expect(w.text()).toContain('还原')
    expect(w.text()).toContain('启用')
  })

  it('list 加载失败提示', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'list' ? Promise.reject(new Error('no')) : defaultImpl(_m, cmd))
    await mountView()
    expect(useToast().toasts.some(t => t.message.includes('加载人格列表失败'))).toBe(true)
  })

  it('点击非激活人格打开预览：逐文件加载、失败提示', async () => {
    const w = await mountView()
    await gotoLocal(w)
    await w.findAll('.persona-card').find(c => c.text().includes('custom1'))!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'file.get', { name: 'custom1', file: 'IDENTITY.md' })
    expect(w.text()).toContain('👤 custom1')

    // 预览失败
    requestMock.mockImplementation((_m, cmd) => cmd === 'file.get' ? Promise.reject(new Error('nf')) : defaultImpl(_m, cmd))
    const w2 = await mountView()
    await gotoLocal(w2)
    await w2.findAll('.persona-card').find(c => c.text().includes('custom1'))!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('加载失败'))).toBe(true)
  })

  it('点击激活中的人格切回当前页签', async () => {
    const w = await mountView()
    await gotoLocal(w)
    await w.findAll('.persona-card').find(c => c.text().includes('custom2'))!.trigger('click')
    expect((w.vm as any).activeTab).toBe('current')
    expect((w.vm as any).previewingPersona).toBe(null)
  })

  it('预览弹窗：切换文件页签、关闭、启用按钮', async () => {
    requestMock.mockImplementation((_m, cmd, p) => {
      if (cmd === 'file.get') return Promise.resolve({ content: `内容:${p?.file}` })
      if (cmd === 'list') return Promise.resolve({ personas: [persona('multi', { files: ['IDENTITY.md', 'SOUL.md'] })] })
      return defaultImpl(_m, cmd, p)
    })
    const w = await mountView()
    await gotoLocal(w)
    await w.findAll('.persona-card').find(c => c.text().includes('multi'))!.trigger('click')
    await flushPromises()
    expect((w.vm as any).previewSelectedFile).toBe('IDENTITY.md')
    // 文件页签切换
    await w.findAll('.preview-tab').find(b => b.text() === 'SOUL.md')!.trigger('click')
    expect((w.vm as any).previewSelectedFile).toBe('SOUL.md')
    // 启用：activate + 关闭预览
    const chat = useChatStore()
    chat.messages.push({} as any)
    await w.findAll('.modal-footer .btn-primary').find(b => b.text() === '启用')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'activate', { name: 'multi' })
    expect((w.vm as any).previewingPersona).toBe(null)
    expect(chat.messages.length).toBe(0)
    expect(chat.historyLoaded).toBe(false)
  })

  it('启用人格成功重置会话历史；失败提示', async () => {
    const w = await mountView()
    await gotoLocal(w)
    const chat = useChatStore()
    chat.messages.push({} as any)
    chat.historyLoaded = true
    chat.hasMoreHistory = false
    await w.findAll('.persona-card').find(c => c.text().includes('custom1'))!.find('.btn-primary').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'activate', { name: 'custom1' })
    expect(useToast().toasts.some(t => t.message === '人格已切换')).toBe(true)
    expect(chat.messages.length).toBe(0)
    expect(chat.historyLoaded).toBe(false)
    expect(chat.hasMoreHistory).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'activate' ? Promise.reject(new Error('locked')) : defaultImpl(_m, cmd))
    await w.findAll('.persona-card').find(c => c.text().includes('custom1'))!.find('.btn-primary').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('切换失败'))).toBe(true)
  })

  it('还原默认人格成功/失败', async () => {
    const w = await mountView()
    await gotoLocal(w)
    const restoreBtn = w.findAll('.persona-card').find(c => c.text().includes('default'))!.findAll('button').find(b => b.text() === '还原')!
    await restoreBtn.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'restore')
    expect(useToast().toasts.some(t => t.message === '已恢复默认人格')).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'restore' ? Promise.reject(new Error('x')) : defaultImpl(_m, cmd))
    await w.findAll('.persona-card').find(c => c.text().includes('default'))!.findAll('button').find(b => b.text() === '还原')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('恢复失败'))).toBe(true)
  })

  it('删除人格成功/失败', async () => {
    const w = await mountView()
    await gotoLocal(w)
    await w.findAll('.persona-card').find(c => c.text().includes('custom1'))!.find('.btn-danger').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('persona', 'remove', { name: 'custom1' })
    expect(useToast().toasts.some(t => t.message === '已删除')).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'remove' ? Promise.reject(new Error('busy')) : defaultImpl(_m, cmd))
    await w.findAll('.persona-card').find(c => c.text().includes('custom1'))!.find('.btn-danger').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('删除失败'))).toBe(true)
  })
})
