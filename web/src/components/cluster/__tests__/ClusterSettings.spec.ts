// ClusterSettings：集群基础设置契约（总开关/保存/重置/Token 重生成、
// 网络诊断、防火墙规则、地址配对、死信重放、快照清理）。
// 后端行为由 handlers/cluster.rs / outbox 死信处理钉住；本 spec 只钉前端编排。
// ClusterRoaming 是独立子组件（自带 roaming.status 拉取，另有测试面），
// 此处替换为空壳隔离噪声；Token 生成器 mock 为固定值以钉住 dirty 判定。
import { mount, flushPromises } from '@vue/test-utils'
import type { VueWrapper } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))
vi.mock('../ClusterRoaming.vue', () => ({
  default: { name: 'ClusterRoamingStub', render: () => null },
}))
vi.mock('../../../lib/uuid', () => ({
  uuidv4: () => 'fixed-uuid-4',
}))

import ClusterSettings from '../ClusterSettings.vue'

const CONFIG = {
  master_enabled: true,
  enabled: true,
  port: 11950,
  rpc_port: 21950,
  broadcast_interval: 45,
  llm_timeout_secs: 3600,
  token: 'tok-1',
}
const SNAP = { name: 'snap-1.tar', size: '1.2 MB', created: '2026-11-01' }
const DEAD = { task_id: 'task-dead-1', source_node: 'node-x', created_at: '2026-11-01', attempts: 3, last_error: 'boom' }

