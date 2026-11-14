import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// 专家Agent 管理页契约（2026-11-13 专家子代理批次②）：
// - experts.list 加载回显（默认卡置顶 + 专家卡栅格）；
// - toggle → experts.set_enabled；编辑先 get 拉 prompt 全文再 update；
// - 删除两段确认（第一击 arm，第二击发 experts.remove）；
// - 默认卡仅可改名（update key=_default 只带 name）；
// - 空名册 empty-state 引导人格超市。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn() }),
}))

import ExpertsView from '../ExpertsView.vue'

const WRITER = {
  slug: 'writer',
  name: 'Writer',
  summary: '写文档的专家',
  tags: ['docs'],
  model: 'glm-5.3-flash',
  enabled: true,
  source: 'manual',
  hired_from: '',
}

const DISABLED_GHOST = {
  slug: 'ghost',
  name: 'Ghost',
  summary: '',
  tags: [],
  model: '',
  enabled: false,
  source: 'shop',
  hired_from: 'persona-42',
}

function defaultListImpl() {
  // 模式策略默认态（config.json 未配置 experts 段 = 模式关/降级开/通知关）。
  const policyState = { enabled: false, degrade: true, notify: false }
  return (_m: string, cmd: string, data?: any) => {
    if (cmd === 'list') {
      return Promise.resolve({ experts: [WRITER, DISABLED_GHOST], default_name: '默认子代理' })
    }
    if (cmd === 'get') {
      return Promise.resolve({ ...WRITER, prompt: 'You write docs.' })
    }
    if (cmd === 'update' && data?.key === '_default') {
      return Promise.resolve({ key: '_default', name: data.name, is_default: true })
    }
    if (cmd === 'config_get') {
      return Promise.resolve({ ...policyState })
    }
    if (cmd === 'config_set') {
      Object.assign(policyState, data)
      return Promise.resolve({ ...policyState })
    }
    return Promise.resolve({})
  }
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation(defaultListImpl())
})

async function mountView() {
  const w = mount(ExpertsView)
  await flushPromises()
  return w
}

describe('ExpertsView 专家名册', () => {
  it('加载回显：默认卡置顶 + 专家卡（来源/模型/停用透明度）', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('experts', 'list')
    // 默认卡
    expect(w.text()).toContain('默认子代理')
    expect(w.text()).toContain('出厂常驻')
    // 专家卡回显
    expect(w.text()).toContain('Writer')
    expect(w.text()).toContain('写文档的专家')
    expect(w.text()).toContain('glm-5.3-flash')
    expect(w.text()).toContain('超市雇佣')
    expect(w.text()).toContain('手工创建')
    // 两个专家卡 + 一个默认卡
    expect(w.findAll('.skill-card').length).toBe(3)
  })

  it('toggle → experts.set_enabled 双向布尔', async () => {
    const w = await mountView()
    requestMock.mockClear()
    // 专家卡开关（策略条三开关不在 .skill-card 内，天然隔离）。
    const toggles = w.findAll('.skill-card .toggle')
    expect(toggles.length).toBe(2)
    // Writer（启用）→ 点击停用
    await toggles[0]!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('experts', 'set_enabled', { key: 'writer', enabled: false })
    // Ghost（停用）→ 点击启用
    await toggles[1]!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('experts', 'set_enabled', { key: 'ghost', enabled: true })
  })

  it('编辑先 get 拉 prompt 全文，保存走 update 带 key', async () => {
    const w = await mountView()
    requestMock.mockClear()
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('experts', 'get', { key: 'writer' })
    const promptArea = w.findAll('textarea')[0]!
    expect((promptArea.element as HTMLTextAreaElement).value).toBe('You write docs.')

    // 改简介保存
    await w.findAll('input.form-input')[1]!.setValue('改过的简介')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find(c => c[1] === 'update')!
    expect(call[2].key).toBe('writer')
    expect(call[2].summary).toBe('改过的简介')
    expect(call[2].prompt).toBe('You write docs.')
  })

  it('删除两段确认：第一击 arm 不发请求，第二击发 remove', async () => {
    const w = await mountView()
    requestMock.mockClear()
    const delBtn = w.findAll('button').find(b => b.text() === '删除')!
    await delBtn.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'remove').length).toBe(0)
    expect(w.text()).toContain('确定解雇？')

    await w.findAll('button').find(b => b.text() === '确认')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('experts', 'remove', { key: 'writer' })
  })

  it('默认卡改名：update key=_default 只带 name', async () => {
    const w = await mountView()
    requestMock.mockClear()
    await w.findAll('button').find(b => b.text() === '✏ 改名')!.trigger('click')
    await w.findAll('input.form-input')[0]!.setValue('总管')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find(c => c[1] === 'update')!
    expect(call[2].key).toBe('_default')
    expect(call[2].name).toBe('总管')
    expect(call[2].summary).toBeUndefined()
    expect(call[2].prompt).toBeUndefined()
    expect(w.text()).toContain('总管')
  })

  it('空名册：empty-state 引导人格超市，无专家卡栅格', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve({ experts: [], default_name: '默认子代理' })
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(w.text()).toContain('暂无雇佣专家')
    expect(w.text()).toContain('前往人格超市')
    // 仅默认卡一张
    expect(w.findAll('.skill-card').length).toBe(1)
  })

  it('新建：无 slug 时走 create', async () => {
    const w = await mountView()
    requestMock.mockClear()
    await w.findAll('button').find(b => b.text()!.includes('新建专家'))!.trigger('click')
    await w.findAll('input.form-input')[0]!.setValue('reviewer')
    await w.findAll('textarea')[0]!.setValue('You review.')
    await w.findAll('button').find(b => b.text() === '创建')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find(c => c[1] === 'create')!
    expect(call[2]).toMatchObject({ name: 'reviewer', prompt: 'You review.' })
  })

  // BUG-20261114-001 回归锁×3：编辑 prompt 生命周期 + toggle 重入。
  it('编辑 get 失败：保存不带 prompt 字段（后端保留现值，不清空存量）', async () => {
    const w = await mountView()
    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') return Promise.reject(new Error('IO 瞬时失败'))
      return Promise.resolve({})
    })
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.at(-1)?.type).toBe('error')
    // 诚实提示在位：textarea 空但保存不动存量提示词
    expect(w.text()).toContain('本次保存不会改动现有提示词')
    await w.findAll('input.form-input')[1]!.setValue('只改简介')
    await w.findAll('button').find(b => b.text() === '保存')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find(c => c[1] === 'update')!
    expect(call[2].key).toBe('writer')
    expect(call[2].summary).toBe('只改简介')
    expect('prompt' in call[2]).toBe(false)
  })

  it('编辑 prompt 过期响应围栏：重开后旧 get 晚到不覆盖新内容', async () => {
    const w = await mountView()
    requestMock.mockClear()
    const resolvers: Array<(v: any) => void> = []
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') return new Promise(r => resolvers.push(r))
      return Promise.resolve({})
    })
    // 第一击编辑（get#1 挂起）→ 关闭 → 重开（get#2 挂起）
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    await w.findAll('button.modal-close')[0]!.trigger('click')
    await w.findAll('button').find(b => b.text() === '编辑')!.trigger('click')
    expect(resolvers.length).toBe(2)
    // get#2（新代序）先回
    resolvers[1]!({ ...WRITER, prompt: 'FRESH' })
    await flushPromises()
    const area = w.findAll('textarea')[0]!
    expect((area.element as HTMLTextAreaElement).value).toBe('FRESH')
    // get#1（过期代序）晚到：不得写进共享 form
    resolvers[0]!({ ...WRITER, prompt: 'STALE' })
    await flushPromises()
    expect((area.element as HTMLTextAreaElement).value).toBe('FRESH')
  })

  it('toggle 双击重入围栏：在飞期间第二击不发请求', async () => {
    const w = await mountView()
    requestMock.mockClear()
    let resolveToggle!: (v: any) => void
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'set_enabled') return new Promise(r => { resolveToggle = r })
      return Promise.resolve({})
    })
    const t = w.findAll('.skill-card .toggle')[0]!
    await t.trigger('click')
    await t.trigger('click')
    expect(requestMock.mock.calls.filter(c => c[1] === 'set_enabled').length).toBe(1)
    resolveToggle({ ...WRITER, enabled: false })
    await flushPromises()
    expect(requestMock.mock.calls.filter(c => c[1] === 'set_enabled').length).toBe(1)
  })
})

