import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// ProxySettingsView 单元测试（coverage 补齐 2026-11-11）：
// per-model 代理表（行内编辑/协议校验/默认模型热切）、调用方 key 管理
// （add/toggle/remove/copy）、config.proxy.json env 段编辑与校验豁免、
// lane 支持表与空态。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import ProxySettingsView from '../ProxySettingsView.vue'

function defaultImpl(_m: string, cmd: string, _p?: any) {
  switch (cmd) {
    case 'proxy_overview':
      return Promise.resolve({
        models: [
          { model_name: 'gpt-main', model: 'openai/gpt-4o', protocol: 'openai', proxy: 'http://p1:8080', is_default: true },
          { model_name: 'claude-alt', model: 'anthropic/claude-3', protocol: '', proxy: '', is_default: false },
        ],
        lane_support: [
          { lane: 'openai-chat', per_model_proxy: true, note: '' },
          { lane: 'cli-lane', per_model_proxy: false, note: 'CLI lane 走环境变量' },
        ],
        notes: ['提示一', '提示二'],
      })
    case 'get':
      return Promise.resolve({
        path: '/ws/config/config.proxy.json',
        file_exists: true,
        parse_error: null,
        env_file: { http_proxy: 'http://file:1', https_proxy: '', all_proxy: 'socks5://file:2', no_proxy: 'localhost' },
        env_process: { http_proxy: 'http://proc:1', https_proxy: '', all_proxy: '', no_proxy: '' },
      })
    case 'keys.list':
      return Promise.resolve({
        keys: [
          { name: 'bot-a', key: 'k-aaa', enabled: true, created_at: 1735689600, note: '机器人A' },
          { name: 'bot-b', key: 'k-bbb', enabled: false, created_at: 0, note: '' },
        ],
        path: '/ws/config/config.proxy_keys.json',
        file_exists: false,
      })
    default:
      return Promise.resolve({})
  }
}

function stubClipboard(impl: () => Promise<void>) {
  const writeText = vi.fn(impl)
  Object.defineProperty(window.navigator, 'clipboard', { value: { writeText }, configurable: true })
  return writeText
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation(defaultImpl)
  useToast().toasts.splice(0)
})

afterEach(() => {
  vi.unstubAllGlobals()
  delete (window.navigator as any).clipboard
})

async function mountView() {
  const w = mount(ProxySettingsView)
  await flushPromises()
  return w
}

function proxyEditInput(w: ReturnType<typeof mount>) {
  return w.findAll('input').find(i => i.attributes('placeholder')?.includes('留空 = 直连'))!
}

