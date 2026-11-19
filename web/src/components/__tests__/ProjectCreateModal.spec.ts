import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// L6++（2026-09-08）F1/F3/F4：新建项目弹窗。校验分层——前端只拦非空 +
// 绝对路径形态（行内提示，不发请求）；存在性/重叠/上限后端权威（拒绝
// 时错误原文回显、modal 不关、无残留条目）。

const createProjectMock = vi.fn()
// D 批（2026-11-19）围栏多根：add-dir 模式走 store 的 addProjectDir /
// removeProjectDir（store 内部调本 mock 的同名方法）。
const addProjectDirMock = vi.fn()
const removeProjectDirMock = vi.fn()

vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({
    list: vi.fn(),
    listProjects: vi.fn().mockResolvedValue({ projects: [], count: 0 }),
    create: vi.fn(),
    createProject: (...a: any[]) => createProjectMock(...a),
    addProjectDir: (...a: any[]) => addProjectDirMock(...a),
    removeProjectDir: (...a: any[]) => removeProjectDirMock(...a),
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
  addProjectDirMock.mockReset()
  removeProjectDirMock.mockReset()
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

// ── D 批（2026-11-19）围栏多根：add-dir 模式 ──
// ＋项目按钮语义分叉的项目臂弹窗：给既有项目添加附加放行目录 + 已有
// extra_dirs 摘除（摘列表不动磁盘）。生效时机诚实声明随 toast（不热生效）。

const ADD_DIR_PROJECT = {
  id: 'p-dir-01',
  name: '计费服务',
  path: 'C:/x/billing',
  created_at: 't',
  running: true,
  extra_dirs: ['D:/assets/lib'],
}

async function mountAddDir(project: any = ADD_DIR_PROJECT) {
  const w = mount(ProjectCreateModal, { props: { mode: 'add-dir', project } })
  await flushPromises()
  return w
}

describe('ProjectCreateModal add-dir mode (D 批围栏多根)', () => {
  it('标题带项目名 + 只有一个路径输入（无名称框）+ 诚实 hint', async () => {
    const w = await mountAddDir()
    expect(w.find('.modal-header h3').text()).toContain('添加目录')
    expect(w.find('.modal-header h3').text()).toContain('计费服务')
    // add-dir 只有一个 form-input（路径）；名称输入不渲染。
    expect(w.findAll('.form-input')).toHaveLength(1)
    // hint 声明不进文件树/checkpoint + 不热生效（诚实边界）。
    expect(w.find('.hint').text()).toContain('不进 checkpoint')
    expect(w.find('.hint').text()).toContain('不热生效')
    w.unmount()
  })

  it('空路径/相对路径 → 行内提示，不发请求；绝对路径 → addProjectDir(project.id, path)', async () => {
    let w = await mountAddDir()
    await w.find('.btn.primary').trigger('click')
    expect(w.find('.inline-error').text()).toContain('路径')
    expect(addProjectDirMock).not.toHaveBeenCalled()

    await w.find('.form-input').setValue('relative/path')
    await w.find('.btn.primary').trigger('click')
    expect(w.find('.inline-error').text()).toContain('绝对路径')
    expect(addProjectDirMock).not.toHaveBeenCalled()
    w.unmount()

    addProjectDirMock.mockResolvedValue({
      project: { ...ADD_DIR_PROJECT, extra_dirs: [...ADD_DIR_PROJECT.extra_dirs, 'E:/share/pub'] },
      note: '已写入注册表',
    })
    w = await mountAddDir()
    await w.find('.form-input').setValue('E:\\share\\pub')
    await w.find('.btn.primary').trigger('click')
    await flushPromises()
    expect(addProjectDirMock).toHaveBeenCalledWith('p-dir-01', 'E:\\share\\pub')
    // 提交成功后输入框清空（可连续添加），modal 不关（emit close 只在 create 模式）。
    expect((w.find('.form-input').element as HTMLInputElement).value).toBe('')
    expect(w.emitted('close')).toBeUndefined()
    expect(w.emitted('updated')).toHaveLength(1)
    w.unmount()
  })

  it('已有 extra_dirs 渲染列表；✕（confirm）→ removeProjectDir + emit updated', async () => {
    vi.stubGlobal('confirm', () => true)
    removeProjectDirMock.mockResolvedValue({
      project: { ...ADD_DIR_PROJECT, extra_dirs: [] },
      note: '已摘除',
    })
    const w = await mountAddDir()
    const rows = w.findAll('.extra-row')
    expect(rows).toHaveLength(1)
    expect(rows[0].text()).toContain('D:/assets/lib')
    await rows[0].find('.extra-remove').trigger('click')
    await flushPromises()
    expect(removeProjectDirMock).toHaveBeenCalledWith('p-dir-01', 'D:/assets/lib')
    expect(w.emitted('updated')).toHaveLength(1)
    vi.unstubAllGlobals()
    w.unmount()
  })

  it('confirm 取消 → 不发请求', async () => {
    vi.stubGlobal('confirm', () => false)
    const w = await mountAddDir()
    await w.findAll('.extra-row')[0].find('.extra-remove').trigger('click')
    await flushPromises()
    expect(removeProjectDirMock).not.toHaveBeenCalled()
    vi.unstubAllGlobals()
    w.unmount()
  })

  it('后端拒绝（重叠/不存在）→ 错误原文行内回显，modal 不关', async () => {
    addProjectDirMock.mockRejectedValue(new Error('附加目录与主工作区重叠'))
    const w = await mountAddDir()
    await w.find('.form-input').setValue('C:/main_ws/inside')
    await w.find('.btn.primary').trigger('click')
    await flushPromises()
    expect(w.find('.inline-error').text()).toContain('附加目录与主工作区重叠')
    expect(w.emitted('close')).toBeUndefined()
    w.unmount()
  })
})
