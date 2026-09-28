import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// 角色目录与客户端委派（2026-09-28）：委派弹窗测试。
// mock useWSAPI（roles.list / chat.spawn 的后端契约由 Rust 侧
// roles_spawn_tests.rs + role_catalog_tests.rs 钉住，这里测弹窗交互层：
// 目录渲染与置灰、校验、提交载荷、失败回显不关弹窗）。

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

  it('成功：chat.spawn 带载荷 → toast + emit done + 关闭', async () => {
    requestMock.mockImplementation((module: string, cmd: string) => {
      if (module === 'roles' && cmd === 'list') return Promise.resolve(ROLES_LIST)
      if (module === 'chat' && cmd === 'spawn')
        return Promise.resolve({ session_id: 'sid-1', role: 'qa', tools_profile: 'readonly', result: '测试全绿' })
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

    expect(requestMock).toHaveBeenCalledWith('chat', 'spawn', {
      session_id: 'sid-1',
      task: '跑一遍单元测试',
      role: 'qa',
      tools_profile: 'readonly',
    })
    expect(w.emitted('done')![0]).toEqual([{ role: 'qa', result: '测试全绿' }])
    expect(w.emitted('close')).toBeTruthy()
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('委派完成'))).toBe(true)
  })

  it('全量工具档位透传 full', async () => {
    const w = mountDialog()
    await flushPromises()
    await w.findAll('select').at(1)!.setValue('full')
    await w.find('.task-input').setValue('重构这个模块')
    await w.findAll('button').find(b => b.text() === '委派')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('chat', 'spawn', expect.objectContaining({ tools_profile: 'full' }))
  })

  it('失败：错误原文回显、不 emit done、按钮复位可重试', async () => {
    requestMock.mockImplementation((module: string, cmd: string) => {
      if (module === 'roles' && cmd === 'list') return Promise.resolve(ROLES_LIST)
      if (module === 'chat' && cmd === 'spawn')
        return Promise.reject("role 'qa' is not available for the current model tier")
      return Promise.resolve({})
    })
    const w = mountDialog()
    await flushPromises()
    await w.find('.task-input').setValue('审查这个 diff')
    const btn = w.findAll('button').find(b => b.text() === '委派')!
    await btn.trigger('click')
    await flushPromises()
    expect(w.emitted('done')).toBeFalsy()
    expect(w.find('.inline-error').text()).toContain('not available')
    expect(useToast().toasts.some(t => t.type === 'error')).toBe(true)
    expect(btn.attributes('disabled')).toBeUndefined()

    // 重试成功
    requestMock.mockImplementation((module: string, cmd: string) => {
      if (module === 'roles' && cmd === 'list') return Promise.resolve(ROLES_LIST)
      if (module === 'chat' && cmd === 'spawn')
        return Promise.resolve({ role: 'qa', result: 'ok' })
      return Promise.resolve({})
    })
    await btn.trigger('click')
    await flushPromises()
    expect(w.emitted('done')).toBeTruthy()
  })
})
