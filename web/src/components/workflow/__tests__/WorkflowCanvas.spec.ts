import { mount, flushPromises, type VueWrapper } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { nextTick } from 'vue'
import { useToast } from '../../../composables/useToast'

// ---------------------------------------------------------------------------
// 模块 mock：WSAPI / SSE（store 链路），参照 SkillsView.spec / WorkflowList.spec
// ---------------------------------------------------------------------------

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))
vi.mock('../../../composables/useSSE', () => ({
  on: vi.fn(),
  off: vi.fn(),
}))

// ---------------------------------------------------------------------------
// @vue-flow 全家桶 mock：画布组件在 jsdom 里无法真正跑 d3-zoom/拖拽。
// useVueFlow 返回的注册函数全部换成 vi.fn，测试里手动取回回调触发
// （onConnect / onNodeDragStop 等的真实事件流需要真实 VueFlow）。
// VueFlow 桩渲染 default slot + 逐节点渲染 #node-workflow slot，
// 让模板里的自定义节点分支（选中/run 状态/diff 徽章/幽灵节点）可被覆盖。
// ---------------------------------------------------------------------------

const vfMocks = vi.hoisted(() => ({
  onConnect: vi.fn(),
  addEdges: vi.fn(),
  removeNodes: vi.fn(),
  removeEdges: vi.fn(),
  fitView: vi.fn(),
  onNodesChange: vi.fn(),
  onEdgesChange: vi.fn(),
  screenToFlowCoordinate: vi.fn(),
  onNodeDragStop: vi.fn(),
  onEdgeUpdate: vi.fn(),
}))

vi.mock('@vue-flow/core', async () => {
  const { defineComponent, h } = await import('vue')
  const VueFlow = defineComponent({
    name: 'VueFlow',
    props: {
      nodes: { type: Array, default: () => [] },
      edges: { type: Array, default: () => [] },
    },
    emits: ['nodeDoubleClick', 'nodeClick', 'paneClick', 'update:nodes', 'update:edges'],
    setup(props: any, { slots }: any) {
      return () =>
        h('div', { class: 'vue-flow-stub' }, [
          slots.default?.(),
          ...(props.nodes ?? []).map((n: any) =>
            h(
              'div',
              { class: 'vf-node-wrap', 'data-id': n.id },
              [slots['node-workflow']?.({ id: n.id, data: n.data, position: n.position })],
            ),
          ),
        ])
    },
  })
  const Handle = defineComponent({
    name: 'Handle',
    props: { type: { type: String, default: 'source' }, position: { type: String, default: 'top' } },
    setup(props: any) {
      return () => h('div', { class: 'vf-handle', 'data-handle-type': props.type })
    },
  })
  return {
    VueFlow,
    Handle,
    Position: { Left: 'left', Right: 'right', Top: 'top', Bottom: 'bottom' },
    MarkerType: { ArrowClosed: 'arrowclosed' },
    useVueFlow: () => vfMocks,
  }
})

vi.mock('@vue-flow/background', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Background: defineComponent({ name: 'Background', setup: () => () => h('div', { class: 'vf-background' }) }),
  }
})
vi.mock('@vue-flow/controls', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Controls: defineComponent({ name: 'Controls', setup: () => () => h('div', { class: 'vf-controls' }) }),
  }
})
vi.mock('@vue-flow/minimap', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    MiniMap: defineComponent({ name: 'MiniMap', setup: () => () => h('div', { class: 'vf-minimap' }) }),
  }
})

// 配置面板桩：WorkflowNodeConfig 会拖进整套 node-configs 表单树，测试只关心
// update/close 事件通道与 node/variables props 的传递。
vi.mock('../WorkflowNodeConfig.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'WorkflowNodeConfig',
      props: {
        node: { type: Object, default: null },
        variables: { type: Array, default: () => [] },
      },
      emits: ['update', 'close'],
      setup(props: any) {
        return () =>
          h('div', { class: 'node-config-stub', 'data-node-id': props.node?.id ?? '' }, [
            h('span', { class: 'config-var-count' }, String(props.variables?.length ?? 0)),
          ])
      },
    }),
  }
})

import WorkflowCanvas from '../WorkflowCanvas.vue'
import { useWorkflowStore } from '../../../stores/workflow'

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

function makeEditing() {
  return {
    name: 'wf1',
    description: 'desc',
    version: '1.0.0',
    triggers: [{ trigger_type: 'manual', config: {} }],
    nodes: [
      { id: 'n1', node_type: 'llm', config: { label: '我的LLM' } },
      { id: 'n2', node_type: 'tool', config: {} },
    ],
    edges: [{ from_node: 'n1', to_node: 'n2', condition: null }],
    variables: { greeting: '你好' },
    metadata: {},
  }
}

