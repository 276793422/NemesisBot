import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// Forge 高级设置折叠区（批次 2 配置页补全）：config.forge.json side-file
// 8 键，读走 forge.config.get（defaults ⊕ raw 深合并）、写走 forge.config.set
// 整批一次保存（任一键非法整批拒绝，不落半批）。后端白名单面由 handlers/forge
// 测试钉住；本 spec 钉前端折叠交互与 config.set 载荷形态。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import ForgeView from '../../views/ForgeView.vue'

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string) => {
    // loadAll / loadConfig 等概览面兜底空形状（catch 静默），本 spec 只关心高级折叠。
    if (cmd === 'config.get') return Promise.resolve({ config: {}, defaults: true })
    if (cmd === 'config.set') return Promise.resolve({ saved: true })
    return Promise.resolve({})
  })
})

async function mountView() {
  const w = mount(ForgeView)
  await flushPromises()
  return w
}

async function openConfigTab(w: ReturnType<typeof mount>) {
  await w.findAll('button.tab').find(b => b.text() === '配置')!.trigger('click')
  await flushPromises()
}

describe('ForgeView 高级设置折叠（批次 2）', () => {
  it('折叠默认收起；点 forge-adv-toggle 展开 → 8 键控件渲染（缺省值兜底注记在场）', async () => {
    const w = await mountView()
    await openConfigTab(w)
    expect(w.find('[data-test="forge-adv-save"]').exists()).toBe(false) // v-if advOpen
    await w.find('[data-test="forge-adv-toggle"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-test="forge-adv-save"]').exists()).toBe(true)
    expect(w.text()).toContain('缺省值') // advDefaults=true 注记：config.forge.json 尚未生成
    expect(w.text()).toContain('评审模型')
    expect(w.text()).toContain('微优化 token 预算')
    w.unmount()
  })

  it('改 judge_threshold + budget_tokens → 保存 → forge.config.set 整批 8 键载荷', async () => {
    const w = await mountView()
    await openConfigTab(w)
    await w.find('[data-test="forge-adv-toggle"]').trigger('click')
    await flushPromises()
    const numInputs = w.findAll('input[type="number"]')
    // 折叠内数值输入顺序：judge_threshold / budget_tokens / max_experiences
    await numInputs[0].setValue('0.85')
    await numInputs[1].setValue('4000')
    await flushPromises()
    await w.find('[data-test="forge-adv-save"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('forge', 'config.set', {
      values: {
        replay_auto_ingest: false,
        'reflection.use_llm': false,
        'artifacts.auto_skill': false,
        'validation.auto_validate': false,
        'test.judge_model': null,
        'test.judge_threshold': 0.85,
        'optimize.budget_tokens': 4000,
        'collection.max_experiences': 500,
      },
    })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('高级设置已保存'))).toBe(true)
    w.unmount()
  })

  it('config.set 失败 → error toast（整批拒绝不落半批的失败面）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.get') return Promise.resolve({ config: {}, defaults: true })
      if (cmd === 'config.set') return Promise.reject(new Error('invalid judge_threshold'))
      return Promise.resolve({})
    })
    const w = await mountView()
    await openConfigTab(w)
    await w.find('[data-test="forge-adv-toggle"]').trigger('click')
    await flushPromises()
    await w.find('[data-test="forge-adv-save"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('保存失败'))).toBe(true)
    w.unmount()
  })
})