function defaultImpl(_m: string, cmd: string, _p?: any) {
  switch (cmd) {
    case 'config.get': return Promise.resolve(CONFIG)
    case 'snapshots.list': return Promise.resolve({ snapshots: [SNAP] })
    case 'dead_list': return Promise.resolve({ dead_letters: [DEAD] })
    default: return Promise.resolve({})
  }
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation(defaultImpl)
  useToast().toasts.splice(0)
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

async function mountSettings(impl: (_m: string, cmd: string, p?: any) => Promise<any> = defaultImpl) {
  requestMock.mockImplementation(impl)
  const w = mount(ClusterSettings)
  await flushPromises()
  return w as VueWrapper<any>
}

function btn(w: VueWrapper<any>, text: string) {
  return w.findAll('button').find(b => b.text() === text)!
}

function inputByValue(w: VueWrapper<any>, v: string) {
  return w.findAll('input').find(i => (i.element as HTMLInputElement).value === v)!
}

function hasToast(msg: string) {
  return useToast().toasts.some(t => t.message.includes(msg))
}

describe('ClusterSettings 挂载渲染', () => {
  it('加载配置/快照/死信并渲染', async () => {
    const w = await mountSettings()
    expect(w.text()).toContain('基础设置')
    expect(w.text()).toContain('snap-1.tar')
    expect(w.text()).toContain('task-dead-1')
    expect(w.text()).toContain('对端 node-x')
    expect(w.text()).toContain('重试 3 次')
    expect(w.text()).toContain('boom')
    // masterEnabled=true → toggle active
    expect(w.find('.toggle').classes()).toContain('active')
    // token 回显
    expect((inputByValue(w, 'tok-1').element as HTMLInputElement).value).toBe('tok-1')
    w.unmount()
  })

  it('config.get 为 null / 全部失败：兜底默认值，不崩溃', async () => {
    const w = await mountSettings((_m, cmd) => {
      if (cmd === 'config.get') return Promise.resolve(null)
      if (cmd === 'snapshots.list') return Promise.reject('net')
      if (cmd === 'dead_list') return Promise.reject('net')
      return Promise.resolve({})
    })
    expect((inputByValue(w, '11949').element as HTMLInputElement).value).toBe('11949')
    expect(w.text()).toContain('暂无死信条目')
    expect(w.text()).toContain('暂无快照文件')
    w.unmount()
  })
})

describe('ClusterSettings 基础设置', () => {
  it('总开关：toggle 下发 set_master_enabled；失败提示', async () => {
    const w = await mountSettings()
    await w.find('.toggle').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'config.set_master_enabled', { enabled: false })
    expect(useToast().toasts.some(t => t.message === '集群功能已禁用')).toBe(true)
    expect(w.find('.toggle').classes()).not.toContain('active')

    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'config.set_master_enabled' ? Promise.reject('x') : defaultImpl(_m, cmd))
    await w.find('.toggle').trigger('click')
    await flushPromises()
    expect(hasToast('操作失败')).toBe(true)
    // 失败不翻转开关
    expect(w.find('.toggle').classes()).not.toContain('active')
    w.unmount()
  })

  it('重新生成 / 手动编辑 Token → 固定值 + 未保存提示', async () => {
    const w = await mountSettings()
    expect(w.text()).not.toContain('Token 已修改但未保存')
    await btn(w, '重新生成').trigger('click')
    expect((inputByValue(w, 'fixed-uuid-4').element as HTMLInputElement).value).toBe('fixed-uuid-4')
    expect(w.text()).toContain('Token 已修改但未保存')
    // 手动输入同样触发 dirty 提示
    await inputByValue(w, 'fixed-uuid-4').setValue('hand-edited-token')
    expect(w.text()).toContain('Token 已修改但未保存')
    w.unmount()
  })

  it('保存：payload 正确、成功提示、dirty 提示消失；失败提示', async () => {
    const w = await mountSettings()
    await btn(w, '保存').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'config.save', {
      enabled: true,
      port: 11950,
      rpc_port: 21950,
      broadcast_interval: 45,
      llm_timeout_secs: 3600,
      token: 'tok-1',
    })
    expect(hasToast('配置已保存')).toBe(true)

    // 编辑后 dirty 提示出现，保存后消失
    await btn(w, '重新生成').trigger('click')
    expect(w.text()).toContain('Token 已修改但未保存')
    await btn(w, '保存').trigger('click')
    await flushPromises()
    expect(w.text()).not.toContain('Token 已修改但未保存')

    // 保存失败
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'config.save' ? Promise.reject('locked') : defaultImpl(_m, cmd))
    await btn(w, '保存').trigger('click')
    await flushPromises()
    expect(hasToast('保存失败')).toBe(true)
    w.unmount()
  })

  it('重置：重新 loadConfig 恢复表单', async () => {
    const w = await mountSettings()
    await inputByValue(w, '11950').setValue('12345')
    await btn(w, '重置').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'config.get').length).toBe(2)
    expect((inputByValue(w, '11950').element as HTMLInputElement).value).toBe('11950')
    w.unmount()
  })
})