const DRAFT_PREVIEW = {
  name: 'draft-1',
  file_stem: 'draft-1',
  mtime_ms: 0,
  valid: true,
  validation_errors: [],
  node_count: 2,
  trigger_types: ['manual'],
  workflow: {
    name: 'draft-1',
    description: '',
    version: '1.0.0',
    triggers: [],
    nodes: [
      { id: 'd1', node_type: 'llm', config: {} },
      { id: 'd2', node_type: 'tool', config: {} },
    ],
    edges: [{ from_node: 'd1', to_node: 'd2', condition: 'ok' }],
    variables: {},
    metadata: {},
  },
  yaml: 'nodes: []',
}

const DRAFT_DIFF = {
  isNew: false,
  added: [{ kind: 'added' as const, nodeId: 'd1', nodeType: 'llm' }],
  changed: [{ kind: 'changed' as const, nodeId: 'd2', nodeType: 'tool', oldNodeType: 'llm' }],
  removed: [
    { kind: 'removed' as const, nodeId: 'old1', oldNodeType: 'script' },
    { kind: 'removed' as const, nodeId: 'old2' },
  ],
}

function defaultImpl(_m: string, cmd: string, _p?: any) {
  switch (cmd) {
    case 'validate':
      return Promise.resolve({ valid: true, errors: [] })
    case 'fire_event':
      return Promise.resolve({ published: true, matched_workflows: ['wf1'] })
    case 'draft_apply':
      return Promise.resolve({ name: 'draft-1', replaced_existing: false })
    case 'draft_list':
      return Promise.resolve({ drafts: [] })
    default:
      return Promise.resolve({})
  }
}

// ---------------------------------------------------------------------------
// 环境
// ---------------------------------------------------------------------------

let confirmMock: any
let promptMock: any
let alertMock: any
const wrappers: VueWrapper<any>[] = []

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockImplementation(defaultImpl)
  useToast().toasts.splice(0)
  for (const fn of Object.values(vfMocks)) (fn as any).mockReset()
  vfMocks.screenToFlowCoordinate.mockImplementation(({ x, y }: any) => ({ x, y }))
  confirmMock = vi.fn(() => true)
  promptMock = vi.fn(() => null)
  alertMock = vi.fn(() => undefined)
  vi.stubGlobal('confirm', confirmMock)
  vi.stubGlobal('prompt', promptMock)
  vi.stubGlobal('alert', alertMock)
})

afterEach(() => {
  while (wrappers.length) wrappers.pop()!.unmount()
  vi.unstubAllGlobals()
})

async function mountCanvas() {
  const w = mount(WorkflowCanvas)
  wrappers.push(w)
  await flushPromises()
  await nextTick()
  return w
}

/** 挂载后进入「编辑中」状态（直接写 store，watch(immediate) 已在 setup 跑过，重新赋值触发同步）。 */
async function mountEditing() {
  const store = useWorkflowStore()
  store.editing = makeEditing() as any
  return { w: await mountCanvas(), store }
}

function vf(w: VueWrapper<any>) {
  const c = w.findComponent({ name: 'VueFlow' })
  expect(c.exists()).toBe(true)
  return c
}

/** 取组件 setup 时注册的最新回调（每个用例只挂载一次，at(-1) 即当前实例）。 */
function connectHandler() {
  return vfMocks.onConnect.mock.calls.at(-1)![0] as (c: any) => void
}
function dragStopHandler() {
  return vfMocks.onNodeDragStop.mock.calls.at(-1)![0] as (e: any) => void
}

async function clickBtn(w: VueWrapper<any>, text: string) {
  const btn = w.findAll('button').find(b => b.text().trim() === text)
  expect(btn, `按钮「${text}」应存在`).toBeTruthy()
  await btn!.trigger('click')
  await flushPromises()
}

function pressKey(key: string, target?: HTMLElement) {
  ;(target ?? (window as unknown as HTMLElement)).dispatchEvent(
    new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }),
  )
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

