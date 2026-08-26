import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// M6 补测（quality-hardening goal 2026-08-25）：P3-2 模型页 ——
// 目录缓存态/更新、属性编辑（draft 种子、dirty 判定、逐字段保存、
// v1 不写 null 的清空跳过、effort off 归一）。
// 后端 raw-JSON RMW 语义由 handlers/models/tests.rs 钉住。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import ModelsView from '../ModelsView.vue'

const MODEL = {
  model_name: 'main',
  model: 'zhipu/glm-4.7',
  is_default: true,
  model_tier: null,
  reasoning_effort: null,
  model_size_b: 30,
  real_name: null,
  context_window: null,
  catalog_match: { context_window: 128000, family: 'glm' },
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
})

function listResult(models: unknown[] = [MODEL]) {
  return { models }
}

async function mountView(models: unknown[] = [MODEL], catalog = { exists: true, fetched_at: '2026-08-24', entries: 123 }) {
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'list') return Promise.resolve(listResult(models))
    if (cmd === 'catalog_info') return Promise.resolve(catalog)
    return Promise.resolve({})
  })
  const w = mount(ModelsView)
  await flushPromises()
  return w
}

describe('ModelsView 目录缓存', () => {
  it('显示 catalog_info 的条数/时间；不存在时提示未缓存', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('models', 'catalog_info')
    expect(w.text()).toContain('目录缓存：123 条')
    expect(w.text()).toContain('2026-08-24')

    const w2 = await mountView([MODEL], { exists: false, fetched_at: '', entries: 0 })
    expect(w2.text()).not.toContain('目录缓存：123')
  })

  it('更新目录：成功 toast + 重拉列表；失败 toast；busy 期不重复触发', async () => {
    let release!: (v: unknown) => void
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(listResult())
      if (cmd === 'catalog_info') return Promise.resolve({ exists: false, fetched_at: '', entries: 0 })
      if (cmd === 'catalog_update') return new Promise(r => (release = r))
      return Promise.resolve({})
    })
    const w = mount(ModelsView)
    await flushPromises()

    const btn = w.findAll('button').find(b => b.text().includes('更新模型目录'))!
    await btn.trigger('click')
    await btn.trigger('click') // busy 守卫：不二次发
    expect(requestMock.mock.calls.filter(c => c[1] === 'catalog_update').length).toBe(1)
    expect(btn.attributes('disabled')).toBeDefined()

    release({ exists: true, fetched_at: 'now', entries: 456 })
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('456'))).toBe(true)
    // catalog_match 可能变了 → 重拉 list
    expect(requestMock.mock.calls.filter(c => c[1] === 'list').length).toBe(2)
    expect(btn.attributes('disabled')).toBeUndefined()
  })
})

describe('ModelsView 属性编辑', () => {
  it('展开属性 → draft 按当前值播种；无修改保存 → info 不发写', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '属性')!.trigger('click')
    await flushPromises()

    const editor = w.find('.attr-editor')
    expect(editor.exists()).toBe(true)
    expect((editor.findAll('select')[0].element as HTMLSelectElement).value).toBe('auto')
    expect((editor.find('input[type="number"]').element as HTMLInputElement).value).toBe('30')
    // catalog_match 提供目录值
    expect(editor.text()).toContain('128,000')

    await w.findAll('button').find(b => b.text() === '保存属性')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'update_field').length).toBe(0)
    expect(useToast().toasts.some(t => t.message.includes('没有修改过的属性'))).toBe(true)
  })

  it('改 tier → 只保存脏字段，值与 toast 如实', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '属性')!.trigger('click')
    const editor = w.find('.attr-editor')
    await editor.findAll('select')[0].setValue('mini')
    await w.findAll('button').find(b => b.text() === '保存属性')!.trigger('click')
    await flushPromises()

    const writes = requestMock.mock.calls.filter(c => c[1] === 'update_field')
    expect(writes.length).toBe(1)
    expect(writes[0][2]).toEqual({ name: 'main', field: 'model_tier', value: 'mini' })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('model_tier'))).toBe(true)
  })

  it('effort 从 off 改为 low → 归一发送；size 清空 → v1 跳过不写 null', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '属性')!.trigger('click')
    const editor = w.find('.attr-editor')
    await editor.findAll('select')[1].setValue('low') // effort ''→low（脏）
    await editor.find('input[type="number"]').setValue('') // 30→''（脏，但落 null=跳过）
    await w.findAll('button').find(b => b.text() === '保存属性')!.trigger('click')
    await flushPromises()

    const writes = requestMock.mock.calls.filter(c => c[1] === 'update_field').map(c => c[2])
    expect(writes.length).toBe(1)
    expect(writes[0]).toEqual({ name: 'main', field: 'reasoning_effort', value: 'low' })
    expect(useToast().toasts.some(t => t.message.includes('未保存') && t.message.includes('model_size_b'))).toBe(true)
  })

  it('单字段保存失败 → 错误 toast + 中止后续字段 + 重拉列表', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(listResult())
      if (cmd === 'catalog_info') return Promise.resolve({ exists: false, fetched_at: '', entries: 0 })
      if (cmd === 'update_field') return Promise.reject(new Error('config 被锁'))
      return Promise.resolve({})
    })
    const w = mount(ModelsView)
    await flushPromises()
    await w.findAll('button').find(b => b.text() === '属性')!.trigger('click')
    const editor = w.find('.attr-editor')
    await editor.findAll('select')[0].setValue('big')
    await editor.find('input[placeholder="如 Qwen3-30B"]').setValue('GLM-4.7')
    await w.findAll('button').find(b => b.text() === '保存属性')!.trigger('click')
    await flushPromises()

    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('config 被锁'))).toBe(true)
    // 第一个字段失败即中止：只发了一次 update_field
    expect(requestMock.mock.calls.filter(c => c[1] === 'update_field').length).toBe(1)
    // 失败后重拉列表回显真实盘上状态
    expect(requestMock.mock.calls.filter(c => c[1] === 'list').length).toBe(2)
  })

  it('目录值一键填入 context_window', async () => {
    const w = await mountView()
    await w.findAll('button').find(b => b.text() === '属性')!.trigger('click')
    const fill = w.findAll('a').find(a => a.text() === '填入')!
    await fill.trigger('click')
    const ctxInput = w.find('.attr-editor').findAll('input').at(-1)! as ReturnType<typeof w.find>
    expect((ctxInput.element as HTMLInputElement).value).toBe('128000')
  })
})