describe('ProxySettingsView 挂载与渲染', () => {
  it('挂载拉取三组数据并渲染各区块', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('models', 'proxy_overview', {}, 8000)
    expect(requestMock).toHaveBeenCalledWith('proxy', 'get', {}, 8000)
    expect(requestMock).toHaveBeenCalledWith('proxy', 'keys.list', {}, 8000)
    expect(w.text()).toContain('代理设置')
    expect(w.text()).toContain('gpt-main')
    expect(w.text()).toContain('默认')
    expect(w.text()).toContain('自动推断')
    expect(w.text()).toContain('http://p1:8080')
    expect(w.text()).toContain('未设置')
    expect(w.text()).toContain('修改')
    expect(w.text()).toContain('设置代理')
    expect(w.text()).toContain('支持')
    expect(w.text()).toContain('走环境变量')
    expect(w.text()).toContain('CLI lane 走环境变量')
    expect(w.text()).toContain('提示一')
    expect(w.text()).toContain('bot-a')
    expect(w.text()).toContain('机器人A')
    expect(w.text()).toContain('2025') // fmtKeyTime(1735689600)
    expect(w.text()).toContain('—') // created_at 0 → '—'；空 note → '—'
    expect(w.text()).toContain('HTTP_PROXY')
    expect(w.text()).toContain('http://proc:1')
    expect(w.text()).toContain('（未设置）')
    expect(w.text()).toContain('/ws/config/config.proxy.json')
    expect(w.text()).toContain('——尚未创建，首次添加自动生成')
    const vm = w.vm as any
    expect(vm.fmtKeyTime(0)).toBe('—')
    expect(vm.fmtKeyTime(1735689600)).toContain('2025')
    w.unmount()
  })

  it('三类加载失败的 toast（Error 对象与字符串 rejection）', async () => {
    requestMock.mockImplementation((_m, cmd) => {
      if (cmd === 'proxy_overview') return Promise.reject(new Error('boom1'))
      if (cmd === 'get') return Promise.reject(new Error('boom2'))
      if (cmd === 'keys.list') return Promise.reject('rawstr')
      return Promise.resolve({})
    })
    const w = await mountView()
    const msgs = useToast().toasts.map(t => t.message)
    expect(msgs.some(m => m.includes('加载代理信息失败') && m.includes('boom1'))).toBe(true)
    expect(msgs.some(m => m.includes('加载代理配置文件失败') && m.includes('boom2'))).toBe(true)
    expect(msgs.some(m => m.includes('加载调用方 key 失败') && m.includes('rawstr'))).toBe(true)
    // 字符串 rejection → e?.message || e 右路径
    requestMock.mockImplementation((_m, cmd) => cmd === 'get' ? Promise.reject('raw-boom2') : defaultImpl(_m, cmd))
    await (w.vm as any).loadProxyFile()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('加载代理配置文件失败') && t.message.includes('raw-boom2'))).toBe(true)
    w.unmount()
  })

  it('空数据空态 + 两个刷新按钮各自重拉；响应为 null 走 ?? 兜底；文件已创建则无「尚未创建」提示', async () => {
    // 第一阶段：三个接口全部返回 null → resp?.x 的空值路径 + 空数组兜底
    requestMock.mockImplementation((_m, cmd) => {
      if (cmd === 'proxy_overview') return Promise.resolve(null)
      if (cmd === 'get') return Promise.resolve(null)
      if (cmd === 'keys.list') return Promise.resolve(null)
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(w.text()).toContain('尚未配置任何模型')
    expect(w.text()).toContain('尚未签发任何调用方 key')
    expect(w.text()).toContain('config.proxy.json（工作区 config 目录）')
    expect(w.text()).toContain('config.proxy_keys.json（工作区 config 目录）')
    const refreshBtns = w.findAll('button').filter(b => b.text() === '刷新')
    expect(refreshBtns.length).toBe(2)
    // 刷新时让 proxy_overview 回 undefined（与 null 区分的空值路径）
    requestMock.mockImplementation((_m, cmd) => {
      if (cmd === 'proxy_overview') return Promise.resolve(undefined)
      if (cmd === 'get') return Promise.resolve(null)
      if (cmd === 'keys.list') return Promise.resolve(null)
      return Promise.resolve({})
    })
    const overviewCalls = requestMock.mock.calls.filter(c => c[1] === 'proxy_overview').length
    await refreshBtns[0]!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'proxy_overview').length).toBe(overviewCalls + 1)
    const keysCalls = requestMock.mock.calls.filter(c => c[1] === 'keys.list').length
    await refreshBtns[1]!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'keys.list').length).toBe(keysCalls + 1)

    // 第二阶段：resp 非空但 models 缺失（?? 右路径）+ 文件均已创建（无「尚未创建」后缀）
    requestMock.mockImplementation((_m, cmd) => {
      if (cmd === 'proxy_overview') return Promise.resolve({})
      if (cmd === 'get') return Promise.resolve({ path: '/ws/p.json', file_exists: true, parse_error: null, env_file: {}, env_process: {} })
      if (cmd === 'keys.list') return Promise.resolve({ keys: [], path: '/ws/k.json', file_exists: true })
      return defaultImpl(_m, cmd)
    })
    await (w.vm as any).load()
    await (w.vm as any).loadProxyFile()
    await (w.vm as any).loadKeys()
    await flushPromises()
    expect(w.text()).toContain('/ws/p.json')
    expect(w.text()).toContain('/ws/k.json')
    expect(w.text()).not.toContain('——尚未创建，首次保存自动生成')
    expect(w.text()).not.toContain('——尚未创建，首次添加自动生成')
    w.unmount()
  })

  it('加载中：刷新按钮显示刷新中…，空态隐藏', async () => {
    requestMock.mockImplementation((_m, cmd) => (cmd === 'proxy_overview' || cmd === 'keys.list') ? new Promise(() => {}) : defaultImpl(_m, cmd))
    const w = await mountView()
    expect(w.findAll('button').filter(b => b.text() === '刷新中…').length).toBe(2)
    expect(w.text()).not.toContain('尚未配置任何模型')
    expect(w.text()).not.toContain('尚未签发任何调用方 key')
    w.unmount()
  })

  it('解析失败徽标 + 路径空默认文案 + 未创建提示 + env 全空兜底', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'get'
      ? Promise.resolve({ path: '', file_exists: false, parse_error: 'JSON 语法错误', env_file: {}, env_process: {} })
      : defaultImpl(_m, cmd))
    const w = await mountView()
    expect(w.text()).toContain('配置文件解析失败')
    expect(w.text()).toContain('JSON 语法错误')
    expect(w.text()).toContain('（保存将以空配置重建）')
    expect(w.text()).toContain('config.proxy.json（工作区 config 目录）')
    expect(w.text()).toContain('——尚未创建，首次保存自动生成')
    expect(w.findAll('span.muted').some(m => m.text() === '（未设置）')).toBe(true)
    w.unmount()
  })
})