describe('WorkflowCanvas 空态与基础渲染', () => {
  it('无编辑对象：空态画布、标题与归零统计', async () => {
    const w = await mountCanvas()
    const store = useWorkflowStore()
    expect(w.text()).toContain('画布（空）')
    expect(w.text()).toContain('节点 0')
    expect(w.text()).toContain('边 0')
    expect(w.text()).toContain('触发器 0')
    expect(w.find('.canvas-empty').exists()).toBe(true)
    expect(w.find('.palette').exists()).toBe(true)
    expect((w.vm as any).nodes.length).toBe(0)
    expect((w.vm as any).availableVariables).toEqual([])
    // 空态按钮：查看列表
    await clickBtn(w, '查看列表')
    expect(store.activeTab).toBe('list')
  })

  it('编辑态：节点/边渲染、网格布局、标题与统计', async () => {
    const { w } = await mountEditing()
    expect(w.text()).toContain('编辑：wf1')
    expect(w.text()).toContain('节点 2')
    expect(w.text()).toContain('边 1')
    expect(w.text()).toContain('触发器 1')
    expect(w.find('.canvas-empty').exists()).toBe(false)
    expect(w.find('.canvas-hint').text()).toContain('点击节点 = 选中')
    expect(w.findAll('.wf-node').length).toBe(2)
    expect(w.text()).toContain('我的LLM')
    expect((w.vm as any).nodes[0].position).toEqual({ x: 40, y: 40 })
    expect((w.vm as any).nodes[1].position).toEqual({ x: 300, y: 40 })
    expect((w.vm as any).edges[0].source).toBe('n1')
    expect((w.vm as any).edges[0].animated).toBe(false)
  })

  it('未知节点类型回退：label 用 node_type、category 兜底 basic', async () => {
    const store = useWorkflowStore()
    const editing = makeEditing()
    editing.nodes.push({ id: 'n3', node_type: 'weird_type', config: {} })
    store.editing = editing as any
    const w = await mountCanvas()
    expect(w.text()).toContain('weird_type')
    const node = w.findAll('.wf-node').find(el => el.text().includes('weird_type'))
    expect(node!.classes()).toContain('cat-basic')
  })

  it('重新同步保留既有节点位置，新节点落网格', async () => {
    const { w, store } = await mountEditing()
    ;(w.vm as any).nodes[0].position = { x: 99, y: 88 }
    store.editing = {
      ...(store.editing as any),
      nodes: [
        ...(store.editing as any).nodes,
        { id: 'n4', node_type: 'delay', config: {} },
      ],
      edges: [],
    } as any
    await flushPromises()
    await nextTick()
    expect((w.vm as any).nodes.length).toBe(3)
    expect((w.vm as any).nodes[0].position).toEqual({ x: 99, y: 88 })
    expect((w.vm as any).nodes[2].position).toEqual({ x: 560, y: 40 })
  })

  it('运行状态徽章：selectedRun.node_results 映射到节点', async () => {
    const store = useWorkflowStore()
    store.editing = makeEditing() as any
    store.selectedRun = {
      node_results: [{ node_id: 'n1', state: 'Completed' }, { node_id: 'n2', state: 'Failed' }],
    } as any
    const w = await mountCanvas()
    expect(w.findAll('.wf-node.run-completed').length).toBe(1)
    expect(w.findAll('.wf-node.run-failed').length).toBe(1)
    const badge = w.findAll('.wf-node-state').find(el => el.text() === 'Completed')
    expect(badge).toBeTruthy()
  })
})

