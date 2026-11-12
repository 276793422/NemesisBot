import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// ScannerView 单元测试（coverage 补齐 2026-11-11）：
// 引擎状态卡（徽标/行内编辑/操作按钮）、SSE 进度（complete/error/cancelled/
// 普通相位）、配置页签（只读栅格 + JSON 编辑）、空态与加载失败分支。

const requestMock = vi.fn()
const sseOnMock = vi.fn()
const sseOffMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))
vi.mock('../../composables/useSSE', () => ({
  on: (...args: any[]) => sseOnMock(...args),
  off: (...args: any[]) => sseOffMock(...args),
}))

import ScannerView from '../ScannerView.vue'

function makeEngine(over: Record<string, any> = {}) {
  return {
    name: 'clamav',
    enabled: true,
    url: 'https://database.clamav.net',
    clamav_path: '/usr/bin/clamscan',
    address: '127.0.0.1:3310',
    data_dir: '/var/lib/clamav',
    scan_on_write: true,
    scan_on_download: false,
    scan_on_exec: true,
    update_interval: 'daily',
    max_file_size: 100,
    scan_extensions: ['.exe'],
    skip_extensions: ['.txt'],
    state: {
      install_status: 'installed',
      install_error: '',
      last_install_attempt: '2026-01-01T08:00:00Z',
      db_status: 'ready',
      last_db_update: '2026-02-01T08:00:00Z',
    },
    ...over,
  }
}

const NOT_INSTALLED = { install_status: 'missing', install_error: '', last_install_attempt: '', db_status: '', last_db_update: '' }
const DB_MISSING = { install_status: 'installed', install_error: '', last_install_attempt: '', db_status: 'missing', last_db_update: '' }

function defaultImpl(_m: string, cmd: string, _p?: any) {
  switch (cmd) {
    case 'status':
      return Promise.resolve({ engines: [makeEngine()] })
    case 'config.get':
      return Promise.resolve({ scan_on_write: true, scan_on_download: false, max_file_size: 100, engine_name: 'clamav', nested: { hidden: true } })
    case 'check':
      return Promise.resolve({ engines: [{ name: 'clamav', enabled: true }] }) // 无 state → 补全默认 state 分支
    case 'enable':
      return Promise.resolve({ engines: [makeEngine()] })
    case 'disable':
      return Promise.resolve({ engines: [makeEngine()] })
    case 'install':
    case 'update_db':
    case 'add':
    case 'cancel':
    case 'config.save':
    case 'engine.update_config':
      return Promise.resolve({})
    case 'test':
      return Promise.resolve({ infected: false })
    default:
      return Promise.resolve({})
  }
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation(defaultImpl)
  sseOnMock.mockClear()
  sseOffMock.mockClear()
  useToast().toasts.splice(0)
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

async function mountView() {
  const w = mount(ScannerView)
  await flushPromises()
  return w
}

function sseHandler(): (data: any) => void {
  const call = sseOnMock.mock.calls.find((c) => c[0] === 'scanner-progress')
  return call![1]
}

describe('ScannerView 加载与生命周期', () => {
  it('初始 loading 显示 spinner，加载完成后消失', async () => {
    const w = mount(ScannerView)
    expect(w.find('.spinner').exists()).toBe(true)
    await flushPromises()
    expect(w.find('.spinner').exists()).toBe(false)
    w.unmount()
  })

  it('挂载后拉取 status + config.get，渲染引擎卡与共享层头', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('scanner', 'status')
    expect(requestMock).toHaveBeenCalledWith('scanner', 'config.get')
    expect(w.text()).toContain('病毒扫描')
    expect(w.text()).toContain('CLAMAV')
    expect(w.text()).toContain('已安装')
    expect(w.text()).toContain('就绪')
    expect(w.text()).toContain('启用 1/1')
    expect(w.text()).toContain('/usr/bin/clamscan')
    expect(w.text()).toContain('127.0.0.1:3310')
    expect(sseOnMock).toHaveBeenCalledWith('scanner-progress', expect.any(Function))
    w.unmount()
  })

  it('卸载时解绑 SSE 与 document click 监听', async () => {
    const removeSpy = vi.spyOn(document, 'removeEventListener')
    const w = await mountView()
    const handler = sseHandler()
    w.unmount()
    expect(sseOffMock).toHaveBeenCalledWith('scanner-progress', handler)
    expect(removeSpy).toHaveBeenCalledWith('click', expect.any(Function))
    removeSpy.mockRestore()
  })

  it('status 响应缺 state 字段时补全默认值', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'status' ? Promise.resolve({ engines: [{ name: 'bare', enabled: true, url: 'u' }] }) : defaultImpl(_m, cmd))
    const w = await mountView()
    expect(w.text()).toContain('BARE')
    expect(w.text()).toContain('未配置')
    expect(w.text()).toContain('--')
    w.unmount()
  })

  it('status 失败 console.error 且 UI 仍可用；config.get 失败静默忽略', async () => {
    const errSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    requestMock.mockImplementation((_m, cmd) => {
      if (cmd === 'status') return Promise.reject(new Error('ws down'))
      if (cmd === 'config.get') return Promise.reject(new Error('nope'))
      return defaultImpl(_m, cmd)
    })
    const w = await mountView()
    expect(errSpy).toHaveBeenCalled()
    expect(w.text()).toContain('未检测到扫描引擎')
    expect(w.text()).toContain('未启用')
    await w.findAll('button.tab').find(b => b.text() === '配置')!.trigger('click')
    expect(w.text()).toContain('Scanner 配置')
    errSpy.mockRestore()
    w.unmount()
  })
})

