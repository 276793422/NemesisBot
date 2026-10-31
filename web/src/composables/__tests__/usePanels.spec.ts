import { describe, it, expect, vi, beforeEach } from 'vitest'

// 面板注册表 composable（P2）：fetch 去重/失败保持/变化去重 + route 查询
// + 侧栏条目投影。
const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import { usePanels, panelNavItems, PANEL_NAV_ICON, type PanelPlugin } from '../../composables/usePanels'

const PANEL_A: PanelPlugin = {
  slug: 'panel-echo',
  name: '回显面板',
  payload: true,
  pages: [
    {
      route: 'echo',
      title: '回显面板',
      icon: 'puzzle',
      views: [
        {
          kind: 'table',
          bind: 'cmd://rows',
          columns: [{ key: 'id', label: '编号' }],
          fields: [],
          buttons: [],
        },
      ],
    },
  ],
}

const PANEL_B: PanelPlugin = {
  slug: 'bugfix-flow',
  name: 'BUG 修复流程',
  payload: true,
  pages: [{ route: 'bugfix', title: 'BUG 修复', views: [] }],
}

describe('usePanels', () => {
  beforeEach(() => {
    requestMock.mockReset()
    usePanels().panels.value = [] // 单例复位（模块级状态跨用例残留）
  })

  it('fetchPanels 拉取注册表并投影侧栏条目与 route 查询', async () => {
    requestMock.mockResolvedValue({ panels: [PANEL_A] })
    await usePanels().fetchPanels()
    expect(requestMock).toHaveBeenCalledWith('plugins', 'ui.registry', {}, 8000)
    expect(usePanels().panels.value).toHaveLength(1)

    const items = panelNavItems()
    expect(items).toEqual([
      { id: 'panel/echo', label: '回显面板', icon: PANEL_NAV_ICON, route: '/panel/echo' },
    ])

    const hit = usePanels().pageByRoute('echo')
    expect(hit?.plugin.slug).toBe('panel-echo')
    expect(hit?.page.views[0].bind).toBe('cmd://rows')
    expect(usePanels().pageByRoute('ghost')).toBeNull()
  })

  it('fetchPanels 失败保持现有清单（轮询自愈语义）', async () => {
    requestMock.mockResolvedValueOnce({ panels: [PANEL_A] })
    await usePanels().fetchPanels()
    requestMock.mockRejectedValueOnce('timeout: plugins.ui.registry')
    await usePanels().fetchPanels()
    expect(usePanels().panels.value).toHaveLength(1)
    expect(usePanels().panels.value[0].slug).toBe('panel-echo')
  })

  it('并发 fetchPanels 去重（in-flight 单飞）', async () => {
    requestMock.mockImplementation(
      () => new Promise((r) => setTimeout(() => r({ panels: [PANEL_A] }), 5)),
    )
    const p = usePanels()
    await Promise.all([p.fetchPanels(), p.fetchPanels(), p.fetchPanels()])
    expect(requestMock).toHaveBeenCalledTimes(1)
  })

  it('内容不变不整单替换（轮询零响应式扰动），变化才更新', async () => {
    const p = usePanels()
    requestMock.mockResolvedValue({ panels: [PANEL_A] })
    await p.fetchPanels()
    const ref1 = p.panels.value
    await p.fetchPanels() // 同内容 → 引用不变
    expect(p.panels.value).toBe(ref1)

    requestMock.mockResolvedValue({ panels: [PANEL_A, PANEL_B] })
    await p.fetchPanels()
    expect(p.panels.value).toHaveLength(2)
    expect(panelNavItems().map((i) => i.id)).toEqual(['panel/echo', 'panel/bugfix'])
  })

  it('响应缺 panels 字段 = 空清单（诚实降级不炸）', async () => {
    requestMock.mockResolvedValue({ unexpected: true })
    await usePanels().fetchPanels()
    expect(usePanels().panels.value).toEqual([])
  })
})
