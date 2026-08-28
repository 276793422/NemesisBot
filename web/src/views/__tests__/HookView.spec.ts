import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// P4 hooks.json 编辑器（2026-08-29 重构为双 TAB：总览 + 结构化设置）——
// get 回显（含 invalid 形态）、总览原文编辑保存（语法自检/语义拒绝）、
// 结构化扁平条目序列化（每条各自成组）、切 TAB 从磁盘刷新（最后写入者胜）、
// 一键重启。后端 hooks handler 行为由 handlers/hooks/tests.rs（7 测试）钉住。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import HookView from '../HookView.vue'

const VALID_HOOKS = JSON.stringify({
  hooks: {
    PreToolUse: [
      { matcher: 'Edit|Write', hooks: [{ type: 'command', command: 'python lint.py', timeout: 30 }] },
    ],
    Stop: [
      { hooks: [{ type: 'command', command: 'echo done' }] },
    ],
  },
}, null, 2)

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  vi.useFakeTimers()
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'get') {
      return Promise.resolve({ content: VALID_HOOKS, exists: true, valid: true, summary: { total: 2, PreToolUse: 1, Stop: 1 } })
    }
    if (cmd === 'set') return Promise.resolve({ summary: { total: 2 } })
    return Promise.resolve({})
  })
})

afterEach(() => {
  vi.useRealTimers()
})

async function mountView(tab: '总览' | '设置' = '总览') {
  const w = mount(HookView)
  await flushPromises()
  if (tab !== '总览') {
    await w.findAll('button.tab').find(b => b.text() === tab)!.trigger('click')
    await flushPromises()
  }
  return w
}

describe('HookView 总览', () => {
  it('统计徽标 + 只读明细 + 可编辑原文', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('hooks', 'get')
    expect(w.text()).toContain('共 2 个脚本')
    expect(w.text()).toContain('python lint.py')
    expect(w.text()).toContain('全部工具')
    const ta = w.find('textarea')
    expect((ta.element as HTMLTextAreaElement).value).toBe(VALID_HOOKS)
  })

  it('invalid 形态：横幅渲染错误说明', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') return Promise.resolve({ content: '{ not json', exists: true, valid: false, error: '第 1 行: 期待对象键' })
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(w.text()).toContain('期待对象键')
    expect(w.text()).toContain('fail-open')
  })

  it('原文保存：语法错误 → 不发 set；合法 → set + 成功 toast', async () => {
    const w = await mountView()
    const ta = w.find('textarea')

    await ta.setValue('{ broken')
    await w.findAll('button').find(b => b.text() === '保存原文')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'set').length).toBe(0)
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('JSON 语法错误'))).toBe(true)

    await ta.setValue(VALID_HOOKS)
    await w.findAll('button').find(b => b.text() === '保存原文')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('hooks', 'set', { content: VALID_HOOKS })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('2 个脚本'))).toBe(true)
  })

  it('后端语义拒绝 → 错误 toast（文件未写入语义）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') return Promise.resolve({ content: VALID_HOOKS, exists: true, valid: true })
      if (cmd === 'set') return Promise.reject(new Error('未知 hook 事件名 foo'))
      return Promise.resolve({})
    })
    const w = await mountView()
    await w.find('textarea').setValue(VALID_HOOKS)
    await w.findAll('button').find(b => b.text() === '保存原文')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('未写入') && t.message.includes('foo'))).toBe(true)
  })
})