describe('ScannerView 引擎卡渲染', () => {
  it('徽标全变体：pending/failed/未配置 + missing/stale/-- + 禁用态 + 安装错误', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'status' ? Promise.resolve({
      engines: [
        makeEngine({ name: 'eng-a', state: { install_status: 'pending', install_error: '', last_install_attempt: '', db_status: 'missing', last_db_update: '' } }),
        makeEngine({ name: 'eng-b', enabled: false, state: { install_status: 'failed', install_error: '下载超时', last_install_attempt: '', db_status: 'stale', last_db_update: '' } }),
        makeEngine({ name: 'eng-c', state: { install_status: '', install_error: '', last_install_attempt: '', db_status: '', last_db_update: '' } }),
      ],
    }) : defaultImpl(_m, cmd))
    const w = await mountView()
    expect(w.text()).toContain('待安装')
    expect(w.text()).toContain('安装失败')
    expect(w.text()).toContain('下载超时')
    expect(w.text()).toContain('未配置')
    expect(w.text()).toContain('缺失')
    expect(w.text()).toContain('需更新')
    expect(w.text()).toContain('已禁用')
    expect(w.text()).toContain('启用 2/3')
    w.unmount()
  })

  it('空字段显示「点击设置」占位，四个字段逐个进入/退出编辑', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'status'
      ? Promise.resolve({ engines: [makeEngine({ name: 'fresh', clamav_path: '', address: '', url: '', data_dir: '', state: { install_status: '', install_error: '', last_install_attempt: '', db_status: '', last_db_update: '' } })] })
      : defaultImpl(_m, cmd))
    const w = await mountView()
    expect(w.findAll('.editable-placeholder').length).toBe(4)
    for (let i = 0; i < 4; i++) {
      await w.findAll('.editable-val')[i]!.trigger('click')
      expect(w.find('.inline-edit').exists()).toBe(true)
      await w.find('.inline-edit-input').trigger('keyup', { key: 'Escape' })
      expect(w.find('.inline-edit').exists()).toBe(false)
    }
    w.unmount()
  })

  it('formatTime / getOpLabel 辅助函数分支', async () => {
    const w = await mountView()
    const vm = w.vm as any
    expect(vm.formatTime('2026-01-01T08:00:00Z')).toContain('2026')
    expect(vm.formatTime('')).toBe('--')
    // 本环境 Invalid Date.toLocaleString 不抛错而是返回 "Invalid Date"
    expect(vm.formatTime('garbage-time')).toBe('Invalid Date')
    // new Date(Symbol) 在 try 内抛 TypeError → catch 原样返回入参
    const sym = Symbol('weird')
    expect(vm.formatTime(sym as any)).toBe(sym)
    expect(vm.getOpLabel('updating-db')).toBe('更新数据库...')
    expect(vm.getOpLabel('downloading-db')).toBe('下载病毒库...')
    expect(vm.getOpLabel('mystery')).toBe('mystery')
    w.unmount()
  })
})