describe('WorkflowCanvas 节点库 palette', () => {
  it('渲染全部节点条目', async () => {
    const w = await mountCanvas()
    expect(w.findAll('.palette-item').length).toBeGreaterThanOrEqual(10)
    expect(w.text()).toContain('调用大模型生成文本')
  })

  it('分类按钮过滤', async () => {
    const w = await mountCanvas()
    await w.findAll('.cat-btn').find(b => b.text() === 'AI')!.trigger('click')
    expect(w.findAll('.palette-item').length).toBe(4)
    expect(w.text()).not.toContain('基于变量值的分支')
    await w.findAll('.cat-btn').find(b => b.text() === '控制')!.trigger('click')
    expect(w.text()).toContain('基于变量值的分支')
    await w.findAll('.cat-btn').find(b => b.text() === '全部')!.trigger('click')
    expect(w.findAll('.palette-item').length).toBeGreaterThanOrEqual(10)
  })

  it('搜索按 label/type/description 过滤；无匹配显示空态', async () => {
    const w = await mountCanvas()
    // 'llm' 命中 type（llm）与两条 AI 节点的 description（LLM ...）
    await w.find('.palette-search').setValue('llm')
    expect(w.findAll('.palette-item').length).toBe(3)
    // description 命中
    await w.find('.palette-search').setValue('调用大模型')
    expect(w.findAll('.palette-item').length).toBe(1)
    await w.find('.palette-search').setValue('zzz-no-match')
    expect(w.find('.palette-empty').text()).toContain('无匹配节点')
  })

  it('点击 palette 新增节点：无编辑对象时自动新建并选中', async () => {
    const w = await mountCanvas()
    const store = useWorkflowStore()
    await w.findAll('.palette-item')[0].trigger('click')
    expect(store.editing).not.toBeNull()
    expect(store.editing!.nodes.length).toBe(1)
    expect(store.editing!.nodes[0].node_type).toBe('llm')
    expect((w.vm as any).selectedNodeId).toBe(store.editing!.nodes[0].id)
    expect(store.editingDirty).toBe(true)
    expect(w.text()).toContain('● 未保存')
  })

  it('点击 palette 新增节点：已有编辑对象时追加并落网格位', async () => {
    const { w, store } = await mountEditing()
    const item = w.findAll('.palette-item').find(i => i.attributes('title') === '基于变量值的分支')
    await item!.trigger('click')
    expect(store.editing!.nodes.length).toBe(3)
    const added = store.editing!.nodes[2]
    expect(added.node_type).toBe('condition')
    expect(added.id).toMatch(/^condition_/)
    expect((w.vm as any).nodes[2].position).toEqual({ x: 560, y: 40 })
    expect((w.vm as any).selectedNodeId).toBe(added.id)
  })

  it('dragstart 记录拖拽类型并 setData；无 dataTransfer 早退；dragend 清空', async () => {
    const w = await mountCanvas()
    const item = w.findAll('.palette-item')[0]
    // 无 dataTransfer：直接返回，不记录
    await item.trigger('dragstart')
    expect((w.vm as any).draggingType).toBeNull()
    const dt = { setData: vi.fn(), effectAllowed: '' }
    await item.trigger('dragstart', { dataTransfer: dt })
    expect((w.vm as any).draggingType).toBe('llm')
    expect(dt.effectAllowed).toBe('move')
    expect(dt.setData).toHaveBeenCalledWith('application/x-workflow-node', 'llm')
    await item.trigger('dragend')
    expect((w.vm as any).draggingType).toBeNull()
  })

  it('dragover：仅当正在拖拽时允许放置', async () => {
    const w = await mountCanvas()
    const container = w.find('.flow-container')
    const dt = { dropEffect: 'none' } as any
    await container.trigger('dragover', { dataTransfer: dt })
    expect(dt.dropEffect).toBe('none')
    ;(w.vm as any).draggingType = 'llm'
    await container.trigger('dragover', { dataTransfer: dt })
    expect(dt.dropEffect).toBe('move')
    // 无 dataTransfer 不炸
    await container.trigger('dragover')
  })

  it('drop：流坐标转换后新增节点', async () => {
    const { w, store } = await mountEditing()
    const item = w.findAll('.palette-item')[0]
    await item.trigger('dragstart', { dataTransfer: { setData: vi.fn() } })
    const container = w.find('.flow-container')
    await container.trigger('drop', {
      clientX: 30,
      clientY: 40,
      dataTransfer: { getData: () => 'llm' },
    })
    expect(vfMocks.screenToFlowCoordinate).toHaveBeenCalledWith({ x: 30, y: 40 })
    expect(store.editing!.nodes.length).toBe(3)
    expect(store.editing!.nodes[2].node_type).toBe('llm')
    expect((w.vm as any).nodes[2].position).toEqual({ x: 30, y: 40 })
    expect(store.editingDirty).toBe(true)
  })

  it('drop：无类型 / 未知类型 / 无 dataTransfer 均忽略', async () => {
    const { w, store } = await mountEditing()
    const container = w.find('.flow-container')
    await container.trigger('drop', { dataTransfer: { getData: () => '' } })
    await container.trigger('drop', { dataTransfer: { getData: () => 'no-such-type' } })
    await container.trigger('drop')
    expect(store.editing!.nodes.length).toBe(2)
    expect(vfMocks.screenToFlowCoordinate).not.toHaveBeenCalled()
  })
})

describe('WorkflowCanvas 连线与节点拖放回调', () => {
  it('onConnect 成功：写入 editing.edges 并 addEdges', async () => {
    const { w, store } = await mountEditing()
    connectHandler()({ source: 'n2', target: 'n1' })
    await nextTick()
    expect(store.editing!.edges.length).toBe(2)
    expect(store.editing!.edges[1]).toEqual({ from_node: 'n2', to_node: 'n1', condition: null })
    expect(vfMocks.addEdges).toHaveBeenCalledTimes(1)
    expect(vfMocks.addEdges.mock.calls[0][0].source).toBe('n2')
    expect(vfMocks.addEdges.mock.calls[0][0].target).toBe('n1')
    expect(store.editingDirty).toBe(true)
  })

  it('onConnect：自环 / 重复边 / 草稿预览 / 无编辑对象 均忽略', async () => {
    const { w, store } = await mountEditing()
    connectHandler()({ source: 'n1', target: 'n1' })
    connectHandler()({ source: 'n1', target: 'n2' })
    expect(store.editing!.edges.length).toBe(1)
    expect(vfMocks.addEdges).not.toHaveBeenCalled()
    // 无编辑对象
    store.discardEditing()
    await flushPromises()
    connectHandler()({ source: 'n1', target: 'n2' })
    // 草稿预览态
    store.enterDraftPreview(DRAFT_PREVIEW as any, null)
    await flushPromises()
    connectHandler()({ source: 'd1', target: 'd2' })
    expect(vfMocks.addEdges).not.toHaveBeenCalled()
  })

  it('onNodeDragStop：更新画布节点位置；无 node / 未知 id 安全跳过', async () => {
    const { w } = await mountEditing()
    dragStopHandler()({ node: { id: 'n1', position: { x: 5, y: 6 } } })
    expect((w.vm as any).nodes[0].position).toEqual({ x: 5, y: 6 })
    dragStopHandler()({})
    dragStopHandler()({ node: { id: 'nope', position: { x: 1, y: 1 } } })
    expect((w.vm as any).nodes[1].position).toEqual({ x: 300, y: 40 })
  })
})