describe('ProxySettingsView 模型代理编辑', () => {
  it('修改默认模型代理：update_field + set_default 热切 + toast', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '修改')!.trigger('click')
    const input = proxyEditInput(w)
    expect((input.element as HTMLInputElement).value).toBe('http://p1:8080')
    await input.setValue('socks5://n1:1080')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('models', 'update_field', { name: 'gpt-main', field: 'proxy', value: 'socks5://n1:1080' }, 8000)
    expect(requestMock).toHaveBeenCalledWith('models', 'set_default', { name: 'gpt-main' }, 8000)
    expect(useToast().toasts.some(t => t.message.includes('「gpt-main」代理已保存') && t.message.includes('已热切生效'))).toBe(true)
    w.unmount()
  })

  it('非默认模型：keyup.enter 保存、不发 set_default、toast 无热切字样', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '设置代理')!.trigger('click')
    const input = proxyEditInput(w)
    expect((input.element as HTMLInputElement).value).toBe('')
    await input.setValue('http://n2:1')
    await input.trigger('keyup', { key: 'Enter' })
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('models', 'update_field', { name: 'claude-alt', field: 'proxy', value: 'http://n2:1' }, 8000)
    expect(requestMock.mock.calls.filter(c => c[1] === 'set_default').length).toBe(0)
    const saved = useToast().toasts.find(t => t.message.includes('「claude-alt」代理已保存'))!
    expect(saved.message).not.toContain('已热切生效')
    w.unmount()
  })

  it('清除按钮进入空值编辑 → 保存即清除代理', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '清除')!.trigger('click')
    expect((proxyEditInput(w).element as HTMLInputElement).value).toBe('')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('models', 'update_field', { name: 'gpt-main', field: 'proxy', value: '' }, 8000)
    w.unmount()
  })

  it('非法协议拦截：不发请求', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '修改')!.trigger('click')
    await proxyEditInput(w).setValue('ftp://x:21')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('代理必须以'))).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'update_field').length).toBe(0)
    w.unmount()
  })

  it('保存失败：toast 且留在编辑态', async () => {
    const w = await mountView()
    const vm = w.vm as any
    requestMock.mockImplementation((_m, cmd) => cmd === 'update_field' ? Promise.reject(new Error('denied')) : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '修改')!.trigger('click')
    await proxyEditInput(w).setValue('http://z:9')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保存失败') && t.message.includes('denied'))).toBe(true)
    // 字符串 rejection → e?.message || e 右路径
    requestMock.mockImplementation((_m, cmd) => cmd === 'update_field' ? Promise.reject('raw-deny') : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保存失败') && t.message.includes('raw-deny'))).toBe(true)
    expect(vm.editing).toBe('gpt-main')
    expect(vm.saving).toBe(null)
    w.unmount()
  })

  it('saving 守卫早退；Esc 取消编辑', async () => {
    const w = await mountView()
    const vm = w.vm as any
    vm.saving = 'busy'
    await vm.saveEdit(vm.rows[0])
    expect(requestMock.mock.calls.filter(c => c[1] === 'update_field').length).toBe(0)
    vm.saving = null

    await w.findAll('button').find(b => b.text() === '修改')!.trigger('click')
    expect(vm.editing).toBe('gpt-main')
    await proxyEditInput(w).trigger('keyup', { key: 'Escape' })
    expect(vm.editing).toBe(null)
    expect(vm.editValue).toBe('')
    w.unmount()
  })

  it('保存中按钮显示保存中… 且禁用', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'update_field' ? new Promise(() => {}) : defaultImpl(_m, cmd))
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '修改')!.trigger('click')
    await proxyEditInput(w).setValue('http://x:1')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    expect(w.findAll('button').find(b => b.text() === '保存中…')).toBeTruthy()
    w.unmount()
  })
})

