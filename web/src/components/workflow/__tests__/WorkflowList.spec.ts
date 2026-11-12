import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))
vi.mock('../../../composables/useSSE', () => ({
  on: vi.fn(),
  off: vi.fn(),
}))

import WorkflowList from '../WorkflowList.vue'
import { useWorkflowStore } from '../../../stores/workflow'

const WF_A = {
  name: 'daily-report',
  description: '每日报告',
  version: 3,
  node_count: 5,
  triggers: [
    { trigger_type: 'cron', driven: true, next_fire_at: '2026-11-12 09:00' },
    { trigger_type: 'event', driven: false, reason: '无监听者' },
  ],
  chat_index: 'abcdef12',
  has_chat_password: false,
}
const WF_B = {
  name: 'one-shot',
  description: '',
  version: 1,
  node_count: 2,
  triggers: [{ trigger_type: 'manual', driven: true, next_fire_at: null }],
  chat_index: 'ffff0000',
  has_chat_password: true,
}

function defaultImpl(_m: string, cmd: string) {
  switch (cmd) {
    case 'list': return Promise.resolve({ workflows: [WF_A, WF_B], trigger_driver_status: { cron: { trigger_type: 'cron', driven: true }, event: { trigger_type: 'event', driven: false, reason: '无监听者' } } })
    default: return Promise.resolve({})
  }
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockImplementation(defaultImpl)
})

afterEach(() => {
  vi.restoreAllMocks()
})

async function mountList() {
  const w = mount(WorkflowList)
  await flushPromises()
  return w
}

describe('WorkflowList 列表渲染', () => {
  it('挂载时列表为空则拉取并渲染行', async () => {
    const w = await mountList()
    expect(requestMock).toHaveBeenCalledWith('workflow', 'list')
    expect(w.text()).toContain('daily-report')
    expect(w.text()).toContain('每日报告')
    expect(w.text()).toContain('2026-11-12 09:00')
  })

  it('已有数据时挂载不重复拉取（5s 缓存）', async () => {
    const store = useWorkflowStore()
    store.workflows = [WF_A] as any
    await mountList()
    expect(requestMock.mock.calls.filter(c => c[1] === 'list').length).toBe(0)
  })

  it('空列表与错误态文案', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'list' ? Promise.resolve({ workflows: [] }) : Promise.resolve({}))
    const w = await mountList()
    expect(w.text()).toContain('暂无工作流')

    requestMock.mockImplementation((_m, cmd) => cmd === 'list' ? Promise.reject('boom') : Promise.resolve({}))
    const store = useWorkflowStore()
    store.clearListCache()
    await store.fetchList(true)
    // 字符串错误原样透出，非字符串统一兜底文案
    expect(store.listError).toBe('boom')
  })

  it('未驱动触发器横幅与徽章 warn 样式、driverStatus 区块', async () => {
    const w = await mountList()
    expect(w.text()).toContain('部分触发器未驱动')
    expect(w.find('.badge-warn').exists()).toBe(true)
    expect(w.find('.badge-ok').exists()).toBe(true)
    expect(w.text()).toContain('触发器驱动状态')
    expect(w.text()).toContain('无监听者')
  })

  it('全部触发已驱动时不显示横幅；无下次触发显示 —', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'list'
      ? Promise.resolve({ workflows: [WF_B], trigger_driver_status: {} })
      : Promise.resolve({}))
    const w = await mountList()
    expect(w.find('.warn-banner').exists()).toBe(false)
    expect(w.text()).toContain('—')
  })
})

describe('WorkflowList 工具栏动作', () => {
  it('刷新：清缓存并强制拉取', async () => {
    const w = await mountList()
    const calls = requestMock.mock.calls.filter(c => c[1] === 'list').length
    await w.findAll('button').find(b => b.text().includes('刷新'))!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'list').length).toBe(calls + 1)
  })

  it('新建/编辑/历史：切页签并按需加载', async () => {
    const w = await mountList()
    const store = useWorkflowStore()
    await w.findAll('button').find(b => b.text().includes('新建工作流'))!.trigger('click')
    expect(store.activeTab).toBe('canvas')
    expect(store.editingIsNew).toBe(true)

    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    expect(store.activeTab).toBe('canvas')

    await w.findAll('button').find(b => b.text() === '历史')!.trigger('click')
    expect(store.activeTab).toBe('history')
    expect(requestMock).toHaveBeenCalledWith('workflow', 'list_executions', { workflow_name: 'daily-report' })
  })

  it('聊天按钮在新标签页打开 path 路由', async () => {
    const openSpy = vi.spyOn(window, 'open').mockImplementation(() => null)
    const w = await mountList()
    await w.findAll('button').find(b => b.attributes('title') === '在新标签页中测试聊天此工作流')!.trigger('click')
    expect(openSpy).toHaveBeenCalledWith('/workflow/chat/abcdef12', '_blank')
    // 无 chat_index 的工作流不渲染聊天按钮（只有 daily-report 有）
    const chatBtns = w.findAll('button').filter(b => b.attributes('title') === '在新标签页中测试聊天此工作流')
    expect(chatBtns.length).toBe(2)
  })
})