describe('WorkflowCanvas 选中与配置面板', () => {
  it('node-click 选中、pane-click 取消；选中态样式', async () => {
    const { w } = await mountEditing()
    vf(w).vm.$emit('nodeClick', { node: { id: 'n1' } })
    await nextTick()
    expect((w.vm as any).selectedNodeId).toBe('n1')
    expect(w.findAll('.wf-node.selected').length).toBe(1)
    vf(w).vm.$emit('nodeClick', {})
    expect((w.vm as any).selectedNodeId).toBe('n1')
    vf(w).vm.$emit('paneClick')
    await nextTick()
    expect((w.vm as any).selectedNodeId).toBeNull()
    expect(w.findAll('.wf-node.selected').length).toBe(0)
  })

  it('双击节点打开配置面板；面板 close 关闭；availableVariables 传递', async () => {
    const { w } = await mountEditing()
    vf(w).vm.$emit('nodeDoubleClick', {})
    expect(w.find('.node-config-stub').exists()).toBe(false)
    vf(w).vm.$emit('nodeDoubleClick', { node: { id: 'n1' } })
    await nextTick()
    const panel = w.find('.node-config-stub')
    expect(panel.exists()).toBe(true)
    expect(panel.attributes('data-node-id')).toBe('n1')
    // 内置 8 个 + workflow 变量 1 个 + 节点输出（排除当前选中的 n1）1 个
    expect(panel.find('.config-var-count').text()).toBe('10')
    w.findComponent({ name: 'WorkflowNodeConfig' }).vm.$emit('close')
    await nextTick()
    expect(w.find('.node-config-stub').exists()).toBe(false)
  })

  it('面板 update → updateNodeDef 同步 def 与画布 data', async () => {
    const { w, store } = await mountEditing()
    vf(w).vm.$emit('nodeDoubleClick', { node: { id: 'n1' } })
    await nextTick()
    w.findComponent({ name: 'WorkflowNodeConfig' }).vm.$emit('update', { config: { label: '改名后' } })
    await nextTick()
    expect(store.editing!.nodes[0].config.label).toBe('改名后')
    expect((w.vm as any).nodes[0].data.label).toBe('改名后')
    expect(store.editingDirty).toBe(true)
  })

  it('updateNodeDef 守卫：未知 id / 无编辑对象', async () => {
    const { w, store } = await mountEditing()
    ;(w.vm as any).updateNodeDef('ghost-id', { config: {} })
    expect(store.editing!.nodes.length).toBe(2)
    store.discardEditing()
    await flushPromises()
    ;(w.vm as any).updateNodeDef('n1', { config: {} })
    expect(store.editing).toBeNull()
  })
})

describe('WorkflowCanvas 键盘删除', () => {
  it('Delete 删除选中节点及相邻边；取消选中与面板', async () => {
    const { w, store } = await mountEditing()
    vf(w).vm.$emit('nodeDoubleClick', { node: { id: 'n1' } })
    await nextTick()
    ;(w.vm as any).nodes[0].selected = true
    pressKey('Delete')
    await nextTick()
    expect(store.editing!.nodes.length).toBe(1)
    expect(store.editing!.edges.length).toBe(0)
    expect(vfMocks.removeNodes).toHaveBeenCalledWith(['n1'])
    expect(vfMocks.removeEdges).not.toHaveBeenCalled()
    expect((w.vm as any).selectedNodeId).toBeNull()
    expect(w.find('.node-config-stub').exists()).toBe(false)
    expect(store.editingDirty).toBe(true)
  })

  it('Backspace 删除选中边：正则匹配的边移除，不匹配的跳过', async () => {
    const { w, store } = await mountEditing()
    ;(w.vm as any).edges[0].selected = true
    ;(w.vm as any).edges.push({ id: 'weird-id', source: 'n1', target: 'n2', selected: true })
    pressKey('Backspace')
    await nextTick()
    // e-n1-n2-0 命中正则被移除；weird-id 不匹配正则仅跳过（continue）
    expect(store.editing!.edges.length).toBe(0)
    expect(vfMocks.removeEdges).toHaveBeenCalledWith(['e-n1-n2-0', 'weird-id'])
    expect(vfMocks.removeNodes).not.toHaveBeenCalled()
    expect(store.editingDirty).toBe(true)
  })

  it('无选中 / 焦点在输入框 / 草稿预览：Delete 不删除', async () => {
    const { w, store } = await mountEditing()
    pressKey('Delete')
    expect(store.editing!.nodes.length).toBe(2)

    const input = document.createElement('input')
    document.body.appendChild(input)
    pressKey('Delete', input)
    expect(store.editing!.nodes.length).toBe(2)
    input.remove()

    // 非 Delete/Backspace 键
    pressKey('a')
    expect(store.editing!.nodes.length).toBe(2)

    store.enterDraftPreview(DRAFT_PREVIEW as any, null)
    await flushPromises()
    expect((w.vm as any).nodes.length).toBe(2)
    ;(w.vm as any).nodes[0].selected = true
    pressKey('Delete')
    expect((w.vm as any).nodes.length).toBe(2)
    expect(vfMocks.removeNodes).not.toHaveBeenCalled()
  })

  it('editing 为 null：选中节点删除走 continue，仍清画布节点', async () => {
    const { w, store } = await mountEditing()
    store.discardEditing()
    await flushPromises()
    ;(w.vm as any).nodes.push({
      id: 'zz',
      selected: true,
      position: { x: 0, y: 0 },
      data: { label: 'zz', nodeType: 'tool', category: 'basic', def: { id: 'zz', node_type: 'tool', config: {} } },
    })
    pressKey('Delete')
    expect(vfMocks.removeNodes).toHaveBeenCalledWith(['zz'])
    expect(store.editing).toBeNull()
    expect(store.editingDirty).toBe(true)
  })
})