describe('ProxySettingsView 调用方 key 管理', () => {
  it('添加 key：守卫、空名拦截、重复 key 拦截、成功（含 note 回显/自动生成）、失败', async () => {
    const w = await mountView()
    const vm = w.vm as any
    const nameInput = w.findAll('input').find(i => i.attributes('placeholder')?.includes('调用方名称'))!
    const keyInput = w.findAll('input').find(i => i.attributes('placeholder')?.includes('留空自动生成'))!
    const noteInput = w.findAll('input').find(i => i.attributes('placeholder')?.includes('备注'))!

    // 守卫早退
    vm.keyAdding = true
    await vm.addKey()
    vm.keyAdding = false
    await nextTick() // 等「添加中… → 添加」的按钮文案回滚渲染完成
    expect(requestMock.mock.calls.filter(c => c[1] === 'keys.add').length).toBe(0)

    // 空名拦截（名称为空时按钮 disabled，走输入框回车路径）
    await nameInput.trigger('keyup', { key: 'Enter' })
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '调用方名称不能为空')).toBe(true)

    // 重复 key 拦截
    await nameInput.setValue('bot-c')
    await keyInput.setValue('k-aaa')
    await w.findAll('button').find(b => b.text() === '添加')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('该 key 已被其他调用方使用'))).toBe(true)

    // 成功：带 key 与备注，服务端 note 优先
    requestMock.mockImplementation((_m, cmd) => cmd === 'keys.add' ? Promise.resolve({ note: '已签发 bot-c' }) : defaultImpl(_m, cmd))
    await keyInput.setValue('k-ccc')
    await noteInput.setValue('测试用')
    await w.findAll('button').find(b => b.text() === '添加')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('proxy', 'keys.add', { name: 'bot-c', key: 'k-ccc', note: '测试用' }, 8000)
    expect(useToast().toasts.some(t => t.message === '已签发 bot-c')).toBe(true)
    expect((nameInput.element as HTMLInputElement).value).toBe('')

    // 成功：留空 key 自动生成（keyup.enter 触发）——恢复默认 mock（keys.add 返回 {} → 默认文案）
    requestMock.mockImplementation(defaultImpl)
    await nameInput.setValue('bot-d')
    await nameInput.trigger('keyup', { key: 'Enter' })
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('proxy', 'keys.add', { name: 'bot-d', key: undefined, note: '' }, 8000)
    expect(useToast().toasts.some(t => t.message === '已保存，即刻生效')).toBe(true)

    // 失败
    requestMock.mockImplementation((_m, cmd) => cmd === 'keys.add' ? Promise.reject(new Error('dup name')) : defaultImpl(_m, cmd))
    await nameInput.setValue('bot-e')
    await w.findAll('button').find(b => b.text() === '添加')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保存失败') && t.message.includes('dup name'))).toBe(true)
    // 字符串 rejection → e?.message || e 右路径
    requestMock.mockImplementation((_m, cmd) => cmd === 'keys.add' ? Promise.reject('raw-dup') : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '添加')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保存失败') && t.message.includes('raw-dup'))).toBe(true)
    expect(vm.keyAdding).toBe(false)
    w.unmount()
  })

  it('toggleKey：禁用/启用两个方向 + 服务端 note 优先 + 失败提示 + 守卫', async () => {
    const w = await mountView()
    const vm = w.vm as any
    vm.keyToggling = 'busy'
    await vm.toggleKey(vm.keyRows[0])
    expect(requestMock.mock.calls.filter(c => c[1] === 'keys.set_enabled').length).toBe(0)
    vm.keyToggling = null

    await w.findAll('button').find(b => b.text() === '禁用')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('proxy', 'keys.set_enabled', { name: 'bot-a', enabled: false }, 8000)
    expect(useToast().toasts.some(t => t.message === '已禁用，即刻生效')).toBe(true)

    await w.findAll('button').find(b => b.text() === '启用')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('proxy', 'keys.set_enabled', { name: 'bot-b', enabled: true }, 8000)
    expect(useToast().toasts.some(t => t.message === '已启用，即刻生效')).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'keys.set_enabled' ? Promise.resolve({ note: '已切换' }) : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '禁用')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '已切换')).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'keys.set_enabled' ? Promise.reject(new Error('lock')) : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '禁用')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('操作失败') && t.message.includes('lock'))).toBe(true)
    // 字符串 rejection → e?.message || e 右路径
    requestMock.mockImplementation((_m, cmd) => cmd === 'keys.set_enabled' ? Promise.reject('raw-lock') : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '禁用')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('操作失败') && t.message.includes('raw-lock'))).toBe(true)
    expect(vm.keyToggling).toBe(null)
    w.unmount()
  })

  it('removeKey：confirm 拦截不发请求；成功/服务端 note/失败', async () => {
    const confirmSpy = vi.fn(() => false)
    vi.stubGlobal('confirm', confirmSpy)
    const w = await mountView()
    const vm = w.vm as any
    await w.findAll('button').find(b => b.text() === '删除')!.trigger('click')
    expect(confirmSpy).toHaveBeenCalled()
    expect(requestMock.mock.calls.filter(c => c[1] === 'keys.remove').length).toBe(0)

    vm.keyRemoving = 'busy'
    vi.stubGlobal('confirm', vi.fn(() => true)) // 先放行 confirm，才能真正到达 keyRemoving 守卫
    await vm.removeKey(vm.keyRows[0])
    expect(requestMock.mock.calls.filter(c => c[1] === 'keys.remove').length).toBe(0)
    vm.keyRemoving = null
    vi.stubGlobal('confirm', confirmSpy) // 恢复 false，供下方 UI 拦截断言

    vi.stubGlobal('confirm', vi.fn(() => true))
    await w.findAll('button').find(b => b.text() === '删除')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('proxy', 'keys.remove', { name: 'bot-a' }, 8000)
    expect(useToast().toasts.some(t => t.message === '已删除，即刻生效')).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'keys.remove' ? Promise.resolve({ note: '已移除 bot-a' }) : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '删除')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '已移除 bot-a')).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'keys.remove' ? Promise.reject(new Error('lock')) : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '删除')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('删除失败') && t.message.includes('lock'))).toBe(true)
    // 字符串 rejection → e?.message || e 右路径
    requestMock.mockImplementation((_m, cmd) => cmd === 'keys.remove' ? Promise.reject('raw-lock') : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '删除')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('删除失败') && t.message.includes('raw-lock'))).toBe(true)
    expect(vm.keyRemoving).toBe(null)
    w.unmount()
  })

  it('copyKey：无 clipboard 可选链短路；写剪贴板成功/失败 toast', async () => {
    const w = await mountView()
    // 未注入 clipboard → navigator.clipboard?. 空值路径，无 toast 不崩溃
    await w.findAll('button').find(b => b.text() === '复制')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('Key 已复制'))).toBe(false)
    expect(useToast().toasts.some(t => t.message.includes('复制失败'))).toBe(false)

    const writeText = stubClipboard(() => Promise.resolve())
    await w.findAll('button').find(b => b.text() === '复制')!.trigger('click')
    await flushPromises()
    expect(writeText).toHaveBeenCalledWith('k-aaa')
    expect(useToast().toasts.some(t => t.message === 'Key 已复制到剪贴板')).toBe(true)

    stubClipboard(() => Promise.reject(new Error('denied')))
    await w.findAll('button').find(b => b.text() === '复制')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('复制失败，请手动选择复制'))).toBe(true)
    w.unmount()
  })
})

