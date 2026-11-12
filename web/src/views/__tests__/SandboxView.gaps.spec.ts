import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))
vi.mock('../../composables/useSSE', () => ({ on: vi.fn(), off: vi.fn() }))

import SandboxView from '../SandboxView.vue'

const confirmMock = vi.fn(() => true)

function windowsOverview(over: Record<string, unknown> = {}) {
  return {
    platform: 'windows',
    executor: {
      enabled: true, sandbox: true, strict: false, allow_network: false,
      acl: { dacl: false, strict: false, max_files: 20000 },
    },
    backend_probe: {
      kind: 'sandboxie', start_exe_present: true, sbiesvc_running: true,
      sbiesvc_state: 'Running', sbiedrv_state: 'Loaded', driver_installed: true,
      engine_owned: true, seven_zip: { available: true, source: 'runtime' },
      box_root: 'C:\\box\\NemesisBox',
    },
    ready: true,
    pending: [{ real_path: 'C:\\box\\out.txt', box_path: 'c:\\out.txt', size: 1200 }],
    denials: [],
    ...over,
  }
}

function linuxOverview(over: Record<string, unknown> = {}) {
  return {
    platform: 'linux',
    executor: { enabled: true, sandbox: true, strict: false, allow_network: false },
    backend_probe: { backends: [{ name: 'landlock', form: 'SelfApply', availability: 'full', detail: [] }], selected: 'landlock' },
    ready: true,
    ...over,
  }
}

function defaultImpl(_m: string, cmd: string, _p?: any) {
  if (cmd === 'overview') return Promise.resolve(windowsOverview())
  if (cmd === 'pending') return Promise.resolve([{ real_path: 'C:\\box\\out.txt', box_path: 'c:\\out.txt', size: 1200 }])
  if (cmd === 'denials.list') return Promise.resolve({ denials: [] })
  return Promise.resolve({ committed: 1, total: 1, deleted: 1 })
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation(defaultImpl)
  useToast().toasts.splice(0)
  vi.stubGlobal('confirm', confirmMock)
  confirmMock.mockClear()
  confirmMock.mockImplementation(() => true)
})

async function mountView() {
  const w = mount(SandboxView)
  await flushPromises()
  return w
}

describe('SandboxView 执行体开关（config tab）', () => {
  it('启用沙盒执行：confirm 后 set_config 双键联动', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'overview' ? Promise.resolve(windowsOverview({ executor: { enabled: false, sandbox: false, strict: false, allow_network: false } })) : defaultImpl(_m, cmd, {}))
    const w = await mountView()
    ;(w.vm as any).enableSandboxExec()
    await flushPromises()
    expect(confirmMock).toHaveBeenCalled()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'set_config', { enabled: true, sandbox: true })

    confirmMock.mockImplementation(() => false)
    ;(w.vm as any).disableSandboxExec()
    expect(requestMock.mock.calls.filter(c => (c[2] as any)?.enabled === false).length).toBe(0)
  })

  it('停用沙盒执行走 confirm；用户态联网开关', async () => {
    const w = await mountView()
    ;(w.vm as any).disableSandboxExec()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'set_config', { enabled: false, sandbox: false })

    requestMock.mockImplementation((_m, cmd) => cmd === 'overview' ? Promise.resolve(linuxOverview()) : defaultImpl(_m, cmd, {}))
    const w2 = await mountView()
    const before = (w2.vm as any).allowNetworkCfg
    ;(w2.vm as any).toggleUserlandNetwork()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'set_config', { allow_network: !before })
  })

  it('严格模式：后端不可用时需确认；可用时直接切', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'overview' ? Promise.resolve(windowsOverview({ ready: false })) : defaultImpl(_m, cmd, {}))
    const w = await mountView()
    confirmMock.mockImplementation(() => false)
    ;(w.vm as any).toggleStrict()
    expect(requestMock.mock.calls.filter(c => (c[2] as any)?.strict === true).length).toBe(0)

    confirmMock.mockImplementation(() => true)
    ;(w.vm as any).toggleStrict()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'set_config', { strict: true })
  })
})

describe('SandboxView ACL 三键', () => {
  it('DACL 开启需确认并下发 acl_dacl；取消不发', async () => {
    const w = await mountView()
    ;(w.vm as any).toggleAclDacl()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'set_config', { acl_dacl: true })

    confirmMock.mockImplementation(() => false)
    ;(w.vm as any).toggleAclDacl()
    expect(requestMock.mock.calls.filter(c => (c[2] as any)?.acl_dacl === false).length).toBe(0)
  })

  it('ACL fail-closed 开关带未开启 dacl 提示后缀；max_files 非法回退', async () => {
    const w = await mountView()
    const vm = w.vm as any
    vm.toggleAclStrict()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'set_config', { acl_strict: true })
    expect(useToast().toasts.some(t => t.message.includes('acl.dacl 未开'))).toBe(true)

    vm.aclMaxFilesInput = 'abc'
    vm.saveAclMaxFiles()
    expect(vm.aclMaxFilesInput).toBe('20000')
    expect(useToast().toasts.some(t => t.message.includes('1-4294967295'))).toBe(true)

    vm.aclMaxFilesInput = '5000'
    vm.saveAclMaxFiles()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'set_config', { acl_max_files: 5000 })
  })
})

