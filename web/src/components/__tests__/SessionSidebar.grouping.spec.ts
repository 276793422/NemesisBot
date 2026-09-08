import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// L6++（2026-09-08）：会话侧栏对话/项目双分组。分组纯前端派生（组只是
// 聚合键，会话行渲染单源复用）：项目组=按注册表聚合；对话组=无 projectId；
// 孤儿组=projectId 在注册表已不存在的残留绑定 →「已移除」灰组。
// pin 语义=组内置顶；不可用项目（running=false）组头 ⚠ 置灰且无「＋」。
// 2026-09-08 三块改版：展示顺序=项目组在上、对话组在下、孤儿组垫底；
// 项目管理收进组头 hover「⋯」菜单（含打开目录）；顶部功能区=新建对话/
// 定时/Skills/工作区。

const listMock = vi.fn()
const listProjectsMock = vi.fn()
const createMock = vi.fn()
const removeProjectMock = vi.fn()
const deleteMock = vi.fn()
const openProjectDirMock = vi.fn()
const routerPushMock = vi.fn()

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: (...a: any[]) => routerPushMock(...a) }),
}))

vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({
    list: (...a: any[]) => listMock(...a),
    listProjects: (...a: any[]) => listProjectsMock(...a),
    create: (...a: any[]) => createMock(...a),
    createProject: vi.fn(),
    removeProject: (...a: any[]) => removeProjectMock(...a),
    renameProject: vi.fn(),
    openProjectDir: (...a: any[]) => openProjectDirMock(...a),
    rename: vi.fn(),
    clear: vi.fn(),
    export: vi.fn(),
    delete: (...a: any[]) => deleteMock(...a),
    turns: vi.fn(),
    fork: vi.fn(),
  }),
}))

import SessionSidebar from '../SessionSidebar.vue'

beforeEach(() => {
  setActivePinia(createPinia())
  listMock.mockReset()
  listProjectsMock.mockReset()
  createMock.mockReset()
  removeProjectMock.mockReset()
  deleteMock.mockReset()
  openProjectDirMock.mockReset()
  openProjectDirMock.mockResolvedValue({ opened: 'C:/x' })
  routerPushMock.mockReset()
  // store.remove 读 res.paused_cron_jobs——默认给成功空响应。
  deleteMock.mockResolvedValue({})
  localStorage.clear()
})

async function mountWith(sessions: any[], projects: any[]) {
  listMock.mockResolvedValue({ sessions })
  listProjectsMock.mockResolvedValue({ projects, count: projects.length })
  const w = mount(SessionSidebar)
  await flushPromises()
  return w
}

function groupNames(w: any): string[] {
  return w.findAll('.group-header .group-name').map((n: any) => n.text().trim())
}

function titles(w: any): string[] {
  return w.findAll('.session-item .session-title').map((t: any) => t.text().replace('📌', '').trim())
}

/** 在（当前打开的）菜单里点指定文案的条目。 */
async function clickMenuItem(w: any, scope: string, text: string) {
  const item = w
    .findAll(`${scope} .menu-item`)
    .find((b: any) => b.text().includes(text))
  expect(item, `menu item "${text}" must exist in ${scope}`).toBeTruthy()
  await item.trigger('click')
}

const P1 = { id: 'p1', name: '项目一', path: 'C:/x/1', created_at: 't', running: true }
const P2 = { id: 'p2', name: '项目二', path: 'C:/x/2', created_at: 't', running: true }
const P_DOWN = { id: 'p3', name: '坏项目', path: 'C:/x/3', created_at: 't', running: false }

