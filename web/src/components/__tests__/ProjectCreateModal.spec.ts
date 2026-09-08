import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// L6++（2026-09-08）F1/F3/F4：新建项目弹窗。校验分层——前端只拦非空 +
// 绝对路径形态（行内提示，不发请求）；存在性/重叠/上限后端权威（拒绝
// 时错误原文回显、modal 不关、无残留条目）。

const createProjectMock = vi.fn()

vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({
    list: vi.fn(),
    listProjects: vi.fn().mockResolvedValue({ projects: [], count: 0 }),
    create: vi.fn(),
    createProject: (...a: any[]) => createProjectMock(...a),
    removeProject: vi.fn(),
    renameProject: vi.fn(),
    rename: vi.fn(),
    clear: vi.fn(),
    export: vi.fn(),
    delete: vi.fn(),
    turns: vi.fn(),
    fork: vi.fn(),
  }),
}))

import ProjectCreateModal from '../ProjectCreateModal.vue'

beforeEach(() => {
  setActivePinia(createPinia())
  createProjectMock.mockReset()
})

async function mountAndFill(name: string, path: string) {
  const w = mount(ProjectCreateModal)
  await flushPromises()
  const inputs = w.findAll('.form-input')
  await inputs[0].setValue(name)
  await inputs[1].setValue(path)
  return w
}

describe('ProjectCreateModal (L6++ G5)', () => {
  it('空名称 / 空路径 / 相对路径 → 行内提示，不发请求', async () => {
    let w = await mountAndFill('', 'C:/x')
    await w.find('.btn.primary').trigger('click')
    expect(w.find('.inline-error').text()).toContain('名称')
    expect(createProjectMock).not.toHaveBeenCalled()
    w.unmount()

    w = await mountAndFill('项目', '  ')
    await w.find('.btn.primary').trigger('click')
    expect(w.find('.inline-error').text()).toContain('路径')
    expect(createProjectMock).not.toHaveBeenCalled()
    w.unmount()

    w = await mountAndFill('项目', 'works/relative')
    await w.find('.btn.primary').trigger('click')
    expect(w.find('.inline-error').text()).toContain('绝对路径')
    expect(createProjectMock).not.toHaveBeenCalled()
    w.unmount()
  })

  it('Windows 与 POSIX 绝对路径形态都放行到请求层', async () => {
    createProjectMock.mockResolvedValue({
      project: { id: 'p1', name: 'n', path: 'p', created_at: 't', running: true },
    })
    let w = await mountAndFill('项目A', 'C:\\works\\a')
    await w.find('.btn.primary').trigger('click')
    await flushPromises()
    expect(createProjectMock).toHaveBeenCalledWith('项目A', 'C:\\works\\a')
    w.unmount()

    createProjectMock.mockClear()
    w = await mountAndFill('项目B', '/home/me/b')
    await w.find('.btn.primary').trigger('click')
    await flushPromises()
    expect(createProjectMock).toHaveBeenCalledWith('项目B', '/home/me/b')
    w.unmount()
  })

  it('后端拒绝 → 错误原文行内回显 + modal 不关（F4）', async () => {
    createProjectMock.mockRejectedValue(new Error('与主 workspace 重叠'))
    const w = await mountAndFill('项目', 'C:/x/overlap')
    await w.find('.btn.primary').trigger('click')
    await flushPromises()
    expect(w.find('.inline-error').text()).toContain('与主 workspace 重叠')
    expect(w.emitted('close')).toBeUndefined()
    expect(createProjectMock).toHaveBeenCalledTimes(1)
    w.unmount()
  })

  it('成功 → emit created + close（modal 关闭，无残留）', async () => {
    createProjectMock.mockResolvedValue({
      project: { id: 'p9', name: '计费服务', path: 'C:/x/billing', created_at: 't', running: true },
    })
    const w = await mountAndFill('计费服务', 'C:/x/billing')
    await w.find('.btn.primary').trigger('click')
    await flushPromises()
    expect(w.emitted('created')![0][0]).toMatchObject({ id: 'p9', name: '计费服务' })
    expect(w.emitted('close')).toHaveLength(1)
    w.unmount()
  })
})
