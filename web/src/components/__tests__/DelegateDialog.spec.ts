import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// 角色目录与客户端委派（2026-09-28）：委派弹窗测试。
// mock useWSAPI（roles.list / chat.spawn 的后端契约由 Rust 侧
// roles_spawn_tests.rs + role_catalog_tests.rs 钉住，这里测弹窗交互层：
// 目录渲染与置灰、校验、提交载荷、fire-and-forget 发出即关 +
// 完成/失败 toast 异步通知（2026-11-04 委派 UX）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...a: any[]) => requestMock(...a) }),
}))

import DelegateDialog from '../DelegateDialog.vue'

const ROLES_LIST = {
  tier: 'big',
  visible_count: 2,
  roles: [
    { slug: 'explorer', description: '只读侦察', min_tier: 'mini', visible: true, hidden: false },
    { slug: 'qa', description: '质量审查', min_tier: 'mini', visible: true, hidden: false },
    { slug: 'coordinator', description: '多代理编排', min_tier: 'big', visible: false, hidden: false },
    { slug: 'fork', description: '会话分叉', min_tier: 'big', visible: false, hidden: true },
  ],
}

function mountDialog() {
  return mount(DelegateDialog, { props: { sessionId: 'sid-1' } })
}

beforeEach(() => {
  requestMock.mockReset().mockImplementation((module: string, cmd: string) => {
    if (module === 'roles' && cmd === 'list') return Promise.resolve(ROLES_LIST)
    return Promise.resolve({})
  })
  useToast().toasts.splice(0)
})

describe('DelegateDialog 角色目录', () => {
  it('打开即拉 roles.list，渲染全量目录 + 档位提示', async () => {
    const w = mountDialog()
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('roles', 'list')
    const options = w.findAll('select').at(0)!.findAll('option')
    // 缺省「自动」+ 4 个目录项（不可见项也展示——诚实目录）。
    expect(options.length).toBe(5)
    expect(options[1].text()).toContain('explorer')
    expect(options[1].text()).toContain('只读侦察')
    expect(options[3].text()).toContain('coordinator')
    expect(options[3].text()).toContain('需 big 档')
    expect(options[4].text()).toContain('fork')
    expect(options[4].text()).toContain('已隐藏')
    expect(w.find('.hint').text()).toContain('big')
    expect(w.find('.hint').text()).toContain('2')
  })

  it('roles.list 失败 → toast 错误并关闭弹窗', async () => {
    requestMock.mockRejectedValue('agent loop not running')
    const w = mountDialog()
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('agent loop not running'))).toBe(true)
    expect(w.emitted('close')).toBeTruthy()
  })

  it('不可见角色 option 置灰（visible=false → disabled）', async () => {
    const w = mountDialog()
    await flushPromises()
    const options = w.findAll('select').at(0)!.findAll('option')
    expect(options[1].attributes('disabled')).toBeUndefined()
    expect(options[3].attributes('disabled')).toBeDefined()
    expect(options[4].attributes('disabled')).toBeDefined()
  })
})

describe('DelegateDialog 委派提交', () => {
  it('空任务 → 行内提示，不发 chat.spawn', async () => {
    const w = mountDialog()
    await flushPromises()
    await w.findAll('button').find(b => b.text() === '委派')!.trigger('click')
    await flushPromises()
    expect(w.find('.inline-error').text()).toContain('请填写任务描述')
    expect(requestMock).not.toHaveBeenCalledWith('chat', 'spawn', expect.anything())
  })

  it('fire-and-forget：发出即 emit done+close，完成 toast 异步补报', async () => {
    let resolveSpawn!: (v: any) => void
    requestMock.mockImplementation((module: string, cmd: string) => {
      if (module === 'roles' && cmd === 'list') return Promise.resolve(ROLES_LIST)
      if (module === 'chat' && cmd === 'spawn')
        return new Promise(res => { resolveSpawn = res })
      return Promise.resolve({})
    })
    const w = mountDialog()
    await flushPromises()
    const selects = w.findAll('select')
    await selects.at(0)!.setValue('qa')
    await selects.at(1)!.setValue('readonly')
    await w.find('.task-input').setValue('跑一遍单元测试')
    await w.findAll('button').find(b => b.text() === '委派')!.trigger('click')
    await flushPromises()

    // 第 4 参 timeoutMs=0：委派是分钟级任务，禁用 30s 默认超时（假失败根因修复）。
    expect(requestMock).toHaveBeenCalledWith('chat', 'spawn', {
      session_id: 'sid-1',
      task: '跑一遍单元测试',
      role: 'qa',
      tools_profile: 'readonly',
    }, 0)
    // 发出即关（done 在发送时触发，payload 只有 task/role——结果尚不存在）；
    // 子代理还在跑（spawn 未 resolve）弹窗就已经关闭 = 不阻塞用户前台工作。
    expect(w.emitted('done')![0]).toEqual([{ task: '跑一遍单元测试', role: 'qa' }])
    expect(w.emitted('close')).toBeTruthy()
    expect(useToast().toasts.some(t => t.type === 'info' && t.message.includes('委派已发出'))).toBe(true)
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(false)

    // 完成后（server 落盘 + subagent_reply push）toast 异步补报结果。
    resolveSpawn({ role: 'qa', result: '测试全绿' })
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('委派完成'))).toBe(true)
  })

  it('全量工具档位透传 full', async () => {
    const w = mountDialog()
    await flushPromises()
    await w.findAll('select').at(1)!.setValue('full')
    await w.find('.task-input').setValue('重构这个模块')
    await w.findAll('button').find(b => b.text() === '委派')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('chat', 'spawn', expect.objectContaining({ tools_profile: 'full' }), 0)
  })

  it('失败：不回显弹窗（已关闭），错误走 toast 通知', async () => {
    requestMock.mockImplementation((module: string, cmd: string) => {
      if (module === 'roles' && cmd === 'list') return Promise.resolve(ROLES_LIST)
      if (module === 'chat' && cmd === 'spawn')
        return Promise.reject("role 'qa' is not available for the current model tier")
      return Promise.resolve({})
    })
    const w = mountDialog()
    await flushPromises()
    await w.find('.task-input').setValue('审查这个 diff')
    await w.findAll('button').find(b => b.text() === '委派')!.trigger('click')
    await flushPromises()
    // fire-and-forget：失败不阻塞——弹窗照常关闭、done 照常发出，错误只经
    // toast 通知（回复/失败注记由服务端落进会话，subagent_reply resync）。
    expect(w.emitted('done')).toBeTruthy()
    expect(w.emitted('close')).toBeTruthy()
    expect(w.find('.inline-error').exists()).toBe(false)
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('委派失败'))).toBe(true)
  })
})