describe('SandboxView Sandboxie 引擎动作', () => {
  it('7z 与 Sandboxie 安装成功/失败', async () => {
    const w = await mountView()
    await (w.vm as any).install7z()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'install_7z', undefined, 0)
    expect(useToast().toasts.some(t => t.message === '7z 环境就绪')).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'install_sandboxie' ? Promise.reject(new Error('net')) : defaultImpl(_m, cmd, {}))
    await (w.vm as any).installSandboxie()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('下载失败'))).toBe(true)
    expect((w.vm as any).busy).toBe(null)
  })

  it('启动/停止引擎：confirm 门与成功路径', async () => {
    const w = await mountView()
    confirmMock.mockImplementation(() => false)
    ;(w.vm as any).startSandboxie()
    expect(requestMock.mock.calls.filter(c => c[1] === 'start').length).toBe(0)

    confirmMock.mockImplementation(() => true)
    ;(w.vm as any).startSandboxie()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'start')
    expect(useToast().toasts.some(t => t.message.includes('Sandboxie 引擎已启动'))).toBe(true)

    ;(w.vm as any).stopSandboxie()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'stop')
    expect(useToast().toasts.some(t => t.message.includes('Sandboxie 引擎已停止'))).toBe(true)
  })

  it('启动失败提示', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'start' ? Promise.reject(new Error('uac')) : defaultImpl(_m, cmd, {}))
    const w = await mountView()
    ;(w.vm as any).startSandboxie()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('启动失败'))).toBe(true)
  })

  it('openBox/openExplorer/toggleNetwork', async () => {
    const w = await mountView()
    const vm = w.vm as any
    await vm.openBox()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'open_box')
    await vm.openExplorer()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'open_explorer')
    expect(useToast().toasts.some(t => t.message.includes('资源管理器'))).toBe(true)
    const before = vm.allowNetwork
    await vm.toggleNetwork()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'set_network', { enabled: !before })

    requestMock.mockImplementation((_m, cmd) => cmd === 'open_box' ? Promise.reject(new Error('no')) : defaultImpl(_m, cmd, {}))
    await vm.openBox()
    expect(useToast().toasts.some(t => t.message.includes('打开失败'))).toBe(true)
  })
})

describe('SandboxView 文件同步（files tab）', () => {
  async function mountWithFiles() {
    const w = await mountView()
    ;(w.vm as any).toggleFile('C:\\box\\out.txt')
    return w
  }

  it('toggleFile/isSel/selectAll/selectNone', async () => {
    const w = await mountWithFiles()
    const vm = w.vm as any
    expect(vm.isSel('C:\\box\\out.txt')).toBe(true)
    vm.toggleFile('C:\\box\\out.txt')
    expect(vm.isSel('C:\\box\\out.txt')).toBe(false)
    vm.selectAll()
    expect(vm.isSel('C:\\box\\out.txt')).toBe(true)
    vm.selectNone()
    expect(vm.isSel('C:\\box\\out.txt')).toBe(false)
  })

  it('同步：空选拦截；选中成功提示计数', async () => {
    const w = await mountView()
    const vm = w.vm as any
    await vm.syncSelected()
    expect(useToast().toasts.some(t => t.message.includes('请先勾选'))).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'commit').length).toBe(0)

    vm.toggleFile('C:\\box\\out.txt')
    await vm.syncSelected()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'commit', { files: ['C:\\box\\out.txt'] })
    expect(useToast().toasts.some(t => t.message.includes('已同步 1/1'))).toBe(true)
  })

  it('全量同步与失败', async () => {
    const w = await mountView()
    await (w.vm as any).syncAll()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'commit', { all: true })
    expect(useToast().toasts.some(t => t.message.includes('已同步全部'))).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'commit' ? Promise.reject(new Error('busy')) : defaultImpl(_m, cmd, {}))
    await (w.vm as any).syncAll()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('同步失败'))).toBe(true)
  })

  it('删除：空选拦截、confirm 门、部分失败文案', async () => {
    const w = await mountView()
    const vm = w.vm as any
    await vm.deleteSelected()
    expect(useToast().toasts.some(t => t.message.includes('请先勾选'))).toBe(true)

    vm.toggleFile('C:\\box\\out.txt')
    confirmMock.mockImplementation(() => false)
    await vm.deleteSelected()
    expect(requestMock.mock.calls.filter(c => c[1] === 'delete').length).toBe(0)

    confirmMock.mockImplementation(() => true)
    await vm.deleteSelected()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('sandbox', 'delete', { files: ['C:\\box\\out.txt'] })
    expect(useToast().toasts.some(t => t.message.includes('已从沙箱删除 1/1'))).toBe(true)
    expect(vm.isSel('C:\\box\\out.txt')).toBe(false)

    requestMock.mockImplementation((_m, cmd) => cmd === 'delete' ? Promise.resolve({ errors: ['e1'], deleted: 0, total: 1 }) : defaultImpl(_m, cmd, {}))
    vm.toggleFile('C:\\box\\out.txt')
    await vm.deleteSelected()
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('部分删除失败'))).toBe(true)
  })

  it('formatSize 三档', async () => {
    const w = await mountView()
    const vm = w.vm as any
    expect(vm.formatSize(0)).toBe('0B')
    expect(vm.formatSize(500)).toBe('500B')
    expect(vm.formatSize(2048)).toBe('2K')
    expect(vm.formatSize(2097152)).toBe('2.0M')
  })
})
