import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// 2026-08-29：CommandsView CRUD 契约——加载回显、校验拦截（空名/重名/空
// 提示词）、整表保存、删除。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import CommandsView from '../CommandsView.vue'

const TABLE = [
  { name: 'review', description: '代码审查', argument_hint: '<路径>', prompt: '请审查 $ARGUMENTS' },
]

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'list') return Promise.resolve({ commands: TABLE.map(c => ({ ...c })), total: 1 })
    if (cmd === 'save') return Promise.resolve({ saved: true, total: 1 })
    return Promise.resolve({})
  })
})

async function mountView(tab: '概览' | '命令管理' = '概览') {
  const w = mount(CommandsView)
  await flushPromises()
  if (tab !== '概览') {
    await w.findAll('button.tab').find(b => b.text() === tab)!.trigger('click')
    await flushPromises()
  }
  return w
}

describe('CommandsView 命令管理', () => {
  it('加载回显命令表；保存走 commands.save 整表', async () => {
    const w = await mountView('命令管理')
    expect(requestMock).toHaveBeenCalledWith('commands', 'list')
    expect(w.text()).toContain('/review')
    expect(w.text()).toContain('代码审查')

    requestMock.mockClear()
    await w.findAll('button').find(b => b.text() === '保存全部')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find(c => c[1] === 'save')!
    expect(call[2].commands[0]).toMatchObject({ name: 'review', prompt: '请审查 $ARGUMENTS' })
  })

  it('空名称 / 重名 / 空提示词 → 校验拦截不发 save', async () => {
    const w = await mountView('命令管理')

    // 空名称（新增一条不填直接保存）
    requestMock.mockClear()
    await w.findAll('button').find(b => b.text() === '+ 添加命令')!.trigger('click')
    await w.findAll('button').find(b => b.text() === '保存全部')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'save').length).toBe(0)
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('名称不能为空'))).toBe(true)

    // 重名
    requestMock.mockClear()
    const nameInputs = w.findAll('input[placeholder*="不带 /"]')
    await nameInputs[1]!.setValue('review')
    const prompts = w.findAll('textarea')
    await prompts[1]!.setValue('p2')
    await w.findAll('button').find(b => b.text() === '保存全部')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('重复'))).toBe(true)

    // 空提示词（换一条独立的：改唯一名再清空提示词）
    requestMock.mockClear()
    await nameInputs[1]!.setValue('other')
    const textareas = w.findAll('textarea')
    await textareas[1]!.setValue('   ')
    await w.findAll('button').find(b => b.text() === '保存全部')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'save').length).toBe(0)
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('提示词不能为空'))).toBe(true)
  })

  it('删除条目 → 保存后从整表移除', async () => {
    const w = await mountView('命令管理')
    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'save') return Promise.resolve({ saved: true, total: 0 })
      return Promise.resolve({})
    })
    console.log('BEFORE-DEL:', JSON.stringify((w.vm as any).commands), 'count=', document.querySelectorAll('.page-commands').length)
    await w.findAll('button').find(b => b.text() === '删除')!.trigger('click')
    await w.findAll('button').find(b => b.text() === '保存全部')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find(c => c[1] === 'save')!
    expect(call[2].commands).toEqual([])
  })

  it('概览 TAB：说明 + 只读明细（名称/描述/提示/提示词）+ 共计数', async () => {
    const w = await mountView()
    expect(w.text()).toContain('快捷提示词发送器')
    expect(w.text()).toContain('共 1 条')
    expect(w.text()).toContain('/review')
    expect(w.text()).toContain('代码审查')
    expect(w.text()).toContain('<路径>')
    expect(w.text()).toContain('请审查 $ARGUMENTS')
    // 概览无编辑入口（保存/添加按钮不存在于概览 TAB）
    expect(w.findAll('button').find(b => b.text() === '保存全部')).toBeUndefined()
    expect(w.findAll('button').find(b => b.text() === '+ 添加命令')).toBeUndefined()
  })

  it('切 TAB 从磁盘刷新：详细页未保存的修改被丢弃（最后写入者胜）', async () => {
    const w = await mountView('命令管理')
    const nameInput = w.findAll('input[placeholder*="不带 /"]')[0]!
    await nameInput.setValue('本地未保存')
    await w.findAll('button.tab').find(b => b.text() === '概览')!.trigger('click')
    await flushPromises()
    await w.findAll('button.tab').find(b => b.text() === '命令管理')!.trigger('click')
    await flushPromises()
    const reloaded = w.findAll('input[placeholder*="不带 /"]')[0]!
    expect((reloaded.element as HTMLInputElement).value).toBe('review')
  })
})