describe('ClusterSettings 网络诊断', () => {
  it('检测通过：映射标签/原始名/通过汇总/成功 toast', async () => {
    const w = await mountSettings((_m, cmd) =>
      cmd === 'firewall.check'
        ? Promise.resolve({ all_pass: true, tests: [
            { name: 'udp_bind', pass: true, detail: 'bound' },
            { name: 'custom_raw', pass: false, detail: 'nope' },
          ] })
        : defaultImpl(_m, cmd))
    await btn(w, '检测网络').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'firewall.check')
    expect(w.text()).toContain('UDP 端口绑定')
    expect(w.text()).toContain('custom_raw')
    expect(w.text()).toContain('bound')
    expect(w.text()).toContain('网络正常，集群通信就绪')
    expect(useToast().toasts.some(t => t.message === '网络检测通过')).toBe(true)
    w.unmount()
  })

  it('检测有问题：warn toast + 失败汇总', async () => {
    const w = await mountSettings((_m, cmd) =>
      cmd === 'firewall.check'
        ? Promise.resolve({ all_pass: false, tests: [{ name: 'udp_bind', pass: false, detail: 'in use' }] })
        : defaultImpl(_m, cmd))
    await btn(w, '检测网络').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('网络检测发现问题'))).toBe(true)
    expect(w.text()).toContain('存在问题，可能影响集群通信')
    w.unmount()
  })

  it('检测请求失败：error toast', async () => {
    const w = await mountSettings((_m, cmd) =>
      cmd === 'firewall.check' ? Promise.reject('net down') : defaultImpl(_m, cmd))
    await btn(w, '检测网络').trigger('click')
    await flushPromises()
    expect(hasToast('检测失败')).toBe(true)
    w.unmount()
  })

  it('添加防火墙规则：success / uac / permission_denied / 兜底 / 异常 全分支', async () => {
    const w = await mountSettings()

    // success
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'firewall.add_rules' ? Promise.resolve({ success: true, message: '已添加 2 条' }) : defaultImpl(_m, cmd))
    await btn(w, '添加防火墙规则').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'firewall.add_rules', { udp_port: 11950, tcp_port: 21950 })
    expect(useToast().toasts.some(t => t.message === '防火墙规则已添加')).toBe(true)
    expect(w.find('.fw-summary.pass').text()).toContain('已添加 2 条')

    // uac_triggered
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'firewall.add_rules'
        ? Promise.resolve({ uac_triggered: true, message: '需要管理员确认', manual_commands: ['netsh advfirewall add rule'] })
        : defaultImpl(_m, cmd))
    await btn(w, '添加防火墙规则').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('UAC 弹窗'))).toBe(true)
    expect(w.find('.fw-summary.uac').text()).toContain('需要管理员确认')
    expect(w.text()).toContain('netsh advfirewall add rule')

    // permission_denied
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'firewall.add_rules'
        ? Promise.resolve({ permission_denied: true, message: 'denied', platform_hint: '以管理员运行', manual_commands: ['cmd /c netsh ...'] })
        : defaultImpl(_m, cmd))
    await btn(w, '添加防火墙规则').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message.includes('权限不足'))).toBe(true)
    expect(w.text()).toContain('以管理员运行')
    expect(w.text()).toContain('cmd /c netsh ...')
    expect(w.find('.fw-summary.fail').exists()).toBe(true)

    // 兜底分支（无 success/uac/permission 标记）
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'firewall.add_rules' ? Promise.resolve({ success: false, message: '未知失败' }) : defaultImpl(_m, cmd))
    await btn(w, '添加防火墙规则').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '未知失败')).toBe(true)
    expect(w.find('.fw-summary.fail').text()).toContain('未知失败')

    // 请求异常
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'firewall.add_rules' ? Promise.reject('net down') : defaultImpl(_m, cmd))
    await btn(w, '添加防火墙规则').trigger('click')
    await flushPromises()
    expect(hasToast('添加失败')).toBe(true)
    w.unmount()
  })
})