describe('ScannerView 引擎操作', () => {
  it('toggle：禁用引擎 → enable；启用引擎 → disable；失败提示', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'status' ? Promise.resolve({ engines: [makeEngine({ enabled: false })] }) : defaultImpl(_m, cmd))
    const w = await mountView()
    await w.find('.toggle').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('scanner', 'enable', { name: 'clamav' })
    expect(useToast().toasts.some(t => t.message === 'clamav 已启用')).toBe(true)
    w.unmount()
  })

  it('toggle disable：响应 engines 数组补全默认 state 并刷新', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'disable'
      ? Promise.resolve({ engines: [{ name: 'clamav', enabled: false }] })
      : defaultImpl(_m, cmd))
    const w = await mountView()
    await w.find('.toggle').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === 'clamav 已禁用')).toBe(true)
    expect(w.text()).toContain('已禁用')
    expect(w.text()).toContain('未配置') // 新引擎对象无 state → 默认徽标
    w.unmount()
  })

  it('toggle 失败提示操作失败', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'disable' ? Promise.reject(new Error('busy')) : defaultImpl(_m, cmd))
    const w = await mountView()
    await w.find('.toggle').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('操作失败'))).toBe(true)
    w.unmount()
  })

  it('检查状态：忙碌态进度条 → engines 响应刷新 + toast', async () => {
    // check 请求挂起，让忙碌态停留到断言之后再完成
    let resolveCheck!: (v: any) => void
    requestMock.mockImplementation((_m, cmd) => cmd === 'check' ? new Promise((res) => { resolveCheck = res }) : defaultImpl(_m, cmd))
    const w = await mountView()
    const clickP = w.findAll('button').find(b => b.text() === '检查状态')!.trigger('click')
    await nextTick()
    expect(w.text()).toContain('检查中...')
    expect(w.text()).toContain('停止')
    resolveCheck({ engines: [makeEngine()] })
    await clickP
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('scanner', 'check', { name: 'clamav' })
    expect(useToast().toasts.some(t => t.message === 'clamav 状态检查完成')).toBe(true)
    expect(w.text()).not.toContain('检查中...')
    // 第二次检查：无 state 响应 → eng.state || 默认补全分支
    requestMock.mockImplementation(defaultImpl)
    await w.findAll('button').find(b => b.text() === '检查状态')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.filter(t => t.message === 'clamav 状态检查完成').length).toBe(2)
    expect(w.text()).toContain('未配置')
    w.unmount()
  })

  it('检查状态：单引擎响应走 updateSingleEngine 更新该卡（无 state → 沿用原 state）', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'check'
      ? Promise.resolve({ name: 'clamav', enabled: true, address: '127.0.0.1:9999' })
      : defaultImpl(_m, cmd))
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '检查状态')!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('127.0.0.1:9999')
    expect(w.text()).toContain('已安装') // 原 state 保留
    w.unmount()
  })

  it('检查状态失败：toast + 清除忙碌态', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'check' ? Promise.reject(new Error('timeout')) : defaultImpl(_m, cmd))
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '检查状态')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('检查失败'))).toBe(true)
    expect(w.text()).not.toContain('检查中...')
    w.unmount()
  })

  it('安装：成功后台提示且忙碌态保留（等 SSE）；失败提示并清除', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'status' ? Promise.resolve({ engines: [makeEngine({ state: { ...NOT_INSTALLED } })] }) : defaultImpl(_m, cmd))
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '安装')!.trigger('click')
    expect(w.text()).toContain('准备安装...')
    expect(w.text()).toContain('5%')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('scanner', 'install', { name: 'clamav' })
    expect(useToast().toasts.some(t => t.message === 'clamav 安装已在后台启动')).toBe(true)
    expect(w.text()).toContain('准备安装...') // 后台任务：无响应不清忙碌态
    w.unmount()
  })

  it('安装失败：toast + 清除忙碌态', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'install' ? Promise.reject(new Error('denied')) : cmd === 'status' ? Promise.resolve({ engines: [makeEngine({ state: { ...NOT_INSTALLED } })] }) : defaultImpl(_m, cmd))
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '安装')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('安装失败'))).toBe(true)
    expect(w.text()).not.toContain('准备安装...')
    w.unmount()
  })

  it('更新数据库：missing → 按钮可见；成功后台提示；失败提示并清除', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'status' ? Promise.resolve({ engines: [makeEngine({ state: { ...DB_MISSING } })] }) : defaultImpl(_m, cmd))
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '更新数据库')!.trigger('click')
    expect(w.text()).toContain('准备更新...')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('scanner', 'update_db', { name: 'clamav' })
    expect(useToast().toasts.some(t => t.message === 'clamav 数据库更新已在后台启动')).toBe(true)
    w.unmount()
  })

  it('更新数据库失败：toast + 清除忙碌态', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'update_db' ? Promise.reject(new Error('net')) : cmd === 'status' ? Promise.resolve({ engines: [makeEngine({ state: { ...DB_MISSING } })] }) : defaultImpl(_m, cmd))
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '更新数据库')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('更新失败'))).toBe(true)
    expect(w.text()).not.toContain('准备更新...')
    w.unmount()
  })

  it('测试扫描：取消 prompt / 安全 / 感染 / 未知病毒 / 失败', async () => {
    const w = await mountView()
    // prompt 取消 → 不发请求
    const promptSpy = vi.fn(() => null)
    vi.stubGlobal('prompt', promptSpy)
    await w.findAll('button').find(b => b.text() === '测试扫描')!.trigger('click')
    await flushPromises()
    expect(promptSpy).toHaveBeenCalled()
    expect(requestMock.mock.calls.filter(c => c[1] === 'test').length).toBe(0)

    // 安全
    vi.stubGlobal('prompt', vi.fn(() => '/tmp/a.exe'))
    await w.findAll('button').find(b => b.text() === '测试扫描')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('scanner', 'test', { name: 'clamav', path: '/tmp/a.exe' })
    expect(useToast().toasts.some(t => t.message === '扫描完成: 文件安全')).toBe(true)

    // 感染（带病毒名）
    requestMock.mockImplementation((_m, cmd) => cmd === 'test' ? Promise.resolve({ infected: true, virus: 'EICAR' }) : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '测试扫描')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'warn' && t.message.includes('检测到威胁') && t.message.includes('EICAR'))).toBe(true)

    // 感染（无病毒名 → 未知病毒）
    requestMock.mockImplementation((_m, cmd) => cmd === 'test' ? Promise.resolve({ infected: true }) : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '测试扫描')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'warn' && t.message.includes('未知病毒'))).toBe(true)

    // 失败
    requestMock.mockImplementation((_m, cmd) => cmd === 'test' ? Promise.reject(new Error('bad')) : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '测试扫描')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('测试失败'))).toBe(true)
    w.unmount()
  })

  it('空态添加 ClamAV：成功刷新；失败提示', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'status' ? Promise.resolve({}) : cmd === 'config.get' ? Promise.resolve(null) : defaultImpl(_m, cmd))
    const w = await mountView()
    expect(w.text()).toContain('未检测到扫描引擎')
    await w.findAll('button').find(b => b.text() === '添加 ClamAV')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('scanner', 'add', { name: 'clamav' })
    expect(useToast().toasts.some(t => t.message === 'ClamAV 引擎已添加')).toBe(true)

    requestMock.mockImplementation((_m, cmd) => cmd === 'add' ? Promise.reject(new Error('no net')) : cmd === 'status' ? Promise.resolve({}) : cmd === 'config.get' ? Promise.resolve(null) : defaultImpl(_m, cmd))
    await w.findAll('button').find(b => b.text() === '添加 ClamAV')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('添加失败'))).toBe(true)
    w.unmount()
  })

  it('幽灵引擎操作早退（不存在的引擎不发请求）', async () => {
    const w = await mountView()
    const vm = w.vm as any
    await vm.checkEngine('ghost')
    await vm.installEngine('ghost')
    await vm.updateDb('ghost')
    vm.clearOp('ghost')
    vm.updateSingleEngine({ name: 'ghost' })
    vm.handleProgress({ engine: 'ghost', phase: 'complete' })
    expect(requestMock.mock.calls.filter(c => ['check', 'install', 'update_db'].includes(c[1])).length).toBe(0)
    expect(useToast().toasts.some(t => t.message.includes('ghost'))).toBe(false)
    w.unmount()
  })

  it('停止操作：成功后 500ms 自动刷新；失败提示', async () => {
    vi.useFakeTimers()
    const w = await mountView()
    const handler = sseHandler()
    handler({ engine: 'clamav', phase: 'installing', progress: 30, message: '下载中...' })
    await flushPromises()
    expect(w.text()).toContain('下载中...')
    expect(w.text()).toContain('30%')
    requestMock.mockClear()
    await w.findAll('button').find(b => b.text() === '停止')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('scanner', 'cancel', { name: 'clamav' })
    expect(useToast().toasts.some(t => t.message === 'clamav 操作已取消')).toBe(true)
    const statusCalls = requestMock.mock.calls.filter(c => c[1] === 'status').length
    await vi.advanceTimersByTimeAsync(500)
    expect(requestMock.mock.calls.filter(c => c[1] === 'status').length).toBe(statusCalls + 1)

    requestMock.mockImplementation((_m, cmd) => cmd === 'cancel' ? Promise.reject(new Error('x')) : cmd === 'status' ? Promise.resolve({ engines: [makeEngine()] }) : defaultImpl(_m, cmd))
    // 500ms 刷新把忙碌态清掉了 → 重新注入 SSE 相位恢复「停止」按钮
    handler({ engine: 'clamav', phase: 'installing' })
    await flushPromises()
    await w.findAll('button').find(b => b.text() === '停止')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('取消失败'))).toBe(true)
    w.unmount()
  })
})

