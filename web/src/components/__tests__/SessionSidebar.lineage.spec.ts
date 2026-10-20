import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// WS9/P17（2026-09-26）：SessionSidebar 谱系投影——组内会话按谱系树
// 缩进排序（子会话紧跟父会话、深度递增左内边距）+ 会话信息弹窗「谱系」
// 节（祖先链 ↳ 缩进行 + 缘由 + 回退记录 + 分支摘要预览）。
// 2026-10-20 改版：缩进收敛 10px/级；⑂ marker 升入标题行；分支行
// branched 树杈类 + 行菜单「脱离父分支」（detach）。

const listMock = vi.fn()
const detachMock = vi.fn()
vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn() }),
}))
vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({
    list: (...a: any[]) => listMock(...a),
    create: vi.fn(),
    rename: vi.fn(),
    clear: vi.fn(),
    export: vi.fn(),
    delete: vi.fn(),
    turns: vi.fn(),
    fork: vi.fn(),
    detach: (...a: any[]) => detachMock(...a),
  }),
}))

import SessionSidebar from '../SessionSidebar.vue'

beforeEach(() => {
  setActivePinia(createPinia())
  listMock.mockReset()
  detachMock.mockReset()
})

async function mountWith(sessions: any[]) {
  listMock.mockResolvedValue({ sessions })
  const w = mount(SessionSidebar)
  await flushPromises()
  return w
}

describe('SessionSidebar WS9 谱系树缩进排序', () => {
  it('子会话紧跟父会话之后，行内边距按深度递增（树形缩进）', async () => {
    // 输入乱序（孙 → 子 → 根）——DOM 序应为 根 → 子 → 孙。
    const w = await mountWith([
      { id: 'gc', firstMessage: '孙会话', parent: 'agent:main:session:child' },
      { id: 'child', firstMessage: '子会话', parent: 'agent:main:session:root' },
      { id: 'root', firstMessage: '根会话' },
    ])

    const rows = w.findAll('.session-item')
    const indexOf = (needle: string) => rows.findIndex(r => r.text().includes(needle))
    const root = indexOf('根会话')
    const child = indexOf('子会话')
    const gc = indexOf('孙会话')
    expect(root).toBeGreaterThanOrEqual(0)
    expect(child).toBe(root + 1)
    expect(gc).toBe(child + 1)

    // 缩进：根无内联 padding；子 10+10=20px；孙 10+20=30px（2026-10-20 收敛 10px/级）。
    expect(rows[root].attributes('style') || '').not.toContain('padding-left')
    expect(rows[child].attributes('style')).toContain('padding-left: 20px')
    expect(rows[gc].attributes('style')).toContain('padding-left: 30px')
    w.unmount()
  })

  it('父不在列表（已删/跨组）→ 降级为本组根，不缩进不树排', async () => {
    const w = await mountWith([
      { id: 'orphan', firstMessage: '孤儿 fork', parent: 'agent:main:session:gone' },
      { id: 'plain', firstMessage: '普通会话' },
    ])
    const rows = w.findAll('.session-item')
    for (const r of rows) {
      expect(r.attributes('style') || '').not.toContain('padding-left')
    }
    w.unmount()
  })

  it('分支行：⑂ marker 升入标题行 + branched 树杈类；行菜单「脱离父分支」仅分支会话可见', async () => {
    detachMock.mockResolvedValue({ session_id: 'child', detached: true })
    vi.stubGlobal('confirm', () => true)
    const w = await mountWith([
      { id: 'root', firstMessage: '根会话' },
      { id: 'child', firstMessage: '子会话', parent: 'agent:main:session:root' },
    ])

    const childRow = w.findAll('.session-item').find(r => r.text().includes('子会话'))!
    const rootRow = w.findAll('.session-item').find(r => r.text().includes('根会话'))!
    // ⑂ marker 只在分支会话标题行；树杈连接线挂点（::before 宿主类）同此。
    expect(childRow.find('.fork-marker').exists()).toBe(true)
    expect(childRow.classes()).toContain('branched')
    expect(rootRow.find('.fork-marker').exists()).toBe(false)
    expect(rootRow.classes()).not.toContain('branched')

    // 分支会话菜单有「脱离父分支」，点击 → confirm + detach + 列表刷新。
    await childRow.find('.row-more').trigger('click')
    const detachBtn = w.findAll('.menu-item').find(b => b.text().includes('脱离父分支'))
    expect(detachBtn).toBeTruthy()
    await detachBtn!.trigger('click')
    await flushPromises()
    expect(detachMock).toHaveBeenCalledWith('child')

    // 根会话（无 parent）菜单里没有该项。
    const rootRowAfter = w.findAll('.session-item').find(r => r.text().includes('根会话'))!
    await rootRowAfter.find('.row-more').trigger('click')
    const rootDetach = w.findAll('.menu-item').find(b => b.text().includes('脱离父分支'))
    expect(rootDetach).toBeUndefined()
    vi.unstubAllGlobals()
    w.unmount()
  })
})

