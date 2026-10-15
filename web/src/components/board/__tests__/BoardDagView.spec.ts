import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// BoardDagView 全景流程图冒烟（2026-10-15 需求②③）：数据加载 → SVG 渲染
// 链路。布局算法本体在 dagLayout.spec.ts，这里只钉组件接线：
// 项目下拉 / 节点+边渲染 / external 小签 / 环横幅 / 状态点击高亮 / 节点点击开详情。

const handlers = new Map<string, ((data?: unknown) => void)[]>()

vi.mock('../../../composables/useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    const arr = handlers.get(type) || []
    arr.push(handler)
    handlers.set(type, arr)
  }),
  off: vi.fn(),
}))

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import BoardDagView from '../BoardDagView.vue'
import IssueDetailModal from '../IssueDetailModal.vue'

const PROJECT = { id: 1, name: '城市天际线', status: 'in_progress', created_at: 0 }

function iss(id: number, over: Record<string, unknown> = {}) {
  return {
    id,
    number: `NB-${id}`,
    title: `任务${id}`,
    status: 'todo',
    priority: 1,
    assignee: null,
    assignee_id: null,
    creator: { kind: 'user', id: 'test' },
    origin: null,
    project_id: 1,
    updated_at: 0,
    depends_on: [],
    ...over,
  }
}

beforeEach(() => {
  handlers.clear()
  requestMock.mockReset()
  requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
    if (cmd === 'project.list') return Promise.resolve({ projects: [PROJECT] })
    if (cmd === 'issue.list') return Promise.resolve({ issues: [], total: 0 })
    if (cmd === 'issue.get') return Promise.resolve({ issue: iss(2) })
    if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
    return Promise.resolve({})
  })
})

async function mountWith(issues: any[]) {
  requestMock.mockImplementation((_m: string, cmd: string, d: any) => {
    if (cmd === 'project.list') return Promise.resolve({ projects: [PROJECT] })
    if (cmd === 'issue.list') return Promise.resolve({ issues, total: issues.length })
    if (cmd === 'issue.get') return Promise.resolve({ issue: issues.find((i) => i.id === d?.id) ?? iss(2) })
    if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
    return Promise.resolve({})
  })
  const w = mount(BoardDagView)
  await flushPromises()
  return w
}

describe('BoardDagView 全景流程图（需求②③）', () => {
  it('项目下拉默认选中首项；链式依赖渲染 2 节点 + 1 边 + 统计/图例', async () => {
    const w = await mountWith([
      iss(1, { status: 'done' }),
      iss(2, { depends_on: [{ id: 1, number: 'NB-1', status: 'done' }] }),
    ])
    const select = w.find('select')
    expect((select.element as HTMLSelectElement).value).toBe('1')
    expect(w.findAll('g.dag-node').length).toBe(2)
    expect(w.findAll('.dag-edges path:not([stroke-dasharray])').length).toBe(1)
    expect(w.find('.dag-stats').exists()).toBe(true)
    expect(w.find('.dag-legend').exists()).toBe(true)
    expect(w.text()).toContain('2 节点')
    w.unmount()
  })

  it('external 依赖（上游不在视图）→ 左置小签显示编号，不静默丢', async () => {
    const w = await mountWith([iss(5, { depends_on: [{ id: 99, number: 'NB-99', status: 'todo' }] })])
    const svgText = w.find('.dag-svg').text()
    expect(svgText).toContain('NB-99')
    w.unmount()
  })

  it('环依赖 → 红色横幅提示 unlayeredCount', async () => {
    const w = await mountWith([
      iss(1, { depends_on: [{ id: 2, number: 'NB-2', status: 'todo' }] }),
      iss(2, { depends_on: [{ id: 1, number: 'NB-1', status: 'todo' }] }),
    ])
    expect(w.find('.dag-banner-error').exists()).toBe(true)
    expect(w.find('.dag-banner-error').text()).toContain('循环依赖')
    w.unmount()
  })

  it('状态统计 chip 点击 → 高亮该状态（其余节点压暗 opacity），再点取消', async () => {
    const w = await mountWith([
      iss(1, { status: 'done' }),
      iss(2, { status: 'todo' }),
    ])
    const chips = w.findAll('.dag-stat-chip')
    const doneChip = chips.find((c) => c.text().includes('已完成'))!
    await doneChip.trigger('click')
    await flushPromises()
    expect(doneChip.classes()).toContain('active')
    const nodes = w.findAll('g.dag-node')
    const opacities = nodes.map((n) => n.attributes('opacity'))
    expect(opacities).toContain('1') // done 节点保持
    expect(opacities).toContain('0.12') // todo 节点压暗
    await doneChip.trigger('click')
    await flushPromises()
    // 取消高亮后全部节点恢复 opacity=1（模板恒渲染 opacity 属性）。
    expect(w.findAll('g.dag-node').map((n) => n.attributes('opacity'))).toEqual(['1', '1'])
    w.unmount()
  })

  it('节点点击 → 打开 IssueDetailModal；关闭回调收起', async () => {
    const w = await mountWith([iss(2)])
    expect(w.findComponent(IssueDetailModal).exists()).toBe(false)
    await w.find('g.dag-node').trigger('click')
    await flushPromises()
    expect(w.findComponent(IssueDetailModal).exists()).toBe(true)
    w.findComponent(IssueDetailModal).vm.$emit('close')
    await flushPromises()
    expect(w.findComponent(IssueDetailModal).exists()).toBe(false)
    w.unmount()
  })

  it('悬停节点 → 其直接依赖边高亮（0.95）、其余压暗（0.08），移开恢复', async () => {
    const w = await mountWith([
      iss(1, { status: 'done' }),
      iss(2, { depends_on: [{ id: 1, number: 'NB-1', status: 'done' }] }),
      iss(3, { depends_on: [{ id: 2, number: 'NB-2', status: 'done' }] }),
    ])
    const edgePaths = () =>
      w.findAll('.dag-edges path').filter((p) => !p.attributes('stroke-dasharray'))
    expect(edgePaths().length).toBe(2)
    expect(edgePaths()[0].attributes('opacity')).toBe('0.55')
    await w.findAll('g.dag-node')[0].trigger('mouseenter')
    await flushPromises()
    expect(edgePaths()[0].attributes('opacity')).toBe('0.95') // 1→2 高亮
    expect(edgePaths()[1].attributes('opacity')).toBe('0.08') // 2→3 压暗
    await w.findAll('g.dag-node')[0].trigger('mouseleave')
    await flushPromises()
    expect(edgePaths()[0].attributes('opacity')).toBe('0.55')
    w.unmount()
  })

  it('空项目 → 诚实空态（不渲染画布）', async () => {
    const w = await mountWith([])
    expect(w.find('.dag-svg').exists()).toBe(false)
    expect(w.find('.dag-empty').exists()).toBe(true)
    w.unmount()
  })
})