describe('ScannerView SSE 进度', () => {
  it('complete/error/cancelled 提示（各文案两变体）+ 各安排 500ms 刷新；普通相位更新进度', async () => {
    vi.useFakeTimers()
    const w = await mountView()
    const handler = sseHandler()
    handler({ engine: 'clamav', phase: 'complete', message: '全部完成' })
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '全部完成')).toBe(true)
    handler({ engine: 'clamav', phase: 'error', message: '炸了' })
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message === '炸了')).toBe(true)
    handler({ engine: 'clamav', phase: 'cancelled' })
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === 'clamav 操作已取消')).toBe(true)
    handler({ engine: 'clamav', phase: 'complete' }) // 无 message → 默认文案
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === 'clamav 操作完成')).toBe(true)
    handler({ engine: 'clamav', phase: 'error' }) // 无 message → 默认文案
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message === 'clamav 操作失败')).toBe(true)
    handler({ engine: 'clamav', phase: 'cancelled', message: '手动停止' }) // 带 message
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '手动停止')).toBe(true)

    const before = requestMock.mock.calls.filter(c => c[1] === 'status').length
    await vi.advanceTimersByTimeAsync(500)
    expect(requestMock.mock.calls.filter(c => c[1] === 'status').length).toBe(before + 6)

    handler({ engine: 'clamav', phase: 'extracting', progress: 66, message: '解压中' })
    await flushPromises()
    expect(w.text()).toContain('解压中')
    expect(w.text()).toContain('66%')
    expect(w.find('.progress-bar').attributes('style')).toContain('66%')
    // 无 phase 的裸进度 → operation 保留（extracting）、progress 更新、progressMsg 清空回退标签
    handler({ engine: 'clamav', progress: 10 })
    await flushPromises()
    expect(w.text()).toContain('10%')
    expect(w.text()).toContain('解压中...')
    w.unmount()
  })
})