describe('SessionSidebar 双分组 (L6++ G5)', () => {
  it('零项目：全部会话落对话组，项目区无组头，「＋新建项目」入口可达', async () => {
    const w = await mountWith([{ id: 'a', firstMessage: '会话 A' }], [])
    expect(titles(w)).toEqual(['会话 A'])
    expect(groupNames(w)).toEqual([])
    const createBtn = w.findAll('.project-create-btn')
    expect(createBtn).toHaveLength(1)
    // 入口点击 → modal 打开（关闭回调由组件内部管理）。
    await createBtn[0].trigger('click')
    expect(w.find('.modal-backdrop').exists()).toBe(true)
    w.unmount()
  })

  it('顶部功能区：新建对话 / 定时 / Skills / 工作区四入口', async () => {
    const w = await mountWith([], [])
    // 新建对话 → store.create。
    createMock.mockResolvedValue({ session_id: 's9', title: '新对话' })
    await w.find('.fn-new-chat').trigger('click')
    await flushPromises()
    expect(createMock).toHaveBeenCalled()
    const { useSessionStore } = await import('../../stores/session')
    expect(useSessionStore().currentId).toBe('s9')

    // 定时 / Skills → 路由跳转。
    const btns = w.findAll('.fn-btn').map((b: any) => b.text())
    expect(btns).toHaveLength(3)
    await w.findAll('.fn-btn')[0].trigger('click')
    expect(routerPushMock).toHaveBeenCalledWith('/tasks')
    await w.findAll('.fn-btn')[1].trigger('click')
    expect(routerPushMock).toHaveBeenCalledWith('/skills')

    // 工作区 → 共享文件树单例开合（偏好持久化）。
    await w.findAll('.fn-btn')[2].trigger('click')
    expect(localStorage.getItem('nb_filetree_collapsed')).toBe('0')
    expect(w.findAll('.fn-btn')[2].classes()).toContain('on')
    w.unmount()
  })

  it('多项目混排：按注册表聚合分组，组头计数正确；项目组在上、对话组在下', async () => {
    const w = await mountWith(
      [
        { id: 'a', firstMessage: '对话会话' },
        { id: 'x', firstMessage: 'P1 会话一', projectId: 'p1' },
        { id: 'y', firstMessage: 'P1 会话二', projectId: 'p1' },
        { id: 'z', firstMessage: 'P2 会话', projectId: 'p2' },
      ],
      [P1, P2],
    )
    // DOM 顺序 = 项目一组 → 项目二组 → 对话组。
    expect(titles(w)).toEqual(['P1 会话一', 'P1 会话二', 'P2 会话', '对话会话'])
    expect(groupNames(w)).toEqual(['项目一', '项目二'])
    expect(w.findAll('.group-header .group-count').map((c: any) => c.text())).toEqual(['(2)', '(1)'])
    w.unmount()
  })

  it('pin=组内置顶：项目组内 pinned 会话置顶，不跨组', async () => {
    localStorage.setItem('nb_pinned_sessions', JSON.stringify(['y', 'a']))
    const w = await mountWith(
      [
        { id: 'a', firstMessage: '对话会话' },
        { id: 'x', firstMessage: 'P1 会话一', projectId: 'p1' },
        { id: 'y', firstMessage: 'P1 会话二', projectId: 'p1' },
      ],
      [P1],
    )
    // 项目组：y 置顶到 x 前；对话组：a 置顶（仅 1 条看不出序，但标记在）。
    expect(titles(w)).toEqual(['P1 会话二', 'P1 会话一', '对话会话'])
    w.unmount()
  })

  it('孤儿组派生：注册表已无此 pid 的会话落「已移除」灰组（垫底、无＋无菜单），删除后组消失', async () => {
    vi.stubGlobal('confirm', () => true)
    const w = await mountWith(
      [
        { id: 'g', firstMessage: '孤儿会话', projectId: 'p_gone' },
        { id: 'a', firstMessage: '对话会话' },
      ],
      [],
    )
    expect(groupNames(w)).toEqual(['已移除'])
    const orphanHeader = w.findAll('.group-header')[0]
    expect(orphanHeader.classes()).toContain('orphan')
    // 灰组无就地新建（＋）也无管理菜单（⋯）。
    expect(orphanHeader.find('.add-in-group').exists()).toBe(false)
    expect(orphanHeader.find('.group-more').exists()).toBe(false)
    // 行序：对话组在上、孤儿组垫底。
    expect(titles(w)).toEqual(['对话会话', '孤儿会话'])
    // 可删除：删掉唯一孤儿会话 → 灰组自然消失（按标题定位孤儿行）。
    const orphanItem = w
      .findAll('.session-item')
      .find((i: any) => i.text().includes('孤儿会话'))!
    await orphanItem.find('.row-more').trigger('click')
    await clickMenuItem(w, '.row-menu', '删除会话')
    await flushPromises()
    expect(deleteMock).toHaveBeenCalledWith('g')
    expect(groupNames(w)).toEqual([])
    vi.unstubAllGlobals()
    w.unmount()
  })

  it('不可用项目（running=false）：组头置灰 ⚠，无「＋」就地新建（菜单仍可达打开目录/移除）', async () => {
    const w = await mountWith([{ id: 'd', firstMessage: '坏项目会话', projectId: 'p3' }], [P_DOWN])
    const header = w.findAll('.group-header')[0]
    expect(header.classes()).toContain('unavailable')
    expect(header.find('.group-warn').exists()).toBe(true)
    expect(header.find('.add-in-group').exists()).toBe(false)
    // 菜单可用：无「新建会话」，但打开目录/重命名/移除项目在。
    await header.find('.group-more').trigger('click')
    const items = header.findAll('.group-menu .menu-item').map((b: any) => b.text())
    expect(items.some((t: string) => t.includes('新建会话'))).toBe(false)
    expect(items.some((t: string) => t.includes('打开目录'))).toBe(true)
    expect(items.some((t: string) => t.includes('移除项目'))).toBe(true)
    w.unmount()
  })

  it('可用项目组「＋」就地新建：sessions.create 透传 project_id（乐观行落组内）', async () => {
    createMock.mockResolvedValue({ session_id: 's_new', title: '新对话' })
    const w = await mountWith([], [P1])
    const header = w.findAll('.group-header')[0]
    await header.find('.add-in-group').trigger('click')
    await flushPromises()
    expect(createMock).toHaveBeenCalledWith(undefined, 'p1')
    // 乐观行直接落项目一组（无需等 list 刷新）。
    expect(titles(w)).toEqual(['新对话'])
    expect(groupNames(w)).toEqual(['项目一'])
    const { useSessionStore } = await import('../../stores/session')
    expect(useSessionStore().currentId).toBe('s_new')
    w.unmount()
  })

  it('项目菜单「打开目录」→ projects.open_dir 透传 project_id', async () => {
    const w = await mountWith([], [P1])
    const header = w.findAll('.group-header')[0]
    await header.find('.group-more').trigger('click')
    await clickMenuItem(w, '.group-menu', '打开目录')
    expect(openProjectDirMock).toHaveBeenCalledWith('p1')
    w.unmount()
  })

  it('移除项目：确认文案钉死「仅解除分组，不删除…」；拒绝则不调 API；确认后落孤儿组', async () => {
    removeProjectMock.mockResolvedValue({ removed: P1, note: '仅解除分组，未删除会话与项目目录内的任何文件' })

    // 拒绝路径：confirm=false → 不调 API（独立挂载——成功移除后组变孤儿组
    // 无 ⋯ 菜单，两路径不能共用一次挂载）。
    vi.stubGlobal('confirm', () => false)
    let w = await mountWith([{ id: 'x', firstMessage: 'P1 会话', projectId: 'p1' }], [P1])
    let header = w.findAll('.group-header')[0]
    await header.find('.group-more').trigger('click')
    await clickMenuItem(w, '.group-menu', '移除项目')
    expect(removeProjectMock).not.toHaveBeenCalled()
    vi.unstubAllGlobals()
    w.unmount()

    // 确认路径：文案钉死 + removeProject 调用 + 落孤儿组。
    const confirmSpy = vi.fn(() => true)
    vi.stubGlobal('confirm', confirmSpy)
    w = await mountWith([{ id: 'x', firstMessage: 'P1 会话', projectId: 'p1' }], [P1])
    header = w.findAll('.group-header')[0]
    await header.find('.group-more').trigger('click')
    await clickMenuItem(w, '.group-menu', '移除项目')

    expect(confirmSpy).toHaveBeenCalledTimes(1)
    const asked = String(confirmSpy.mock.calls[0][0])
    expect(asked).toContain('仅解除分组，不删除会话与项目目录内的任何文件')
    expect(removeProjectMock).toHaveBeenCalledWith('p1')
    await flushPromises()
    // 组头消失，其会话落「已移除」灰组（projectId 残留，纯前端派生）。
    expect(groupNames(w)).toEqual(['已移除'])
    expect(titles(w)).toEqual(['P1 会话'])
    vi.unstubAllGlobals()
    w.unmount()
  })
})