describe('WorkflowCanvas 工具栏', () => {
  it('新建按钮：创建空白工作流', async () => {
    const w = await mountCanvas()
    const store = useWorkflowStore()
    await clickBtn(w, '+ 新建')
    expect(store.editing).not.toBeNull()
    expect(store.editingIsNew).toBe(true)
    expect(w.text()).toContain('新建工作流：')
  })

  it('清空画布：confirm 取消 / 确认 / 空画布 / 无编辑对象', async () => {
    const { w, store } = await mountEditing()
    confirmMock.mockReturnValueOnce(false)
    await clickBtn(w, '🗑 清空')
    expect(store.editing!.nodes.length).toBe(2)

    confirmMock.mockReturnValueOnce(true)
    await clickBtn(w, '🗑 清空')
    expect(store.editing!.nodes.length).toBe(0)
    expect(store.editing!.edges.length).toBe(0)
    expect(store.editingDirty).toBe(true)
    expect((w.vm as any).nodes.length).toBe(0)
    expect(w.text()).toContain('节点 0')

    // 空画布再点：confirm 都不会被调
    const confirmCalls = confirmMock.mock.calls.length
    await clickBtn(w, '🗑 清空')
    expect(confirmMock.mock.calls.length).toBe(confirmCalls)

    // 无编辑对象：按钮 disabled 但直接触发也安全
    store.discardEditing()
    await flushPromises()
    await clickBtn(w, '🗑 清空')
    expect(store.editing).toBeNull()
  })

  it('丢弃并返回列表', async () => {
    const { w, store } = await mountEditing()
    await clickBtn(w, '✕ 丢弃')
    expect(store.editing).toBeNull()
    expect(store.activeTab).toBe('list')
    expect(w.text()).toContain('画布（空）')
  })

  it('适应窗口：nextTick 后 fitView', async () => {
    const { w } = await mountEditing()
    await clickBtn(w, '⤢ 适应')
    expect(vfMocks.fitView).toHaveBeenCalledWith({ padding: 0.2 })
  })

  it('导出 YAML：切页签', async () => {
    const { w, store } = await mountEditing()
    await clickBtn(w, '📝 YAML')
    expect(store.activeTab).toBe('yaml')
  })

  it('校验：无错误不显示区块；有错误渲染；请求失败兜底', async () => {
    const { w } = await mountEditing()
    await clickBtn(w, '🔍 校验')
    expect(requestMock).toHaveBeenCalledWith('workflow', 'validate', { workflow: expect.objectContaining({ name: 'wf1' }) })
    expect(w.find('.validation-errors').exists()).toBe(false)

    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'validate' ? Promise.resolve({ valid: false, errors: ['缺少 trigger'] }) : Promise.resolve({}))
    await clickBtn(w, '🔍 校验')
    expect(w.find('.validation-errors').exists()).toBe(true)
    expect(w.find('.validation-errors').text()).toContain('缺少 trigger')

    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'validate' ? Promise.reject('boom') : Promise.resolve({}))
    await clickBtn(w, '🔍 校验')
    expect(w.find('.validation-errors').text()).toContain('boom')
  })

  it('保存：空名 prompt 取消不保存；命名成功走 create；失败 alert', async () => {
    const w = await mountCanvas()
    const store = useWorkflowStore()
    await clickBtn(w, '+ 新建')
    // prompt 取消
    promptMock.mockReturnValueOnce(null)
    await clickBtn(w, '💾 保存')
    expect(requestMock.mock.calls.filter(c => c[1] === 'create').length).toBe(0)
    // prompt 命名
    promptMock.mockReturnValueOnce('  new-name  ')
    await clickBtn(w, '💾 保存')
    expect(store.editing!.name).toBe('new-name')
    expect(requestMock).toHaveBeenCalledWith('workflow', 'create', { workflow: expect.objectContaining({ name: 'new-name' }) })

    // 既有工作流更新失败 → alert
    store.editing = makeEditing() as any
    store.editingIsNew = false
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'update' ? Promise.reject(new Error('busy')) : Promise.resolve({}))
    await clickBtn(w, '💾 保存')
    expect(alertMock).toHaveBeenCalledWith(expect.stringContaining('保存失败'))
  })

  it('模拟事件：取消 / 空类型 / JSON 非法 / 非对象', async () => {
    const { w, store } = await mountEditing()
    const vm = w.vm as any
    // 第一层 prompt 取消
    promptMock.mockReturnValueOnce(null)
    await vm.simulateEvent()
    expect(requestMock.mock.calls.filter(c => c[1] === 'fire_event').length).toBe(0)
    // 空类型
    promptMock.mockReturnValueOnce('   ')
    await vm.simulateEvent()
    expect(requestMock.mock.calls.filter(c => c[1] === 'fire_event').length).toBe(0)
    // 第二层 JSON 解析失败
    promptMock.mockReturnValueOnce('evt.x').mockReturnValueOnce('{bad json')
    await vm.simulateEvent()
    expect(alertMock).toHaveBeenCalledWith(expect.stringContaining('JSON 解析失败'))
    // 非对象 JSON（数组）
    promptMock.mockReturnValueOnce('evt.x').mockReturnValueOnce('[1,2]')
    await vm.simulateEvent()
    expect(alertMock).toHaveBeenCalledWith(expect.stringContaining('必须是 JSON 对象'))
    expect(requestMock.mock.calls.filter(c => c[1] === 'fire_event').length).toBe(0)
    expect(store.editing).not.toBeNull()
  })

  it('模拟事件：发布失败 / 无匹配 / 匹配成功', async () => {
    const { w, store } = await mountEditing()
    const vm = w.vm as any
    // 发布失败
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'fire_event' ? Promise.resolve({ published: false, matched_workflows: [] }) : Promise.resolve({}))
    promptMock.mockReturnValueOnce('evt.x').mockReturnValueOnce(null)
    await vm.simulateEvent()
    expect(alertMock).toHaveBeenCalledWith(expect.stringContaining('事件发布失败'))
    // 无匹配
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'fire_event' ? Promise.resolve({ published: true, matched_workflows: [] }) : Promise.resolve({}))
    promptMock.mockReturnValueOnce('evt.x').mockReturnValueOnce(null)
    await vm.simulateEvent()
    expect(alertMock).toHaveBeenCalledWith(expect.stringContaining('没有工作流订阅'))
    // 匹配成功
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'fire_event' ? Promise.resolve({ published: true, matched_workflows: ['wf1'] }) : Promise.resolve({}))
    promptMock.mockReturnValueOnce('workflow.completed').mockReturnValueOnce('{"status":"ok"}')
    await vm.simulateEvent()
    expect(requestMock).toHaveBeenCalledWith('workflow', 'fire_event', {
      event_type: 'workflow.completed',
      data: { status: 'ok' },
    })
    expect(alertMock).toHaveBeenCalledWith(expect.stringContaining('已触发工作流'))
    expect(alertMock).toHaveBeenCalledWith(expect.stringContaining('wf1'))
    expect(store.editing).not.toBeNull()
  })
})