describe('ScannerView 行内编辑', () => {
  it('进入编辑 → 保存成功回读 → 保存失败保留 → Escape/取消按钮退出', async () => {
    const w = await mountView()
    await w.findAll('.editable-val').find(s => s.text().includes('/usr/bin/clamscan'))!.trigger('click')
    expect(w.find('.inline-edit').exists()).toBe(true)
    const input = w.find('.inline-edit-input')
    expect((input.element as HTMLInputElement).value).toBe('/usr/bin/clamscan')
    // 未修改 → 保存按钮无 btn-primary；修改后 → 有
    expect(w.findAll('.inline-edit button').find(b => b.text() === '保存')!.classes()).not.toContain('btn-primary')
    await input.setValue('/opt/clamscan')
    expect(w.findAll('.inline-edit button').find(b => b.text() === '保存')!.classes()).toContain('btn-primary')
    await w.findAll('.inline-edit button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('scanner', 'engine.update_config', { name: 'clamav', config: { clamav_path: '/opt/clamscan' } })
    expect(useToast().toasts.some(t => t.message === '配置已保存')).toBe(true)
    expect(w.find('.inline-edit').exists()).toBe(false)

    // 保存失败：toast 且不退出编辑态
    await w.findAll('.editable-val')[0]!.trigger('click')
    await w.find('.inline-edit-input').setValue('/bad')
    requestMock.mockImplementation((_m, cmd) => cmd === 'engine.update_config' ? Promise.reject(new Error('readonly')) : defaultImpl(_m, cmd))
    await w.findAll('.inline-edit button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保存失败'))).toBe(true)
    expect(w.find('.inline-edit').exists()).toBe(true)

    // 恢复成功 mock，后续字段走保存成功链路
    requestMock.mockImplementation(defaultImpl)

    // Escape 取消
    await w.find('.inline-edit-input').trigger('keyup', { key: 'Escape' })
    expect(w.find('.inline-edit').exists()).toBe(false)

    // 监听地址 → 取消按钮
    await w.findAll('.editable-val').find(s => s.text() === '127.0.0.1:3310')!.trigger('click')
    expect(w.find('.inline-edit').exists()).toBe(true)
    await w.findAll('.inline-edit button').find(b => b.text() === '取消')!.trigger('click')
    expect(w.find('.inline-edit').exists()).toBe(false)

    // 下载地址 / 数据目录：各自字段走「进入编辑 → v-model 输入 → 保存」完整链路
    for (const [field, oldVal, newVal] of [
      ['url', 'https://database.clamav.net', 'https://mirror.example.com'],
      ['data_dir', '/var/lib/clamav', '/data/clamav-new'],
    ] as const) {
      await w.findAll('.editable-val').find(s => s.text() === oldVal)!.trigger('click')
      expect(w.find('.inline-edit').exists()).toBe(true)
      await w.find('.inline-edit-input').setValue(newVal)
      await w.findAll('.inline-edit button').find(b => b.text() === '保存')!.trigger('click')
      await flushPromises()
      expect(requestMock).toHaveBeenCalledWith('scanner', 'engine.update_config', { name: 'clamav', config: { [field]: newVal } })
      expect(w.find('.inline-edit').exists()).toBe(false)
    }

    // 监听地址保存（换值后保存链路）
    await w.findAll('.editable-val').find(s => s.text() === '127.0.0.1:3310')!.trigger('click')
    await w.find('.inline-edit-input').setValue('127.0.0.1:9999')
    await w.findAll('.inline-edit button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('scanner', 'engine.update_config', { name: 'clamav', config: { address: '127.0.0.1:9999' } })
    expect(w.find('.inline-edit').exists()).toBe(false)
    w.unmount()
  })

  it('handleClickOutside：非编辑态早退；点击 inline-edit/editable-val 内部不取消；点击外部取消', async () => {
    const w = await mountView()
    const vm = w.vm as any
    // 未在编辑态 → 直接返回
    vm.handleClickOutside({ target: { closest: () => null } })
    await w.findAll('.editable-val')[0]!.trigger('click')
    expect(w.find('.inline-edit').exists()).toBe(true)
    vm.handleClickOutside({ target: { closest: (sel: string) => (sel === '.inline-edit' ? {} : null) } })
    expect(w.find('.inline-edit').exists()).toBe(true)
    vm.handleClickOutside({ target: { closest: (sel: string) => (sel === '.editable-val' ? {} : null) } })
    expect(w.find('.inline-edit').exists()).toBe(true)
    vm.handleClickOutside({ target: { closest: () => null } })
    await nextTick()
    expect(w.find('.inline-edit').exists()).toBe(false)
    // 真实 DOM 冒泡：body 点击 → 取消
    await w.findAll('.editable-val')[0]!.trigger('click')
    document.body.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await flushPromises()
    expect(w.find('.inline-edit').exists()).toBe(false)
    w.unmount()
  })
})

