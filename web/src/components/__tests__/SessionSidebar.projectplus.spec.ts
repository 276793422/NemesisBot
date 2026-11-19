import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// D 批（2026-11-19）围栏多根：＋项目按钮语义分叉——当前会话属某项目 →
// 打开该项目的「添加目录」弹窗（ProjectCreateModal mode=add-dir）；非项目
// 会话/无选中 → 原语义「新建项目」。tooltip 随语义走。

const listMock = vi.fn()
const listProjectsMock = vi.fn()
vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn() }),
}))
vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({
    list: (...a: any[]) => listMock(...a),
    listProjects: (...a: any[]) => listProjectsMock(...a),
    create: vi.fn(),
    createProject: vi.fn(),
    addProjectDir: vi.fn(),
    removeProjectDir: vi.fn(),
    removeProject: vi.fn(),
    renameProject: vi.fn(),
    openProjectDir: vi.fn(),
    rename: vi.fn(),
    clear: vi.fn(),
    export: vi.fn(),
    delete: vi.fn(),
    turns: vi.fn(),
    fork: vi.fn(),
  }),
}))

import SessionSidebar from '../SessionSidebar.vue'
import ProjectCreateModal from '../ProjectCreateModal.vue'
import { useSessionStore } from '../../stores/session'

const PROJECT = {
  id: 'p-plus-01',
  name: '计费服务',
  path: 'C:/x/billing',
  created_at: 't',
  running: true,
  extra_dirs: [],
}

beforeEach(() => {
  setActivePinia(createPinia())
  listMock.mockReset()
  listProjectsMock.mockReset()
})

async function mountWithStore(sessions: any[], projects: any[]) {
  listMock.mockResolvedValue({ sessions })
  listProjectsMock.mockResolvedValue({ projects, count: projects.length })
  const w = mount(SessionSidebar)
  await flushPromises()
  return w
}

describe('SessionSidebar ＋项目按钮语义分叉（D 批围栏多根）', () => {
  it('当前会话属项目 → add-dir 弹窗（project prop = 该项目）', async () => {
    const w = await mountWithStore(
      [{ id: 's1', firstMessage: '项目会话', projectId: 'p-plus-01' }],
      [PROJECT],
    )
    const store = useSessionStore()
    store.switchTo('s1')
    await flushPromises()

    expect(w.find('.project-create-btn').attributes('title')).toContain('添加放行目录')
    await w.find('.project-create-btn').trigger('click')

    const modal = w.findComponent(ProjectCreateModal)
    expect(modal.exists()).toBe(true)
    expect(modal.props('mode')).toBe('add-dir')
    expect((modal.props('project') as any).id).toBe('p-plus-01')
    w.unmount()
  })

  it('当前会话非项目 → 新建项目弹窗（mode 缺省 create）', async () => {
    const w = await mountWithStore(
      [
        { id: 's1', firstMessage: '项目会话', projectId: 'p-plus-01' },
        { id: 's2', firstMessage: '普通会话' },
      ],
      [PROJECT],
    )
    const store = useSessionStore()
    store.switchTo('s2')
    await flushPromises()

    expect(w.find('.project-create-btn').attributes('title')).toBe('新建项目')
    await w.find('.project-create-btn').trigger('click')

    const modal = w.findComponent(ProjectCreateModal)
    expect(modal.exists()).toBe(true)
    expect(modal.props('mode')).toBeUndefined()
    expect(modal.props('project')).toBeUndefined()
    w.unmount()
  })

  it('projectId 指向已移除项目（注册表无此 id）→ 回退新建项目（不误导）', async () => {
    const w = await mountWithStore(
      [{ id: 's1', firstMessage: '孤儿项目会话', projectId: 'p-gone' }],
      [PROJECT],
    )
    const store = useSessionStore()
    store.switchTo('s1')
    await flushPromises()

    await w.find('.project-create-btn').trigger('click')
    const modal = w.findComponent(ProjectCreateModal)
    expect(modal.props('mode')).toBeUndefined()
    w.unmount()
  })
})