describe('WorkflowCanvas 草稿预览', () => {
  async function mountPreview(diff: any = DRAFT_DIFF) {
    const store = useWorkflowStore()
    store.enterDraftPreview(DRAFT_PREVIEW as any, diff)
    return { w: await mountCanvas(), store }
  }

  it('预览渲染：幽灵节点 / diff 徽章 / 统计 / palette 隐藏 / fitView', async () => {
    const { w, store } = await mountPreview()
    expect(w.text()).toContain('草稿预览：draft-1')
    expect(w.text()).toContain('节点 2')
    expect(w.text()).toContain('新增 1')
    expect(w.text()).toContain('修改 1')
    expect(w.text()).toContain('移除 2')
    expect(w.find('.palette').exists()).toBe(false)
    expect(w.find('.canvas-hint').text()).toContain('草稿预览（只读）')
    expect(w.findAll('.wf-node').length).toBe(4)
    expect(w.findAll('.wf-node.diff-removed.ghost').length).toBe(2)
    expect(w.text()).toContain('old1（将被移除）')
    expect(w.text()).toContain('old2（将被移除）')
    expect(w.findAll('.wf-node.diff-added').length).toBe(1)
    expect(w.findAll('.wf-node.diff-changed').length).toBe(1)
    const badges = w.findAll('.diff-badge')
    expect(badges.length).toBe(4)
    const kinds = badges.map(b => b.attributes('data-kind')).sort()
    expect(kinds).toEqual(['added', 'changed', 'removed', 'removed'])
    expect(w.text()).toContain('新增')
    expect((w.vm as any).edges[0].label).toBe('ok')
    expect((w.vm as any).edges[0].animated).toBe(true)
    expect(vfMocks.fitView).toHaveBeenCalledWith({ padding: 0.2 })
    expect(store.activeTab).toBe('canvas')
  })

  it('预览无 diff：无徽章、无幽灵、移除 0', async () => {
    const { w } = await mountPreview(null)
    expect(w.text()).toContain('移除 0')
    expect(w.findAll('.diff-badge').length).toBe(0)
    expect(w.findAll('.wf-node').length).toBe(2)
  })

  it('退出预览按钮：清预览态回对话生成 TAB；无编辑对象画布清空', async () => {
    const { w, store } = await mountPreview()
    await clickBtn(w, '✕ 退出预览')
    expect(store.draftPreview).toBeNull()
    expect(store.activeTab).toBe('agentGen')
    expect(w.text()).toContain('画布（空）')
    expect((w.vm as any).nodes.length).toBe(0)
  })

  it('退出预览（watch null 分支）：恢复编辑视图', async () => {
    const { w, store } = await mountEditing()
    store.enterDraftPreview(DRAFT_PREVIEW as any, DRAFT_DIFF as any)
    await flushPromises()
    expect((w.vm as any).nodes.length).toBe(4)
    store.exitDraftPreview()
    await flushPromises()
    await nextTick()
    expect((w.vm as any).nodes.length).toBe(2)
    expect((w.vm as any).nodes[0].id).toBe('n1')
  })

  it('预览态交互禁用：双击不开面板、Delete 与连线无效', async () => {
    const { w, store } = await mountPreview()
    vf(w).vm.$emit('nodeDoubleClick', { node: { id: 'd1' } })
    expect(w.find('.node-config-stub').exists()).toBe(false)
    ;(w.vm as any).nodes[0].selected = true
    pressKey('Delete')
    expect(vfMocks.removeNodes).not.toHaveBeenCalled()
    connectHandler()({ source: 'd1', target: 'd2' })
    expect(store.editing?.edges ?? []).toHaveLength(0)
    expect(vfMocks.addEdges).not.toHaveBeenCalled()
  })

  it('应用草稿：已有同名且 confirm 取消 → 不请求', async () => {
    const { w, store } = await mountPreview()
    store.workflows = [{ name: 'draft-1', description: '', version: '1', node_count: 2, trigger_count: 1, triggers: [], chat_index: 'x', has_chat_password: false }] as any
    confirmMock.mockReturnValueOnce(false)
    await clickBtn(w, '✓ 应用草稿')
    expect(requestMock.mock.calls.filter(c => c[1] === 'draft_apply').length).toBe(0)
    expect(store.draftPreview).not.toBeNull()
  })

  it('应用草稿成功（全新注册）：toast + 回对话生成', async () => {
    const { w, store } = await mountPreview()
    await clickBtn(w, '✓ 应用草稿')
    expect(requestMock).toHaveBeenCalledWith('workflow', 'draft_apply', { name: 'draft-1' })
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('已注册'))).toBe(true)
    expect(store.activeTab).toBe('agentGen')
    expect(store.draftPreview).toBeNull()
  })

  it('应用草稿成功（替换旧定义）', async () => {
    const { w, store } = await mountPreview()
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'draft_apply' ? Promise.resolve({ name: 'draft-1', replaced_existing: true }) : defaultImpl(_m, cmd))
    await clickBtn(w, '✓ 应用草稿')
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('已更新（旧定义已备份）'))).toBe(true)
    expect(store.draftPreview).toBeNull()
  })

  it('应用草稿失败：error toast，留在预览', async () => {
    const { w, store } = await mountPreview()
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'draft_apply' ? Promise.reject(new Error('磁盘已满')) : defaultImpl(_m, cmd))
    await clickBtn(w, '✓ 应用草稿')
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('磁盘已满'))).toBe(true)
    expect(store.draftPreview).not.toBeNull()
    expect(store.activeTab).toBe('canvas')
  })

  it('applyFromPreview 无预览态直接返回', async () => {
    const w = await mountCanvas()
    await (w.vm as any).applyFromPreview()
    expect(requestMock.mock.calls.filter(c => c[1] === 'draft_apply').length).toBe(0)
  })
})