describe('ClusterSettings 配对节点', () => {
  it('空地址拦截：warn 且不发请求', async () => {
    const w = await mountSettings()
    await btn(w, '配对').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '请输入对端地址')).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'pair').length).toBe(0)
    w.unmount()
  })

  it('配对成功：结果面板 + 成功 toast（含 peer_id 兜底地址文案）', async () => {
    const w = await mountSettings((_m, cmd) =>
      cmd === 'pair'
        ? Promise.resolve({ peer_id: 'peer-9', name: 'Peer Nine', rpc_address: '10.0.0.9:21950', udp_address: '10.0.0.9:11950', addresses: ['10.0.0.9:1'] })
        : defaultImpl(_m, cmd))
    const pairInput = w.findAll('input').find(i => (i.attributes('placeholder') as string)?.includes('例如 192.168.1.10'))!
    await pairInput.setValue('10.0.0.9:11950')
    await btn(w, '配对').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'pair', { address: '10.0.0.9:11950' })
    expect(w.text()).toContain('已配对并写入 peers.toml')
    expect(w.text()).toContain('peer-9')
    expect(w.text()).toContain('Peer Nine')
    expect(useToast().toasts.some(t => t.message === '配对成功：peer-9')).toBe(true)

    // 无 peer_id → toast 用地址；无 name → 名称行消失
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'pair' ? Promise.resolve({}) : defaultImpl(_m, cmd))
    await btn(w, '配对').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.message === '配对成功：10.0.0.9:11950')).toBe(true)
    expect(w.text()).not.toContain('名称：')
    w.unmount()
  })

  it('配对失败：失败面板 + error toast；Enter 键触发配对', async () => {
    const w = await mountSettings((_m, cmd) =>
      cmd === 'pair' ? Promise.reject('unreachable') : defaultImpl(_m, cmd))
    const pairInput = w.findAll('input').find(i => (i.attributes('placeholder') as string)?.includes('例如 192.168.1.10'))!
    await pairInput.setValue('10.0.0.9:11950')
    await btn(w, '配对').trigger('click')
    await flushPromises()
    expect(hasToast('配对失败')).toBe(true)
    expect(w.find('.fw-summary.fail').text()).toContain('unreachable')

    // @keyup.enter 也触发 pairNode
    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'pair' ? Promise.resolve({ peer_id: 'peer-enter' }) : defaultImpl(_m, cmd))
    const pairCalls = requestMock.mock.calls.filter(c => c[1] === 'pair').length
    await pairInput.trigger('keyup.enter')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'pair').length).toBe(pairCalls + 1)
    w.unmount()
  })
})

describe('ClusterSettings 发件箱死信', () => {
  it('刷新按钮重拉 dead_list', async () => {
    const w = await mountSettings()
    const base = requestMock.mock.calls.filter(c => c[1] === 'dead_list').length
    await btn(w, '刷新').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'dead_list').length).toBe(base + 1)
    w.unmount()
  })

  it('重放：成功 toast + 重拉列表；失败提示', async () => {
    const w = await mountSettings()
    await btn(w, '重放').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('outbox', 'dead_replay', { task_id: 'task-dead-1' })
    expect(hasToast('已重放入队：task-dead-1')).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'dead_list').length).toBe(2)

    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'dead_replay' ? Promise.reject('gone') : defaultImpl(_m, cmd))
    await btn(w, '重放').trigger('click')
    await flushPromises()
    expect(hasToast('重放失败')).toBe(true)
    w.unmount()
  })

  it('空死信列表显示空态', async () => {
    const w = await mountSettings((_m, cmd) =>
      cmd === 'dead_list' ? Promise.resolve({ dead_letters: [] }) : defaultImpl(_m, cmd))
    expect(w.text()).toContain('暂无死信条目')
    w.unmount()
  })
})

describe('ClusterSettings 续行快照', () => {
  it('清理全部：success toast + 重拉列表；失败提示', async () => {
    const w = await mountSettings()
    await btn(w, '清理全部').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('cluster', 'snapshots.cleanup')
    expect(useToast().toasts.some(t => t.message === '快照已清理')).toBe(true)
    expect(requestMock.mock.calls.filter(c => c[1] === 'snapshots.list').length).toBe(2)

    requestMock.mockImplementation((_m, cmd) =>
      cmd === 'snapshots.cleanup' ? Promise.reject('busy') : defaultImpl(_m, cmd))
    await btn(w, '清理全部').trigger('click')
    await flushPromises()
    expect(hasToast('清理失败')).toBe(true)
    w.unmount()
  })

  it('无快照：空态 + 清理按钮禁用', async () => {
    const w = await mountSettings((_m, cmd) =>
      cmd === 'snapshots.list' ? Promise.resolve({ snapshots: [] }) : defaultImpl(_m, cmd))
    expect(w.text()).toContain('暂无快照文件')
    expect(btn(w, '清理全部').attributes('disabled')).toBeDefined()
    w.unmount()
  })
})