describe('ScannerView 配置页签', () => {
  it('只读栅格：布尔值转是/否，对象值跳过', async () => {
    const w = await mountView()
    await w.findAll('button.tab').find(b => b.text() === '配置')!.trigger('click')
    expect(w.text()).toContain('Scanner 配置')
    expect(w.text()).toContain('是')
    expect(w.text()).toContain('否')
    expect(w.text()).toContain('100')
    expect(w.text()).toContain('clamav')
    expect(w.text()).not.toContain('nested') // 对象值不进栅格
    // 切回引擎状态页签
    await w.findAll('button.tab').find(b => b.text() === '引擎状态')!.trigger('click')
    expect((w.vm as any).activeTab).toBe('engines')
    expect(w.text()).toContain('检查状态')
    w.unmount()
  })

  it('JSON 编辑：保存成功回读；取消退出；非法 JSON 提示且留在编辑态', async () => {
    const w = await mountView()
    await w.findAll('button.tab').find(b => b.text() === '配置')!.trigger('click')
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    const ta = w.find('textarea')
    expect(ta.exists()).toBe(true)
    await ta.setValue('{"scan_on_write": false}')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('scanner', 'config.save', { scan_on_write: false })
    expect(useToast().toasts.some(t => t.message === '配置已保存')).toBe(true)
    expect(w.find('textarea').exists()).toBe(false)

    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await w.findAll('button').find(b => b.text() === '取消')!.trigger('click')
    expect(w.find('textarea').exists()).toBe(false)

    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await w.find('textarea').setValue('{broken')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('保存失败'))).toBe(true)
    expect(w.find('textarea').exists()).toBe(true)
    w.unmount()
  })
})
