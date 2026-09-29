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
    const options = w.findAll('select option').map((o) => o.element.value)
    expect(options).toContain('images-openai')
    w.unmount()
  })
})
