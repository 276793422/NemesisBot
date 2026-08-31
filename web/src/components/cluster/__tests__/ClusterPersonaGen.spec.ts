import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

// D③（persona-gen 收尾）：persona_generate 返回的 coverage 完整性报告在
// 预览区展示——覆盖率/计数/完整徽标/整段硬缺口/缺失+疑点明细。
// 后端对账行为由 crates/nemesis-web/src/handlers/cluster_persona_gen（含
// tests.rs/s10b_tests.rs）+ cluster_deep_tests.rs 钉住；本 spec 只钉展示契约。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import ClusterPersonaGen from '../ClusterPersonaGen.vue'

const BASE_PKG = {
  node_name: 'alpha-01',
  display_name: 'Alpha',
  emoji: '🤖',
  role: 'worker',
  category: 'development',
  tags: ['rust'],
  identity_md: '# Alpha',
  soul_md: '# Soul',
}

const COVERAGE_GAP = {
  total: 10,
  covered: 7,
  skipped: 1,
  missing: 1,
  suspect: 1,
  coverage_rate: 0.7,
  entries: [
    { unit_id: 'u3', status: 'covered', location: 'identity_md' },
    { unit_id: 'u8', status: 'missing', reason: '未在产物中找到关键实体' },
    { unit_id: 'u9', status: 'suspect', reason: '仅在 SOUL 提及一次' },
  ],
  segment_gaps: ['工作经历（2019-2021）'],
}

const COVERAGE_OK = {
  ...COVERAGE_GAP,
  covered: 9,
  missing: 0,
  suspect: 0,
  coverage_rate: 0.9,
  entries: [{ unit_id: 'u3', status: 'covered', location: 'identity_md' }],
  segment_gaps: [],
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
})

async function mountGenerated(pkg: Record<string, unknown>) {
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'persona_generate') return Promise.resolve(pkg)
    return Promise.resolve({ applied: true, reloaded: true, display_name: pkg.display_name })
  })
  const w = mount(ClusterPersonaGen)
  await w.findAll('textarea')[0].setValue('JD 全文')
  const btn = w.findAll('button').find(b => b.text().includes('用此 JD 生成'))!
  await btn.trigger('click')
  await flushPromises()
  return w
}

describe('ClusterPersonaGen 完整性覆盖展示（D③）', () => {
  it('有缺口：渲染覆盖率/计数/⚠徽标/整段缺口/缺失疑点明细', async () => {
    const w = await mountGenerated({ ...BASE_PKG, coverage: COVERAGE_GAP })
    const panel = w.find('[data-testid="coverage-panel"]')
    expect(panel.exists()).toBe(true)
    expect(panel.text()).toContain('70%')
    expect(panel.text()).toContain('信息单元 10')
    expect(panel.text()).toContain('已覆盖 7')
    expect(panel.text()).toContain('缺失 1')
    expect(panel.text()).toContain('疑点 1')
    expect(panel.text()).toContain('⚠ 有缺口')
    // 问题条目只列 missing/suspect，不列 covered。
    expect(panel.text()).toContain('u8')
    expect(panel.text()).toContain('未在产物中找到关键实体')
    expect(panel.text()).toContain('u9')
    expect(panel.text()).not.toContain('u3')
    // 整段硬缺口。
    expect(panel.text()).toContain('整段未产出：工作经历（2019-2021）')
  })

  it('完整：✓徽标，无缺失疑点明细行', async () => {
    const w = await mountGenerated({ ...BASE_PKG, coverage: COVERAGE_OK })
    const panel = w.find('[data-testid="coverage-panel"]')
    expect(panel.exists()).toBe(true)
    expect(panel.text()).toContain('90%')
    expect(panel.text()).toContain('✓ 完整')
    expect(panel.text()).not.toContain('u8')
    expect(panel.text()).not.toContain('整段未产出')
  })

  it('无 coverage 字段（旧后端/未对账）：不渲染面板，其余预览正常', async () => {
    const w = await mountGenerated({ ...BASE_PKG })
    expect(w.find('[data-testid="coverage-panel"]').exists()).toBe(false)
    // 预览主体仍在（display_name 在 input value 里，不在 text() 中）。
    expect(w.text()).toContain('生成结果')
    const nameInput = w.findAll('input').find(i => (i.element as HTMLInputElement).value === 'Alpha')
    expect(nameInput).toBeDefined()
  })
})