describe('HookView 结构化设置页（方案 B 扁平条目）', () => {
  it('设置 TAB 渲染 5 个事件区块；既有条目回显 matcher/命令/超时', async () => {
    const w = await mountView('设置')
    for (const label of ['PreToolUse', 'PostToolUse', 'SessionStart', 'UserPromptSubmit', 'Stop']) {
      expect(w.text()).toContain(label)
    }
    // PreToolUse 的既有条目（matcher=Edit|Write，命令 lint）
    const matcherInput = w.findAll('input').find(i => (i.element as HTMLInputElement).value === 'Edit|Write')
    expect(matcherInput).toBeTruthy()
    const cmdInput = w.findAll('input').find(i => (i.element as HTMLInputElement).value === 'python lint.py')
    expect(cmdInput).toBeTruthy()
    // Stop 事件无 matcher → matcher 输入为空
    const emptyMatchers = w.findAll('input').filter(i => (i.element as HTMLInputElement).value === '')
    expect(emptyMatchers.length).toBeGreaterThan(0)
  })

  it('添加条目 → 保存全部：扁平条目序列化为每条各自成组的 JSON', async () => {
    const w = await mountView('设置')
    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'set') return Promise.resolve({ summary: { total: 2 } })
      return Promise.resolve({})
    })

    // PreToolUse 区块添加一条新钩子（无 matcher，带超时 45）
    const preBlock = w.findAll('.card').find(c => c.text().includes('PreToolUse'))!
    await preBlock.findAll('button').find(b => b.text() === '+ 添加钩子')!.trigger('click')
    const newMatcher = preBlock.findAll('input[placeholder="触发工具（空=全部）"]').at(-1)!
    const newCmd = preBlock.findAll('input[placeholder^="命令"]').at(-1)!
    const newTimeout = preBlock.findAll('input[placeholder="60"]').at(-1)!
    await newMatcher.setValue('Bash')
    await newCmd.setValue('echo hi')
    await newTimeout.setValue('45')

    await w.findAll('button').find(b => b.text() === '保存全部钩子')!.trigger('click')
    await flushPromises()

    const setCall = requestMock.mock.calls.find(c => c[1] === 'set')!
    expect(setCall).toBeTruthy()
    const written = JSON.parse(setCall[2].content)
    // 扁平序列化：新增条目独立成组（不与既有 matcher 合并）
    const preGroups = written.hooks.PreToolUse
    expect(preGroups.length).toBe(2)
    expect(preGroups[0]).toEqual({
      matcher: 'Edit|Write',
      hooks: [{ type: 'command', command: 'python lint.py', timeout: 30 }],
    })
    expect(preGroups[1]).toEqual({
      matcher: 'Bash',
      hooks: [{ type: 'command', command: 'echo hi', timeout: 45 }],
    })
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(true)
  })

  it('删除条目 → 保存后该条不写入', async () => {
    const w = await mountView('设置')
    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'set') return Promise.resolve({ summary: { total: 1 } })
      return Promise.resolve({})
    })
    // 删除 PreToolUse 的唯一条目（✕ 按钮）
    const preBlock = w.findAll('.card').find(c => c.text().includes('PreToolUse'))!
    await preBlock.findAll('button').find(b => b.text() === '✕')!.trigger('click')
    await w.findAll('button').find(b => b.text() === '保存全部钩子')!.trigger('click')
    await flushPromises()
    const written = JSON.parse(requestMock.mock.calls.find(c => c[1] === 'set')![2].content)
    expect(written.hooks.PreToolUse).toBeUndefined()
    expect(written.hooks.Stop).toBeTruthy()
  })

  it('切换 TAB 从磁盘刷新：未保存的原文修改被丢弃（最后写入者胜）', async () => {
    const w = await mountView()
    // 在总览改原文但不保存
    await w.find('textarea').setValue('{ 本地未保存修改 }')
    // 切到设置再切回总览 → 触发磁盘重载，本地修改被磁盘内容覆盖
    await w.findAll('button.tab').find(b => b.text() === '设置')!.trigger('click')
    await flushPromises()
    await w.findAll('button.tab').find(b => b.text() === '总览')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'get').length).toBeGreaterThanOrEqual(2)
    expect((w.find('textarea').element as HTMLTextAreaElement).value).toBe(VALID_HOOKS)
  })
})

describe('HookView 重启', () => {
  it('一键重启：stop → start', async () => {
    const w = await mountView()
    requestMock.mockClear()
    requestMock.mockResolvedValue({})
    await w.findAll('button').find(b => b.text().includes('重启 Agent 生效'))!.trigger('click')
    await vi.advanceTimersByTimeAsync(1500)
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[0] === 'agent').map(c => c[1])).toEqual(['stop', 'start'])
  })
})
