// P10（能力扩展 WS5，2026-09-25）：模型管理页家族分组/筛选/折叠的组件级测试。
// 后端家族真相源 = nemesis-config PROVIDER_PRESETS（P9）；本文件只钉前端
// 分组渲染与筛选交互（卡片明细逻辑由既有页面行为覆盖，不在本测试范围）。
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

const requestMock = vi.fn()
// 注意：本 spec 位于 src/views/__tests__/，src 只隔两层（与既有 view specs 一致）。
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import ModelsView from '../ModelsView.vue'

function modelRow(over: Record<string, unknown> = {}) {
  return {
    model_name: 'alias',
    model: 'openai/gpt-4o',
    api_key: 'sk-***',
    api_base: 'https://api.openai.com/v1',
    is_default: false,
    protocol: null,
    ...over,
  }
}

function mockBackend(models: Record<string, unknown>[]) {
  requestMock.mockImplementation(async (module: string, cmd: string) => {
    if (module === 'models' && cmd === 'list') return { models }
    if (module === 'models' && cmd === 'catalog_info') {
      return { exists: false, fetched_at: '', entries: 0 }
    }
    if (module === 'models' && cmd === 'health') return { models: [], days: 0, note: '' }
    throw new Error(`unexpected request ${module}.${cmd}`)
  })
}

async function mountView(models: Record<string, unknown>[]) {
  mockBackend(models)
  const w = mount(ModelsView)
  await flushPromises()
  return w
}

beforeEach(() => {
  requestMock.mockReset()
})