describe('ExpertsView 模式策略条（2026-11-13 总开关 + 自动降级）', () => {
  it('挂载拉 config_get 回显三开关；默认态降级开/模式关', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('experts', 'config_get')
    expect(w.find('[data-test="policy-enabled"]').classes()).not.toContain('active')
    expect(w.find('[data-test="policy-degrade"]').classes()).toContain('active')
    expect(w.find('[data-test="policy-notify"]').classes()).not.toContain('active')
    // 模式关 → 降级/通知条目半透明（inert 视觉，不遮点击）。
    const inert = w.findAll('.expert-policy-item.is-inert')
    expect(inert.length).toBe(2)
  })

  it('总开关点击 → config_set {enabled:true} + opt-in toast + inert 解除', async () => {
    const w = await mountView()
    requestMock.mockClear()
    await w.find('[data-test="policy-enabled"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('experts', 'config_set', { enabled: true })
    const toast = useToast().toasts.at(-1)
    expect(toast?.type).toBe('success')
    expect(toast?.message).toContain('后台记录')
    // 模式开 → inert 消失。
    expect(w.findAll('.expert-policy-item.is-inert').length).toBe(0)
    expect(w.find('[data-test="policy-enabled"]').classes()).toContain('active')
  })

  it('降级开关点击 → config_set {degrade:false}，独立于总开关', async () => {
    const w = await mountView()
    requestMock.mockClear()
    await w.find('[data-test="policy-degrade"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('experts', 'config_set', { degrade: false })
    expect(requestMock.mock.calls.filter(c => c[1] === 'config_set').length).toBe(1)
  })

  it('config_set 失败 → error toast，开关不翻转', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve({ experts: [], default_name: '默认子代理' })
      if (cmd === 'config_get') return Promise.resolve({ enabled: false, degrade: true, notify: false })
      if (cmd === 'config_set') return Promise.reject(new Error('策略面未装配'))
      return Promise.resolve({})
    })
    const w = await mountView()
    requestMock.mockClear()
    await w.find('[data-test="policy-enabled"]').trigger('click')
    await flushPromises()
    const toast = useToast().toasts.at(-1)
    expect(toast?.type).toBe('error')
    expect(w.find('[data-test="policy-enabled"]').classes()).not.toContain('active')
  })
})