describe('WorkflowList 删除确认弹窗', () => {
  async function openDelete(w: ReturnType<typeof mount>) {
    await w.findAll('button').find(b => b.attributes('title') === '删除此工作流')!.trigger('click')
    await flushPromises()
  }

  it('打开/取消/遮罩关闭', async () => {
    const w = await mountList()
    await openDelete(w)
    expect(w.text()).toContain('删除工作流')
    expect(w.text()).toContain('daily-report')
    await w.findAll('.pwd-actions button').find(b => b.text() === '取消')!.trigger('click')
    expect(w.find('.pwd-modal').exists()).toBe(false)
    await openDelete(w)
    await w.find('.pwd-modal-overlay').trigger('click')
    expect(w.find('.pwd-modal').exists()).toBe(false)
  })

  it('删除成功关窗并从列表移除', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'delete' ? Promise.resolve({ name: 'daily-report', deleted: true }) : defaultImpl(_m, cmd))
    const w = await mountList()
    await openDelete(w)
    await w.findAll('.pwd-actions button').find(b => b.text() === '删除')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('workflow', 'delete', { name: 'daily-report' })
    expect(w.find('.pwd-modal').exists()).toBe(false)
    expect(w.text()).not.toContain('daily-report')
  })

  it('删除业务失败在弹窗内显示错误', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'delete' ? Promise.reject(new Error('仍在运行')) : defaultImpl(_m, cmd))
    const w = await mountList()
    await openDelete(w)
    await w.findAll('.pwd-actions button').find(b => b.text() === '删除')!.trigger('click')
    await flushPromises()
    expect(w.find('.pwd-modal').exists()).toBe(true)
    expect(w.find('.pwd-error').text()).toContain('仍在运行')
  })
})

describe('WorkflowList 聊天密码弹窗', () => {
  async function openPwd(w: ReturnType<typeof mount>, title = '设置聊天密码') {
    await w.findAll('button').find(b => b.attributes('title') === title)!.trigger('click')
    await flushPromises()
  }

  it('无密码工作流默认 set 模式；空密码与不一致校验', async () => {
    const w = await mountList()
    await openPwd(w)
    expect(w.text()).toContain('设置聊天密码')
    const modal = () => w.find('.pwd-modal')
    // 空密码
    await modal().findAll('button').find(b => b.text() === '保存')!.trigger('click')
    expect(modal().find('.pwd-error').text()).toBe('密码不能为空')
    // 不一致
    await modal().findAll('input[type="password"]')[0].setValue('abc')
    await modal().findAll('input[type="password"]')[1].setValue('abd')
    await modal().findAll('button').find(b => b.text() === '保存')!.trigger('click')
    expect(modal().find('.pwd-error').text()).toBe('两次输入的密码不一致')
    expect(requestMock.mock.calls.filter(c => c[1] === 'set_chat_password').length).toBe(0)
  })

  it('设置密码成功：调用接口并刷新列表关窗', async () => {
    const w = await mountList()
    await openPwd(w)
    const modal = () => w.find('.pwd-modal')
    await modal().findAll('input[type="password"]')[0].setValue('pw1')
    await modal().findAll('input[type="password"]')[1].setValue('pw1')
    const calls = requestMock.mock.calls.filter(c => c[1] === 'list').length
    await modal().findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('workflow', 'set_chat_password', { index: 'abcdef12', password: 'pw1' })
    expect(requestMock.mock.calls.filter(c => c[1] === 'list').length).toBe(calls + 1)
    expect(modal().exists()).toBe(false)
  })

  it('有密码工作流默认 clear 模式并可切换重设', async () => {
    const w = await mountList()
    await openPwd(w, '修改/清除聊天密码')
    expect(w.text()).toContain('清除聊天密码')
    expect(w.text()).toContain('任何人通过 URL 都可访问')
    await w.findAll('.pwd-actions button').find(b => b.text() === '切换为重设')!.trigger('click')
    expect(w.text()).toContain('新密码')
    await w.findAll('.pwd-actions button').find(b => b.text() === '切换为清除')!.trigger('click')
    await w.findAll('.pwd-actions button').find(b => b.text() === '清除')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('workflow', 'clear_chat_password', { index: 'ffff0000' })
  })

  it('清除失败在弹窗内显示错误', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'clear_chat_password' ? Promise.reject(new Error('busy')) : defaultImpl(_m, cmd))
    const w = await mountList()
    await openPwd(w, '修改/清除聊天密码')
    await w.findAll('.pwd-actions button').find(b => b.text() === '清除')!.trigger('click')
    await flushPromises()
    expect(w.find('.pwd-error').text()).toBe('busy')
  })
})