describe('ModelsView 家族分组（P10）', () => {
  it('列表按家族分组渲染组头（标签 + 条数）', async () => {
    const w = await mountView([
      modelRow({ model_name: 'glm', model: 'zhipu/glm-4.7' }),
      modelRow({ model_name: 'gpt', model: 'openai/gpt-4o', is_default: true }),
      modelRow({ model_name: 'ds', model: 'deepseek/deepseek-chat' }),
    ])
    const headers = w.findAll('.family-header')
    expect(headers).toHaveLength(3)
    const labels = headers.map((h) => h.find('.family-name').text())
    // 表顺序：openai 在前，zhipu/deepseek 按表序。
    expect(labels).toEqual(['OpenAI', '智谱 AI', 'DeepSeek'])
    const counts = headers.map((h) => h.find('.family-count').text())
    expect(counts).toEqual(['1', '1', '1'])
    w.unmount()
  })

  it('同家族多模型聚合进同组；未知家族进「其他」且置底', async () => {
    const w = await mountView([
      modelRow({ model_name: 'a', model: 'deepseek/deepseek-chat' }),
      modelRow({ model_name: 'b', model: 'deepseek/deepseek-reasoner' }),
      modelRow({ model_name: 'c', model: 'mystery-model-9000' }),
    ])
    const headers = w.findAll('.family-header')
    expect(headers.map((h) => h.find('.family-name').text())).toEqual(['DeepSeek', '其他'])
    const grids = w.findAll('.family-grid')
    expect(grids[0].findAll('.model-card')).toHaveLength(2)
    expect(grids[1].findAll('.model-card')).toHaveLength(1)
    w.unmount()
  })

  it('家族筛选下拉：选家族后只显示该组卡片，选回全部恢复', async () => {
    const w = await mountView([
      modelRow({ model_name: 'glm', model: 'zhipu/glm-4.7' }),
      modelRow({ model_name: 'gpt', model: 'openai/gpt-4o' }),
    ])
    const select = w.find('select.family-filter')
    expect(select.exists()).toBe(true)
    // 全部家族（''）：两组都在。
    expect(w.findAll('.family-group')).toHaveLength(2)
    // 选 zhipu：只剩智谱组。
    await select.setValue('zhipu')
    const groups = w.findAll('.family-group')
    expect(groups).toHaveLength(1)
    expect(groups[0].find('.family-name').text()).toBe('智谱 AI')
    expect(groups[0].findAll('.model-card')).toHaveLength(1)
    // 选回 ''：恢复全部。
    await select.setValue('')
    expect(w.findAll('.family-group')).toHaveLength(2)
    w.unmount()
  })

  it('组头点击折叠/展开（v-show 收起组内网格）', async () => {
    const w = await mountView([modelRow({ model_name: 'gpt', model: 'openai/gpt-4o' })])
    const header = w.find('.family-header')
    const grid = w.find('.family-grid')
    expect((grid.element as HTMLElement).style.display).not.toBe('none')
    await header.trigger('click')
    expect((grid.element as HTMLElement).style.display).toBe('none')
    await header.trigger('click')
    expect((grid.element as HTMLElement).style.display).not.toBe('none')
    w.unmount()
  })

  it('单模型场景照样出组头；筛选下拉存在', async () => {
    const w = await mountView([modelRow({ model_name: 'glm', model: 'zhipu/glm-4.7' })])
    expect(w.findAll('.family-header')).toHaveLength(1)
    expect(w.find('select.family-filter').exists()).toBe(true)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 职能框架 M4/M6：images-openai 图像协议条目的展示与守卫
// （协议下拉新增项；卡片 🖼 徽标；「设为默认」对图像条目禁用——后端
// set_default 本身也拒绝，前端禁用只是防呆）。
describe('ModelsView 图像协议条目（职能框架 M6）', () => {
  it('图像条目：🖼 徽标 + 协议文案 + 设为默认禁用带提示', async () => {
    const w = await mountView([
      modelRow({ model_name: 'dalle', model: 'zhipu/dall-e-3', protocol: 'images-openai' }),
      modelRow({ model_name: 'glm', model: 'zhipu/glm-4.7' }),
    ])
    const cards = w.findAll('.model-card')
    expect(cards).toHaveLength(2)
    const img = cards[0]
    expect(img.text()).toContain('🖼 图像')
    expect(img.text()).toContain('图像生成（generate_image 专用）')
    const btn = img.findAll('button').find((b) => b.text().includes('设为默认'))!
    expect(btn.attributes('disabled')).toBeDefined()
    expect(btn.attributes('title')).toContain('generate_image')
    // 普通条目不受影响。
    const plain = cards[1]
    expect(plain.text()).not.toContain('🖼 图像')
    const plainBtn = plain.findAll('button').find((b) => b.text().includes('设为默认'))!
    expect(plainBtn.attributes('disabled')).toBeUndefined()
    w.unmount()
  })

  it('新建表单协议下拉含 images-openai 项', async () => {
    const w = await mountView([modelRow({ model_name: 'glm', model: 'zhipu/glm-4.7' })])
    // 打开新建表单。
    const addBtn = w.findAll('button').find((b) => b.text().includes('添加模型'))
    if (addBtn) await addBtn.trigger('click')
    await flushPromises()
    const options = w.findAll('select option').map((o) => (o.element as HTMLOptionElement).value)
    expect(options).toContain('images-openai')
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// per-model 流式开关（2026-10-13 stream 键）：add 表单与属性编辑器的三档
// 下拉（''=auto / yes / no），值集与后端 normalize_stream_value 一致；
// 属性编辑器 seed 自条目 stream 值并带构造期生效标注。
describe('ModelsView 流式开关（stream 键 UI）', () => {
  it('新建表单流式开关下拉含三档，默认 auto', async () => {
    const w = await mountView([modelRow({ model_name: 'glm', model: 'zhipu/glm-4.7' })])
    const addBtn = w.findAll('button').find((b) => b.text().includes('添加模型'))
    if (addBtn) await addBtn.trigger('click')
    await flushPromises()
    const group = w.findAll('.form-group').find((g) => g.find('.form-label').text() === '流式开关')
    expect(group).toBeDefined()
    const sel = group!.find('select')
    expect(sel.exists()).toBe(true)
    const values = sel.findAll('option').map((o) => (o.element as HTMLOptionElement).value)
    expect(values).toEqual(['', 'yes', 'no'])
    expect((sel.element as HTMLSelectElement).value).toBe('')
    w.unmount()
  })

  it('属性编辑器流式开关下拉：seed 自条目 stream 值 + 生效标注', async () => {
    const w = await mountView([
      modelRow({ model_name: 'glm', model: 'zhipu/glm-4.7', stream: 'yes' }),
    ])
    const attrBtn = w.findAll('button').find((b) => b.text() === '属性')
    expect(attrBtn).toBeDefined()
    await attrBtn!.trigger('click')
    await flushPromises()
    const editor = w.find('.attr-editor')
    expect(editor.exists()).toBe(true)
    const field = editor
      .findAll('.attr-field')
      .find((f) => f.find('.form-label').text() === '流式开关')
    expect(field).toBeDefined()
    const sel = field!.find('select')
    const values = sel.findAll('option').map((o) => (o.element as HTMLOptionElement).value)
    expect(values).toEqual(['', 'yes', 'no'])
    expect((sel.element as HTMLSelectElement).value).toBe('yes')
    expect(field!.find('.attr-effect').text()).toContain('改默认模型立即生效（自动热切）')
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// per-model 绝对压缩点（2026-10-20 compact_context_tokens）：属性编辑器
// 输入行 seed / dirty 检出 / 保存调用（空串清除）/ 超窗口黄字提醒。
describe('ModelsView 上下文压缩点（compact_context_tokens UI）', () => {
  function findCcField(w: ReturnType<typeof mount>) {
    return w
      .find('.attr-editor')
      .findAll('.attr-field')
      .find((f) => f.find('.form-label').text().includes('上下文压缩点'))
  }

  async function openAttrs(w: ReturnType<typeof mount>) {
    const attrBtn = w.findAll('button').find((b) => b.text() === '属性')!
    await attrBtn.trigger('click')
    await flushPromises()
  }

  it('属性编辑器含压缩点输入行：seed 自条目值 + 即时生效标注', async () => {
    const w = await mountView([
      modelRow({ model_name: 'glm', model: 'zhipu/glm-4.7', compact_context_tokens: 200000 }),
    ])
    await openAttrs(w)
    const field = findCcField(w)
    expect(field).toBeDefined()
    const input = field!.find('input')
    expect((input.element as HTMLInputElement).value).toBe('200000')
    expect(field!.find('.attr-effect').text()).toContain('即时生效')
    expect(field!.find('.attr-clamp-warn').exists()).toBe(false)
    w.unmount()
  })

  it('dirty 检出 + 保存发 update_field；清空发空串（后端移键清除）', async () => {
    const updateCalls: any[] = []
    requestMock.mockImplementation(async (module: string, cmd: string, data?: any) => {
      if (module === 'models' && cmd === 'list') {
        return {
          models: [modelRow({ model_name: 'glm', model: 'zhipu/glm-4.7', compact_context_tokens: 200000 })],
        }
      }
      if (module === 'models' && cmd === 'catalog_info') return { exists: false, fetched_at: '', entries: 0 }
      if (module === 'models' && cmd === 'health') return { models: [], days: 0, note: '' }
      if (module === 'models' && cmd === 'update_field') {
        updateCalls.push(data)
        return { updated: true }
      }
      throw new Error(`unexpected request ${module}.${cmd}`)
    })
    const w = mount(ModelsView)
    await flushPromises()
    await openAttrs(w)

    // 改值 → dirty 显示 → 保存。
    const field = findCcField(w)!
    await field.find('input').setValue('100000')
    expect(w.find('.attr-dirty').text()).toContain('compact_context_tokens')
    const saveBtn = w.findAll('button').find((b) => b.text() === '保存属性')!
    await saveBtn.trigger('click')
    await flushPromises()
    expect(updateCalls).toHaveLength(1)
    expect(updateCalls[0]).toMatchObject({ name: 'glm', field: 'compact_context_tokens', value: '100000' })

    // 清空再保存 → 发空串（后端 0/空串 = 移键清除）。
    await findCcField(w)!.find('input').setValue('')
    await saveBtn.trigger('click')
    await flushPromises()
    expect(updateCalls).toHaveLength(2)
    expect(updateCalls[1]).toMatchObject({ field: 'compact_context_tokens', value: '' })
    w.unmount()
  })

  it('输入超过条目窗口 → 黄字钳制提醒；未超不提醒', async () => {
    const w = await mountView([
      modelRow({
        model_name: 'glm',
        model: 'zhipu/glm-4.7',
        context_window: 131072,
        compact_context_tokens: 100000,
      }),
    ])
    await openAttrs(w)
    const field = findCcField(w)!
    expect(field.find('.attr-clamp-warn').exists()).toBe(false)

    await field.find('input').setValue('500000')
    await flushPromises()
    const warn = findCcField(w)!.find('.attr-clamp-warn')
    expect(warn.exists()).toBe(true)
    expect(warn.text()).toContain('500,000')
    expect(warn.text()).toContain('钳到窗口')
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 五项增强批（2026-10-27）：①真测试三态染点 ②复制 ③改名 ④列表模式
// ⑤一键测试全部（并发 3）。圆点是组件内存态（不落盘）；视图偏好 localStorage。
function baseMock(testImpl?: (data: any) => any) {
  requestMock.mockImplementation(async (module: string, cmd: string, data?: any) => {
    if (module === 'models' && cmd === 'list') return { models: [modelRow({ model_name: 'glm', model: 'zhipu/glm-4.7' })] }
    if (module === 'models' && cmd === 'catalog_info') return { exists: false, fetched_at: '', entries: 0 }
    if (module === 'models' && cmd === 'health') return { models: [], days: 0, note: '' }
    if (module === 'models' && cmd === 'test') return testImpl ? testImpl(data) : { name: 'glm', status: 'ok', latency_ms: 123, lane: 'openai-compat', detail: '' }
    throw new Error(`unexpected request ${module}.${cmd}`)
  })
}

describe('ModelsView 圆点状态灯（五项增强批⑤）', () => {
  it('初始灰点（idle）→ 测试通过染绿 + tooltip 带延迟', async () => {
    baseMock()
    const w = mount(ModelsView)
    await flushPromises()
    const dot = w.find('.model-card .test-dot')
    expect(dot.classes()).toContain('dot--idle')
    const testBtn = w.findAll('button').find((b) => b.text() === '测试')!
    await testBtn.trigger('click')
    await flushPromises()
    expect(dot.classes()).toContain('dot--ok')
    expect(dot.attributes('title')).toContain('123ms')
    w.unmount()
  })

  it('失败染红 + tooltip 带 kind 与 detail', async () => {
    baseMock(() => ({ name: 'glm', status: 'fail', kind: 'auth', detail: '401 未授权', lane: 'openai-compat' }))
    const w = mount(ModelsView)
    await flushPromises()
    const dot = w.find('.model-card .test-dot')
    const testBtn = w.findAll('button').find((b) => b.text() === '测试')!
    await testBtn.trigger('click')
    await flushPromises()
    expect(dot.classes()).toContain('dot--fail')
    expect(dot.attributes('title')).toContain('auth')
    expect(dot.attributes('title')).toContain('401')
    w.unmount()
  })

  it('图像 lane unsupported 保持灰（dot--unsupported）+ tooltip 诚实说明', async () => {
    baseMock(() => ({ name: 'glm', status: 'unsupported', detail: '图像生成条目不能对话测试（仅供 generate_image 工具消费）', lane: 'images-openai' }))
    const w = mount(ModelsView)
    await flushPromises()
    const dot = w.find('.model-card .test-dot')
    const testBtn = w.findAll('button').find((b) => b.text() === '测试')!
    await testBtn.trigger('click')
    await flushPromises()
    expect(dot.classes()).toContain('dot--unsupported')
    expect(dot.attributes('title')).toContain('generate_image')
    w.unmount()
  })
})

describe('ModelsView 一键测试全部（五项增强批⑤）', () => {
  it('对全部模型发测试请求，完成后全部染绿', async () => {
    const tested: string[] = []
    requestMock.mockImplementation(async (module: string, cmd: string, data?: any) => {
      if (module === 'models' && cmd === 'list') {
        return {
          models: [
            modelRow({ model_name: 'a', model: 'openai/gpt-4o' }),
            modelRow({ model_name: 'b', model: 'zhipu/glm-4.7' }),
            modelRow({ model_name: 'c', model: 'deepseek/deepseek-chat' }),
          ],
        }
      }
      if (module === 'models' && cmd === 'catalog_info') return { exists: false, fetched_at: '', entries: 0 }
      if (module === 'models' && cmd === 'health') return { models: [], days: 0, note: '' }
      if (module === 'models' && cmd === 'test') {
        tested.push(data.name)
        return { name: data.name, status: 'ok', latency_ms: 10, lane: 'openai-compat', detail: '' }
      }
      throw new Error(`unexpected request ${module}.${cmd}`)
    })
    const w = mount(ModelsView)
    await flushPromises()
    const allBtn = w.findAll('button').find((b) => b.text().includes('一键测试'))!
    await allBtn.trigger('click')
    await flushPromises()
    expect(tested.sort()).toEqual(['a', 'b', 'c'])
    const dots = w.findAll('.model-card .test-dot')
    expect(dots).toHaveLength(3)
    for (const d of dots) expect(d.classes()).toContain('dot--ok')
    w.unmount()
  })
})

describe('ModelsView 视图切换（五项增强批④）', () => {
  it('默认卡片视图 → 切列表出全景表（含圆点/延迟/操作列）+ localStorage 持久', async () => {
    localStorage.removeItem('models.viewMode')
    requestMock.mockImplementation(async (module: string, cmd: string) => {
      if (module === 'models' && cmd === 'list') {
        return { models: [modelRow({ model_name: 'a', model: 'openai/gpt-4o' }), modelRow({ model_name: 'b', model: 'zhipu/glm-4.7' })] }
      }
      if (module === 'models' && cmd === 'catalog_info') return { exists: false, fetched_at: '', entries: 0 }
      if (module === 'models' && cmd === 'health') return { models: [], days: 0, note: '' }
      throw new Error(`unexpected request ${module}.${cmd}`)
    })
    const w = mount(ModelsView)
    await flushPromises()
    expect(w.find('.list-table').exists()).toBe(false)
    expect(w.find('.family-grid').exists()).toBe(true)
    // 切列表。
    const listBtn = w.findAll('.view-toggle button').find((b) => b.text() === '列表')!
    await listBtn.trigger('click')
    expect(w.find('.list-table').exists()).toBe(true)
    const rows = w.findAll('.list-table tbody tr')
    expect(rows).toHaveLength(2)
    // 圆点在列表里；延迟列初始 --；操作列有 复制/改名。
    expect(rows[0].find('.test-dot').exists()).toBe(true)
    expect(rows[0].text()).toContain('--')
    const ops = rows[0].findAll('button').map((b) => b.text())
    expect(ops).toContain('复制')
    expect(ops).toContain('改名')
    expect(localStorage.getItem('models.viewMode')).toBe('list')
    // 切回卡片。
    const cardBtn = w.findAll('.view-toggle button').find((b) => b.text() === '卡片')!
    await cardBtn.trigger('click')
    expect(w.find('.family-grid').exists()).toBe(true)
    w.unmount()
    localStorage.removeItem('models.viewMode')
  })
})

describe('ModelsView 复制 / 改名（五项增强批②③）', () => {
  async function clickCardBtn(w: ReturnType<typeof mount>, label: string) {
    const btn = w.findAll('.model-card button').find((b) => b.text() === label)!
    await btn.trigger('click')
    await flushPromises()
  }

  it('复制弹窗预填 -copy 与源模型 ID；只改名时发 models.copy {name,new_name}', async () => {
    const copyCalls: any[] = []
    baseMock()
    requestMock.mockImplementation(async (module: string, cmd: string, data?: any) => {
      if (module === 'models' && cmd === 'copy') {
        copyCalls.push(data)
        return { copied: true, name: data.name, new_name: data.new_name }
      }
      if (module === 'models' && cmd === 'list') return { models: [modelRow({ model_name: 'glm', model: 'zhipu/glm-4.7' })] }
      if (module === 'models' && cmd === 'catalog_info') return { exists: false, fetched_at: '', entries: 0 }
      if (module === 'models' && cmd === 'health') return { models: [], days: 0, note: '' }
      throw new Error(`unexpected request ${module}.${cmd}`)
    })
    const w = mount(ModelsView)
    await flushPromises()
    await clickCardBtn(w, '复制')
    const modal = w.find('.modal-backdrop')
    expect(modal.exists()).toBe(true)
    const inputs = modal.findAll('input')
    expect(inputs).toHaveLength(2)
    // 名称预填 -copy；模型 ID 预填源条目的 model 串（可直接换）。
    expect((inputs[0].element as HTMLInputElement).value).toBe('glm-copy')
    expect((inputs[1].element as HTMLInputElement).value).toBe('zhipu/glm-4.7')
    await inputs[0].setValue('glm2')
    await modal.find('.modal-footer .btn-primary').trigger('click')
    await flushPromises()
    // 模型 ID 未改 → 不发 new_model（等值不落盘）。
    expect(copyCalls).toEqual([{ name: 'glm', new_name: 'glm2' }])
    w.unmount()
  })

  it('复制弹窗改模型 ID → 发 new_model（复制即换 LLM）', async () => {
    const copyCalls: any[] = []
    requestMock.mockImplementation(async (module: string, cmd: string, data?: any) => {
      if (module === 'models' && cmd === 'copy') {
        copyCalls.push(data)
        return { copied: true }
      }
      if (module === 'models' && cmd === 'list') return { models: [modelRow({ model_name: 'glm', model: 'zhipu/glm-4.7' })] }
      if (module === 'models' && cmd === 'catalog_info') return { exists: false, fetched_at: '', entries: 0 }
      if (module === 'models' && cmd === 'health') return { models: [], days: 0, note: '' }
      throw new Error(`unexpected request ${module}.${cmd}`)
    })
    const w = mount(ModelsView)
    await flushPromises()
    await clickCardBtn(w, '复制')
    const modal = w.find('.modal-backdrop')
    await modal.findAll('input')[1].setValue('zhipu/glm-4.8')
    await modal.find('.modal-footer .btn-primary').trigger('click')
    await flushPromises()
    expect(copyCalls).toEqual([{ name: 'glm', new_name: 'glm-copy', new_model: 'zhipu/glm-4.8' }])
    w.unmount()
  })

  it('改名弹窗预填当前名与模型 ID；改名+换模型都发（new_model 随行）', async () => {
    const renameCalls: any[] = []
    requestMock.mockImplementation(async (module: string, cmd: string, data?: any) => {
      if (module === 'models' && cmd === 'rename') {
        renameCalls.push(data)
        return { renamed: true }
      }
      if (module === 'models' && cmd === 'list') return { models: [modelRow({ model_name: 'glm', model: 'zhipu/glm-4.7' })] }
      if (module === 'models' && cmd === 'catalog_info') return { exists: false, fetched_at: '', entries: 0 }
      if (module === 'models' && cmd === 'health') return { models: [], days: 0, note: '' }
      throw new Error(`unexpected request ${module}.${cmd}`)
    })
    const w = mount(ModelsView)
    await flushPromises()
    await clickCardBtn(w, '改名')
    const modal = w.find('.modal-backdrop')
    expect(modal.exists()).toBe(true)
    const inputs = modal.findAll('input')
    expect(inputs).toHaveLength(2)
    expect((inputs[0].element as HTMLInputElement).value).toBe('glm')
    expect((inputs[1].element as HTMLInputElement).value).toBe('zhipu/glm-4.7')
    // 只换模型 ID、名称不动（2026-10-27 漏项回归锁：换模型的正门）。
    await inputs[1].setValue('zhipu/glm-4.8')
    await modal.find('.modal-footer .btn-primary').trigger('click')
    await flushPromises()
    expect(renameCalls).toEqual([{ name: 'glm', new_name: 'glm', new_model: 'zhipu/glm-4.8' }])
    w.unmount()
  })
})

describe('ModelsView 明文 Key 一步迁移（2026-10-27）', () => {
  /** 顶栏迁移卡按钮按文本定位（横幅不在 .model-card 内）。 */
  function bannerBtn(w: ReturnType<typeof mount>, label: string) {
    return w.findAll('.key-import-card button').find((b) => b.text().includes(label))!
  }
  /** 既有 mockBackend + migrate_keys / vault.status 通路。 */
  function migrateMock(opts: {
    inlineFirst?: boolean
    migrateResult?: Record<string, unknown> | Error
    vaultStatus?: Record<string, unknown> | null
  }) {
    let listCount = 0
    const migrateCalls: any[] = []
    let statusCalls = 0
    requestMock.mockImplementation(async (module: string, cmd: string, data?: any) => {
      if (module === 'models' && cmd === 'list') {
        listCount++
        const rows =
          opts.inlineFirst !== false && listCount === 1
            ? [modelRow({ model_name: 'a', key_source: { kind: 'inline' } })]
            : [modelRow({ model_name: 'a', key_source: { kind: 'vault' } })]
        return { models: rows }
      }
      if (module === 'models' && cmd === 'catalog_info') return { exists: false, fetched_at: '', entries: 0 }
      if (module === 'models' && cmd === 'health') return { models: [], days: 0, note: '' }
      if (module === 'vault' && cmd === 'status') {
        statusCalls++
        if (opts.vaultStatus === null) throw new Error('vault 未装配')
        return opts.vaultStatus ?? { exists: false, mode: 'dpapi', unlocked: true }
      }
      if (module === 'models' && cmd === 'migrate_keys') {
        migrateCalls.push(data)
        const r = opts.migrateResult ?? {
          target: 'yaml',
          migrated: [{ name: 'a', alias: 'a' }],
          reused: 0,
          conflicts: [],
        }
        if (r instanceof Error) throw r
        return r
      }
      throw new Error(`unexpected request ${module}.${cmd}`)
    })
    return { listCount: () => listCount, migrateCalls, statusCalls: () => statusCalls }
  }

  it('明文 Key 模型在场时渲染迁移卡（两按钮）；无明文时不渲染', async () => {
    const m = migrateMock({})
    const w = mount(ModelsView)
    await flushPromises()
    const card = w.find('.key-import-card')
    expect(card.exists()).toBe(true)
    expect(card.text()).toContain('1 个模型使用明文 Key')
    expect(bannerBtn(w, 'credentials.yaml').text()).toContain('迁移到 credentials.yaml')
    expect(bannerBtn(w, '迁移到加密 vault').exists()).toBe(true)
    w.unmount()

    requestMock.mockReset()
    migrateMock({ inlineFirst: false })
    const w2 = mount(ModelsView)
    await flushPromises()
    expect(w2.find('.key-import-card').exists()).toBe(false)
    expect(m.statusCalls()).toBe(0)
    w2.unmount()
  })

  it('点「迁移到 credentials.yaml」发 {target:yaml}；成功后重拉列表、banner 消失', async () => {
    const m = migrateMock({})
    const w = mount(ModelsView)
    await flushPromises()
    expect(w.find('.key-import-card').exists()).toBe(true)
    await bannerBtn(w, 'credentials.yaml').trigger('click')
    await flushPromises()
    expect(m.migrateCalls).toEqual([{ target: 'yaml' }])
    expect(m.listCount(), '成功后 loadModels 重拉列表').toBe(2)
    expect(w.find('.key-import-card').exists(), '迁移完成 banner 消失').toBe(false)
    w.unmount()
  })

  it('vault 为 argon2 锁定时先弹口令行，确认后带 passphrase 发迁移', async () => {
    const m = migrateMock({ vaultStatus: { exists: true, mode: 'argon2id', unlocked: false } })
    const w = mount(ModelsView)
    await flushPromises()
    await bannerBtn(w, '迁移到加密 vault').trigger('click')
    await flushPromises()
    expect(m.statusCalls()).toBe(1)
    expect(m.migrateCalls, '锁定态不直接发迁移').toEqual([])
    const pw = w.find('.vault-pw-input')
    expect(pw.exists()).toBe(true)
    await pw.setValue('op-w')
    await bannerBtn(w, '解锁并迁移').trigger('click')
    await flushPromises()
    expect(m.migrateCalls).toEqual([{ target: 'vault', passphrase: 'op-w' }])
    expect(w.find('.vault-pw-input').exists(), '完成后口令行收起').toBe(false)
    w.unmount()
  })

  it('vault 已解锁（DPAPI 等）直接迁移，不带 passphrase、不弹口令行', async () => {
    const m = migrateMock({ vaultStatus: { exists: true, mode: 'dpapi', unlocked: true } })
    const w = mount(ModelsView)
    await flushPromises()
    await bannerBtn(w, '迁移到加密 vault').trigger('click')
    await flushPromises()
    expect(m.migrateCalls).toEqual([{ target: 'vault' }])
    expect(w.find('.vault-pw-input').exists()).toBe(false)
    w.unmount()
  })

  it('迁移失败诚实报错：banner 留守、不重拉列表', async () => {
    const m = migrateMock({ migrateResult: new Error('解锁失败') })
    const w = mount(ModelsView)
    await flushPromises()
    await bannerBtn(w, 'credentials.yaml').trigger('click')
    await flushPromises()
    expect(m.listCount(), '失败不重拉').toBe(1)
    expect(w.find('.key-import-card').exists(), 'banner 留守').toBe(true)
    w.unmount()
  })

  it('口令行「取消」收起且不发迁移', async () => {
    const m = migrateMock({ vaultStatus: { exists: true, mode: 'argon2id', unlocked: false } })
    const w = mount(ModelsView)
    await flushPromises()
    await bannerBtn(w, '迁移到加密 vault').trigger('click')
    await flushPromises()
    await bannerBtn(w, '取消').trigger('click')
    await flushPromises()
    expect(w.find('.vault-pw-input').exists()).toBe(false)
    expect(m.migrateCalls).toEqual([])
    w.unmount()
  })
})
