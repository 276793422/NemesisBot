import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// G5 (U18)：身份页 —— 六件套 tab（含 AGENTS/CLAUDE 指令链）、指令链徽标、
// 缺失文档不误报（exists=false 不打 get，保存即创建）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import IdentityView from '../IdentityView.vue'

function listResult() {
  return {
    documents: [
      { name: 'AGENT.md', exists: true, size: 20, instruction_chain: false },
      { name: 'IDENTITY.md', exists: true, size: 30, instruction_chain: false },
      { name: 'SOUL.md', exists: false, size: 0, instruction_chain: false },
      { name: 'USER.md', exists: false, size: 0, instruction_chain: false },
      { name: 'AGENTS.md', exists: false, size: 0, instruction_chain: true },
      { name: 'CLAUDE.md', exists: false, size: 0, instruction_chain: true },
    ],
  }
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
    if (cmd === 'list') return Promise.resolve(listResult())
    if (cmd === 'get') {
      return Promise.resolve({ name: data?.name, content: `# ${data?.name} 内容` })
    }
    if (cmd === 'save') return Promise.resolve({ saved: true })
    return Promise.resolve({})
  })
})

async function mountView() {
  const w = mount(IdentityView)
  await flushPromises()
  return w
}

describe('IdentityView 指令链（G5）', () => {
  it('list 返回六个文档 tab；指令链 tab 带 ⛓ 标记', async () => {
    const w = await mountView()
    const tabs = w.findAll('.tab')
    expect(tabs.length).toBe(6)
    expect(tabs.map(t => t.text().trim())).toEqual([
      '行为指南', '身份定义', '核心原则', '用户偏好', '指令链 AGENTS⛓', '指令链 CLAUDE⛓',
    ])
    const chainTabs = tabs.filter(t => t.find('.chain-dot').exists())
    expect(chainTabs.length).toBe(2)
    expect(chainTabs[0].text()).toContain('AGENTS')
    wrapper_clean(w)
  })

  it('选指令链文档 → 标题带「指令链 · 每轮注入」徽章；人格文档没有', async () => {
    const w = await mountView()
    // 默认选第一个（AGENT.md，人格件）—— 无链徽章
    expect(w.find('.chain-badge:not(.chain-badge--new)').exists()).toBe(false)
    // 点 AGENTS.md → 链徽章出现（该文件缺失，同时有「未创建」徽章）
    await w.findAll('.tab').find(t => t.text().includes('AGENTS'))!.trigger('click')
    await flushPromises()
    expect(w.find('.chain-badge:not(.chain-badge--new)').text()).toContain('指令链 · 每轮注入')
    expect(w.find('.chain-badge--new').exists()).toBe(true)
    wrapper_clean(w)
  })

  it('缺失文档不打 get（无读取失败 toast）；保存即创建并刷新列表', async () => {
    const w = await mountView()
    requestMock.mockClear()
    await w.findAll('.tab').find(t => t.text().includes('CLAUDE'))!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'get').length).toBe(0)
    expect(useToast().toasts.some(t => t.type === 'error')).toBe(false)

    // 编辑 → 保存 → save 调用 + 列表刷新（新文件 exists 标志更新）
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await w.find('textarea').setValue('# 规则')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('identity', 'save', {
      name: 'CLAUDE.md', content: '# 规则',
    })
    expect(requestMock.mock.calls.filter(c => c[1] === 'list').length).toBe(1)
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(true)
    wrapper_clean(w)
  })

  it('读取失败回退到内容所属文档；加载窗口内保存被拒（跨文档覆盖写防线）', async () => {
    const w = await mountView() // AGENT.md 已加载
    expect(w.vm.contentDoc).toBe('AGENT.md')

    // ① get IDENTITY.md 失败 → 回退到内容实际所属的 AGENT.md（不是悬在新标题下）
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'get' && data?.name === 'IDENTITY.md') return Promise.reject(new Error('ws down'))
      if (cmd === 'get') return Promise.resolve({ name: data?.name, content: `# ${data?.name} 内容` })
      return Promise.resolve(listResult())
    })
    await w.findAll('.tab').find(t => t.text().includes('身份定义'))!.trigger('click')
    await flushPromises()
    expect(w.vm.activeDoc).toBe('AGENT.md')
    expect(w.vm.docContent).toBe('# AGENT.md 内容')
    expect(useToast().toasts.some(t => t.type === 'error')).toBe(true)

    // ② 加载窗口内（get 未返回）编辑+保存 → save 不得发出（A 文档内容不能写进 B 文档）
    requestMock.mockClear()
    let resolveGet!: (v: any) => void
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'get') {
        return new Promise(resolve => { resolveGet = resolve })
      }
      return Promise.resolve({})
    })
    await w.findAll('.tab').find(t => t.text().includes('身份定义'))!.trigger('click')
    await flushPromises()
    expect(w.vm.activeDoc).toBe('IDENTITY.md')
    expect(w.vm.contentDoc).toBe('AGENT.md') // 内容还没换成 IDENTITY 的
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await w.find('textarea').setValue('# 误操作内容')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'save').length).toBe(0)
    expect(useToast().toasts.some(t => t.type === 'error')).toBe(true)

    // ③ get 返回后窗口关闭 → 内容与文档一致，保存恢复放行
    resolveGet!({ name: 'IDENTITY.md', content: '# IDENTITY.md 内容' })
    await flushPromises()
    expect(w.vm.contentDoc).toBe('IDENTITY.md')
    expect(w.vm.docContent).toBe('# IDENTITY.md 内容')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'save').length).toBe(1)
    wrapper_clean(w)
  })
})

function wrapper_clean(w: { unmount: () => void }) {
  w.unmount()
}