describe('SessionSidebar 会话信息弹窗「谱系」节', () => {
  it('祖先链 ↳ 缩进行（根 → 父 → · 当前）+ 缘由/回退/摘要', async () => {
    const w = await mountWith([
      {
        id: 'child',
        firstMessage: '子会话标题',
        parent: 'agent:main:session:root',
        parentTitle: '根会话',
        forkedAtTurn: 2,
        forkReason: '探索备选方案',
        lineage: {
          parent_session_id: 'agent:main:session:root',
          fork_point_seq: 2,
          reason: '探索备选方案',
          ancestors: [
            { id: 'agent_main_session_root', title: '根会话' },
            { id: 'agent_main_session_grand', title: '祖会话' },
          ],
        },
        lastRewind: { at_index: 0, dropped_rows: 6, dropped_turns: 3, ts: '2026-09-26T10:00:00+08:00' },
        branchSummaryPreview: '被遗弃分支的结论摘要标记XYZ',
      },
      { id: 'root', firstMessage: '普通会话' },
    ])

    // 打开行菜单 → 会话信息。
    const childRow = w.findAll('.session-item').find(r => r.text().includes('子会话标题'))!
    await childRow.find('.row-more').trigger('click')
    const infoBtn = w.findAll('.menu-item').find(b => b.text().includes('会话信息'))!
    await infoBtn.trigger('click')
    await flushPromises()

    const box = w.find('.info-box')
    expect(box.exists()).toBe(true)
    // 祖先链：后端序「最近父 → 根」，渲染反转为 根 → 祖先中游 → · 当前。
    // text() 会 trim 行首缩进空格——缩进断言用原始 textContent。
    const lines = box.findAll('.lineage-line')
    expect(lines.length).toBe(3)
    expect(lines[0].text()).toBe('↳ 祖会话')
    expect((lines[1].element as HTMLElement).textContent).toBe('  ↳ 根会话')
    expect((lines[2].element as HTMLElement).textContent).toBe('    · 子会话标题')
    // 缘由 / 回退 / 摘要行。
    expect(box.text()).toContain('缘由：探索备选方案')
    expect(box.text()).toContain('第 0 行起截断')
    expect(box.text()).toContain('弃 3 轮 / 6 行')
    expect(box.text()).toContain('被遗弃分支的结论摘要标记XYZ')
    w.unmount()
  })

  it('无谱系事实的会话不渲染谱系/回退/摘要行（零增量）', async () => {
    const w = await mountWith([{ id: 'plain', firstMessage: '普通会话' }])
    const row = w.findAll('.session-item')[0]
    await row.find('.row-more').trigger('click')
    const infoBtn = w.findAll('.menu-item').find(b => b.text().includes('会话信息'))!
    await infoBtn.trigger('click')
    await flushPromises()

    const box = w.find('.info-box')
    expect(box.exists()).toBe(true)
    expect(box.find('.lineage-chain').exists()).toBe(false)
    expect(box.text()).not.toContain('分支回退')
    expect(box.text()).not.toContain('分支摘要')
    w.unmount()
  })
})