describe('ProxySettingsView 环境变量代理', () => {
  it('saveEnv：守卫、非法值拦截（no_proxy 豁免）、成功回读、失败提示', async () => {
    const w = await mountView()
    const vm = w.vm as any
    vm.envSaving = true
    await vm.saveEnv()
    vm.envSaving = false
    expect(requestMock.mock.calls.filter(c => c[1] === 'save').length).toBe(0)

    const envInputs = w.findAll('input').filter(i => i.attributes('placeholder') === '留空 = 不设置该项')
    expect(envInputs.length).toBe(4)

    // http_proxy 非法
    await envInputs[0]!.setValue('ftp://bad')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('HTTP_PROXY 必须以'))).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'save').length).toBe(0)

    // https_proxy 非法
    await envInputs[0]!.setValue('')
    await envInputs[1]!.setValue('badvalue')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('HTTPS_PROXY 必须以'))).toBe(true)

    // no_proxy 非法格式豁免 → 校验通过并保存
    await envInputs[1]!.setValue('')
    await envInputs[3]!.setValue('localhost, .internal')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('proxy', 'save', { env: { http_proxy: '', https_proxy: '', all_proxy: 'socks5://file:2', no_proxy: 'localhost, .internal' } }, 8000)
    expect(useToast().toasts.some(t => t.message.includes('重启网关后生效'))).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'get').length).toBe(2) // 保存后回读

    // 失败
    requestMock.mockImplementation((_m, cmd) => cmd === 'save' ? Promise.reject(new Error('disk')) : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保存失败') && t.message.includes('disk'))).toBe(true)
    // 字符串 rejection → e?.message || e 右路径
    requestMock.mockImplementation((_m, cmd) => cmd === 'save' ? Promise.reject('raw-disk') : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保存失败') && t.message.includes('raw-disk'))).toBe(true)
    expect(vm.envSaving).toBe(false)
    w.unmount()
  })
})
