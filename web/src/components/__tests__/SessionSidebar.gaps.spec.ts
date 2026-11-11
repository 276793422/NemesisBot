import { describe, it, expect, vi, beforeEach, afterEach, type MockInstance } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'
import { useToast } from '../../composables/useToast'

// SessionSidebar 未覆盖面补充 spec（增量：不动既有 batch/changes/fork/
// grouping/lineage 用例）。覆盖目标（对应源码未覆盖行）：
// - loadProjectMax / saveProjectMax（config.get / config.set_field 全分支）；
// - SSE session.created / session.updated → force 刷新（onSessionCreated）；
// - 非批量模式行点击 select → switchTo；新建会话/组内新建失败 toast；
// - 组折叠 toggleCollapse / persistCollapsed / 脏 localStorage 兜底（三组）；
// - 项目组头重命名（startRename/commitRename 全分支 + '__' 守卫）；
// - removeProject / openDir 失败反馈；
// - 导出会话（Blob 下载 + 失败）/ 创建分支弹窗开合 + onForked；
// - 导入会话（triggerImport + onImportFile 全形态与失败）；
// - openChanges 当前会话直接失败（围栏内 toast + 关弹窗）；
// - B11 暂停/恢复（snoozeSession 各时长形态 / unsnooze / detach）+ 徽标
//   （snoozed ⏰ / pending 💤9+ / undelivered / allow_all 🔓）+ snoozeTip；
// - 会话信息弹窗（info modal 全字段 + projectOf 三态 + 关闭）；
// - wf_edit 会话隐藏（localStorage 登记表 + 服务端 bindings 并集）。

const listMock = vi.fn()
const deleteMock = vi.fn()
const listProjectsMock = vi.fn()
const createMock = vi.fn()
const renameMock = vi.fn()
const clearMock = vi.fn()
const exportMock = vi.fn()
const turnsMock = vi.fn()
const removeProjectMock = vi.fn()
const renameProjectMock = vi.fn()
const openProjectDirMock = vi.fn()
const snoozeMock = vi.fn()
const unsnoozeMock = vi.fn()
const detachMock = vi.fn()
const importSessionMock = vi.fn()
const requestMock = vi.fn()

const routerPushMock = vi.fn()

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: (...a: any[]) => routerPushMock(...a) }),
}))

vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({
    list: (...a: any[]) => listMock(...a),
    delete: (...a: any[]) => deleteMock(...a),
    listProjects: (...a: any[]) => listProjectsMock(...a),
    create: (...a: any[]) => createMock(...a),
    createProject: vi.fn(),
    removeProject: (...a: any[]) => removeProjectMock(...a),
    renameProject: (...a: any[]) => renameProjectMock(...a),
    openProjectDir: (...a: any[]) => openProjectDirMock(...a),
    rename: (...a: any[]) => renameMock(...a),
    clear: (...a: any[]) => clearMock(...a),
    export: (...a: any[]) => exportMock(...a),
    turns: (...a: any[]) => turnsMock(...a),
    fork: vi.fn(),
    detach: (...a: any[]) => detachMock(...a),
    snooze: (...a: any[]) => snoozeMock(...a),
    unsnooze: (...a: any[]) => unsnoozeMock(...a),
    importSession: (...a: any[]) => importSessionMock(...a),
    markDelivered: vi.fn(),
  }),
}))

vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

// SSE mock：注册表暴露为 __handlers，测试内派发 session.created / session.updated。
vi.mock('../../composables/useSSE', () => {
  const handlers: Record<string, Array<(d: any) => void>> = {}
  return {
    on: (ev: string, h: (d: any) => void) => {
      ;(handlers[ev] = handlers[ev] || []).push(h)
    },
    off: (ev: string, h?: (d: any) => void) => {
      if (!h) {
        delete handlers[ev]
        return
      }
      handlers[ev] = (handlers[ev] || []).filter((x) => x !== h)
    },
    __handlers: handlers,
  }
})

import SessionSidebar from '../SessionSidebar.vue'
import ForkSessionModal from '../ForkSessionModal.vue'
import SessionChangesPanel from '../logs/SessionChangesPanel.vue'
import { useSessionStore } from '../../stores/session'
import * as sseModule from '../../composables/useSSE'

function sseDispatch(ev: string, data: any) {
  const hs = ((sseModule as any).__handlers[ev] || []) as Array<(d: any) => void>
  hs.forEach((h) => h(data))
}

function sess(id: string, over: Record<string, unknown> = {}) {
  return {
    id,
    channel: 'web',
    startTime: '2026-11-10T01:00:00Z',
    lastTime: '2026-11-10T02:00:00Z',
    messageCount: 3,
    firstMessage: `会话 ${id}`,
    model: '',
    ...over,
  }
}

const P1 = { id: 'p1', name: '项目一', path: 'C:/x/1', created_at: 't', running: true }

let promptSpy: MockInstance<(...args: any[]) => any>

beforeEach(() => {
  setActivePinia(createPinia())
  listMock.mockReset().mockResolvedValue({ sessions: [] })
  deleteMock.mockReset().mockResolvedValue({ deleted: 'x', trashed: true })
  listProjectsMock.mockReset().mockResolvedValue({ projects: [] })
  createMock.mockReset().mockResolvedValue({ session_id: 's9', title: '新对话' })
  renameMock.mockReset().mockResolvedValue({})
  clearMock.mockReset().mockResolvedValue({})
  exportMock.mockReset().mockResolvedValue({ messages: [] })
  turnsMock.mockReset().mockResolvedValue({ turns: [], total_messages: 0, total_turns: 0 })
  removeProjectMock.mockReset().mockResolvedValue({ removed: P1 })
  renameProjectMock.mockReset().mockResolvedValue({ project: { ...P1, name: '改名' } })
  openProjectDirMock.mockReset().mockResolvedValue({ opened: 'C:/x' })
  snoozeMock.mockReset().mockResolvedValue({})
  unsnoozeMock.mockReset().mockResolvedValue({})
  detachMock.mockReset().mockResolvedValue({})
  importSessionMock.mockReset().mockResolvedValue({ imported: 0 })
  requestMock.mockReset().mockRejectedValue(new Error('no ws')) // 默认 config.get 失败 → projectMax 隐藏
  routerPushMock.mockReset()
  vi.spyOn(window, 'confirm').mockReturnValue(true)
  promptSpy = vi.spyOn(window, 'prompt').mockReturnValue(null)
  useToast().toasts.splice(0)
  localStorage.clear()
})

afterEach(() => {
  vi.restoreAllMocks()
})

async function mountWith(sessions: any[], projects: any[] = [], listOver: Record<string, unknown> = {}) {
  listMock.mockResolvedValue({ sessions, ...listOver })
  // 深拷贝项目 fixture：store.renameProject 会就地改 p.name，不能污染共享常量。
  listProjectsMock.mockResolvedValue({ projects: projects.map((p) => ({ ...p })) })
  const w = mount(SessionSidebar)
  await flushPromises()
  return w
}

/** 打开指定会话的行菜单并点击指定文案的菜单项。 */
async function clickRowMenu(w: any, id: string, text: string) {
  await w.find(`.session-item[data-session-id="${id}"] .row-more`).trigger('click')
  const item = w.findAll('.row-menu .menu-item').find((b: any) => b.text().includes(text))
  expect(item, `menu item "${text}" must exist`).toBeTruthy()
  await item.trigger('click')
}

async function clickGroupMenu(w: any, _key: string, text: string) {
  // 本 spec 的组头用例都只挂一个注册项目 → 第一个 .group-header 即目标。
  const target = w.findAll('.group-header')[0]
  await target.find('.group-more').trigger('click')
  const item = target.findAll('.group-menu .menu-item').find((b: any) => b.text().includes(text))
  expect(item, `group menu item "${text}" must exist`).toBeTruthy()
  await item.trigger('click')
  return target
}

// ---------------------------------------------------------------------------
// projects.max 就近编辑 / SSE 刷新 / 行选中 / 新建失败
// ---------------------------------------------------------------------------
describe('SessionSidebar projects.max 与 SSE 刷新', () => {
  it('config.get 成功 → 上限输入可见；setValue 保存 set_field（热生效）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) =>
      cmd === 'get' ? Promise.resolve({ projects: { max: 6 } }) : Promise.resolve({}),
    )
    const w = await mountWith([])
    const input = w.find('[data-test="project-max-input"]')
    expect(input.exists()).toBe(true)
    expect((input.element as HTMLInputElement).value).toBe('6')

    await input.setValue('9')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'projects.max', value: 9 })
    w.unmount()
  })

  it('saveProjectMax 守卫：非法值（<1）复位 4 不发请求；保存失败 toast + 回读', async () => {
    let getCalls = 0
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') {
        getCalls++
        return Promise.resolve({ projects: { max: 6 } })
      }
      if (cmd === 'set_field') return Promise.reject(new Error('disk full'))
      return Promise.resolve({})
    })
    const w = await mountWith([])
    const input = () => w.find('[data-test="project-max-input"]')

    // 非法值 0 → 复位 4、零请求
    await input().setValue('0')
    await flushPromises()
    expect(requestMock.mock.calls.filter((c) => c[1] === 'set_field').length).toBe(0)
    expect((input().element as HTMLInputElement).value).toBe('4')

    // 保存失败 → toast + 重新 loadProjectMax
    const setCalls = requestMock.mock.calls.filter((c) => c[1] === 'set_field').length
    await input().setValue('7')
    await flushPromises()
    expect(requestMock.mock.calls.filter((c) => c[1] === 'set_field').length).toBe(setCalls + 1)
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('项目上限保存失败'))).toBe(true)
    expect(getCalls).toBeGreaterThanOrEqual(2) // 失败后回读
    w.unmount()
  })

  it('config.get 失败 → 上限编辑器隐藏（null 分支）', async () => {
    const w = await mountWith([])
    expect(w.find('[data-test="project-max-input"]').exists()).toBe(false)
    w.unmount()
  })

  it('SSE session.created / session.updated → 强制刷新列表；卸载后不再响应', async () => {
    const w = await mountWith([sess('a1')])
    expect(listMock.mock.calls.length).toBe(1)
    sseDispatch('session.created', {})
    await flushPromises()
    expect(listMock.mock.calls.length).toBe(2)
    sseDispatch('session.updated', {})
    await flushPromises()
    expect(listMock.mock.calls.length).toBe(3)
    w.unmount()
    sseDispatch('session.created', {})
    await flushPromises()
    expect(listMock.mock.calls.length).toBe(3) // off 已注销
  })

  it('非批量模式行点击 → switchTo（currentId 跟随）', async () => {
    const w = await mountWith([sess('a1'), sess('a2')])
    await w.find('.session-item[data-session-id="a2"]').trigger('click')
    expect(useSessionStore().currentId).toBe('a2')
    w.unmount()
  })

  it('新建会话失败（store.create 返回 null）→ toast；组内新建失败同样', async () => {
    createMock.mockRejectedValue(new Error('limit'))
    const w = await mountWith([], [P1])
    await w.find('.fn-new-chat').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('新建会话失败'))).toBe(true)

    await clickGroupMenu(w, 'p1', '＋')
    await flushPromises()
    expect(useToast().toasts.filter((t) => t.message.includes('新建会话失败')).length).toBe(2)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 组折叠持久化 + 脏 localStorage 兜底
// ---------------------------------------------------------------------------
describe('SessionSidebar 组折叠与脏数据兜底', () => {
  it('caret 折叠/展开：会话行随组隐藏，折叠状态持久化 localStorage（F-14）', async () => {
    const w = await mountWith([sess('x1', { projectId: 'p1' })], [P1])
    const caret = w.find('.group-header .caret')
    expect(caret.text()).toBe('▾')
    await caret.trigger('click')
    expect(localStorage.getItem('nb_collapsed_projects')).toBe('["p1"]')
    expect(w.find('.session-item[data-session-id="x1"]').exists()).toBe(false)
    expect(w.find('.group-header .caret').text()).toBe('▸')

    await w.find('.group-header .caret').trigger('click')
    expect(localStorage.getItem('nb_collapsed_projects')).toBe('[]')
    expect(w.find('.session-item[data-session-id="x1"]').exists()).toBe(true)
    w.unmount()
  })

  it('挂载前已折叠的组：直接收起（loadCollapsed 恢复）', async () => {
    localStorage.setItem('nb_collapsed_projects', JSON.stringify(['p1']))
    const w = await mountWith([sess('x1', { projectId: 'p1' })], [P1])
    expect(w.find('.session-item[data-session-id="x1"]').exists()).toBe(false)
    w.unmount()
  })

  it('三个偏好键脏 JSON：挂载不炸、按空集合兜底（catch 分支）', async () => {
    localStorage.setItem('nb_pinned_sessions', '{bad')
    localStorage.setItem('nb_pinned_projects', '[1,')
    localStorage.setItem('nb_collapsed_projects', 'null~~')
    const w = await mountWith([sess('x1', { projectId: 'p1' })], [P1])
    expect(w.find('.session-item[data-session-id="x1"]').exists()).toBe(true)
    expect(w.find('.group-header .pin-flag').exists()).toBe(false)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 项目组头：就地重命名 / 失败反馈
// ---------------------------------------------------------------------------
describe('SessionSidebar 项目重命名与失败反馈', () => {
  it('startRename → 输入框预填现名 → enter 提交 → renameProject 调用 + 组头更新', async () => {
    const w = await mountWith([], [P1])
    await clickGroupMenu(w, 'p1', '重命名')
    const input = w.find('.group-header .rename-input')
    expect(input.exists()).toBe(true)
    expect((input.element as HTMLInputElement).value).toBe('项目一')

    await input.setValue('改名')
    await input.trigger('keyup.enter')
    await flushPromises()
    expect(renameProjectMock).toHaveBeenCalledWith('p1', '改名')
    expect(w.findAll('.group-header .group-name')[0].text()).toContain('改名')
    w.unmount()
  })

  it('重命名失败：toast 透出错误原文；空名不提交；__ 前缀键守卫', async () => {
    renameProjectMock.mockRejectedValue(new Error('db busy'))
    const w = await mountWith([], [P1])
    await clickGroupMenu(w, 'p1', '重命名')
    const input = w.find('.group-header .rename-input')
    await input.setValue('新名')
    await input.trigger('keyup.enter')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('db busy'))).toBe(true)

    // 空名：blur 提交但被守卫拦下（不再发请求）
    await clickGroupMenu(w, 'p1', '重命名')
    const input2 = w.find('.group-header .rename-input')
    await input2.setValue('   ')
    await input2.trigger('blur')
    await flushPromises()
    expect(renameProjectMock).toHaveBeenCalledTimes(1)

    // '__chat' 等内部键守卫（vm 直调）
    const vm = w.vm as any
    vm.renaming = '__chat'
    vm.renameBuf = 'x'
    await vm.commitRename()
    expect(renameProjectMock).toHaveBeenCalledTimes(1)
    w.unmount()
  })

  it('removeProject 失败：toast「移除失败」；openDir 失败：toast「打开目录失败」', async () => {
    // 两个失败臂各校验一种形态：无 message 对象 → 兜底文案；Error → 原文透传。
    removeProjectMock.mockRejectedValueOnce({})
    const w = await mountWith([], [P1])
    await clickGroupMenu(w, 'p1', '移除项目')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('移除失败'))).toBe(true)

    removeProjectMock.mockRejectedValueOnce(new Error('locked'))
    await clickGroupMenu(w, 'p1', '移除项目')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('locked'))).toBe(true)

    openProjectDirMock.mockRejectedValueOnce({})
    await clickGroupMenu(w, 'p1', '打开目录')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('打开目录失败'))).toBe(true)

    openProjectDirMock.mockRejectedValueOnce(new Error('no gui'))
    await clickGroupMenu(w, 'p1', '打开目录')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('no gui'))).toBe(true)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 会话行菜单：导出 / 创建分支 / openChanges 直接失败
// ---------------------------------------------------------------------------
describe('SessionSidebar 导出与分支弹窗', () => {
  it('导出：export → Blob 下载（文件名 session-{id}.json）+ 回收；失败 toast', async () => {
    exportMock.mockResolvedValue({ messages: [{ role: 'user', content: 'hi' }] })
    const origCreate = (URL as any).createObjectURL
    const origRevoke = (URL as any).revokeObjectURL
    ;(URL as any).createObjectURL = vi.fn(() => 'blob:mock-url')
    ;(URL as any).revokeObjectURL = vi.fn()
    try {
      const w = await mountWith([sess('a1')])
      const createSpy = vi.spyOn(document, 'createElement')
      await clickRowMenu(w, 'a1', '导出')
      await flushPromises()
      expect(exportMock).toHaveBeenCalledWith('a1')
      expect(URL.createObjectURL).toHaveBeenCalledTimes(1)
      expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:mock-url')
      const anchors = createSpy.mock.results.map((r) => r.value as any).filter((el) => el?.tagName === 'A')
      expect(anchors.some((a) => a.download === 'session-a1.json')).toBe(true)
      createSpy.mockRestore()

      exportMock.mockRejectedValue(new Error('boom'))
      await clickRowMenu(w, 'a1', '导出')
      await flushPromises()
      expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('导出失败'))).toBe(true)
      w.unmount()
    } finally {
      if (origCreate === undefined) delete (URL as any).createObjectURL
      else (URL as any).createObjectURL = origCreate
      if (origRevoke === undefined) delete (URL as any).revokeObjectURL
      else (URL as any).revokeObjectURL = origRevoke
    }
  })

  it('创建分支：ForkSessionModal 打开（拉轮次）→ 关闭；forked 事件 → 强刷 + 切换新会话', async () => {
    const w = await mountWith([sess('a1')])
    await clickRowMenu(w, 'a1', '创建分支')
    await flushPromises()
    expect(turnsMock).toHaveBeenCalledWith('a1')
    const modal = w.findComponent(ForkSessionModal)
    expect(modal.exists()).toBe(true)
    expect(modal.props('sessionId')).toBe('a1')

    // 关闭
    await modal.find('.close-btn').trigger('click')
    await flushPromises()
    expect(w.findComponent(ForkSessionModal).exists()).toBe(false)

    // 再开 → forked 事件
    await clickRowMenu(w, 'a1', '创建分支')
    await flushPromises()
    w.findComponent(ForkSessionModal).vm.$emit('forked', 'new-sid')
    await flushPromises()
    expect(w.findComponent(ForkSessionModal).exists()).toBe(false)
    expect(listMock.mock.calls.length).toBeGreaterThanOrEqual(2) // force 刷新
    expect(useSessionStore().currentId).toBe('new-sid')
    w.unmount()
  })

  it('openChanges 当前会话直接失败：toast + 弹窗关闭（围栏内分支）', async () => {
    exportMock.mockRejectedValue(new Error('disk'))
    const w = await mountWith([sess('a1')])
    await clickRowMenu(w, 'a1', '变更对比')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('加载会话消息失败'))).toBe(true)
    expect(w.find('.pr-box').exists()).toBe(false)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 导入会话（L22 跨设备移动）
// ---------------------------------------------------------------------------
describe('SessionSidebar 导入会话', () => {
  function stubFiles(w: any, payload: unknown | null) {
    const input = w.find('input[type="file"]')
    const file = payload === null ? null : { text: async () => payload }
    Object.defineProperty(input.element, 'files', { value: file ? [file] : [], configurable: true })
    return input.trigger('change')
  }

  it('「导入」按钮触发隐藏文件选择器（click 转发）', async () => {
    const w = await mountWith([])
    const input = w.find('input[type="file"]').element as HTMLInputElement
    const clickSpy = vi.spyOn(input, 'click')
    await w.find('[data-test="import-toggle"]').trigger('click')
    expect(clickSpy).toHaveBeenCalledTimes(1)
    w.unmount()
  })

  it('数组形态导入成功 → toast 计数 + 强制刷新', async () => {
    importSessionMock.mockResolvedValue({ imported: 3 })
    const w = await mountWith([])
    await stubFiles(w, JSON.stringify([{ role: 'user' }, { role: 'assistant' }, { role: 'user' }]))
    await flushPromises()
    expect(importSessionMock).toHaveBeenCalledWith({ messages: [{ role: 'user' }, { role: 'assistant' }, { role: 'user' }] })
    expect(useToast().toasts.some((t) => t.type === 'success' && t.message.includes('已导入会话（3 条消息）'))).toBe(true)
    expect(listMock.mock.calls.length).toBeGreaterThanOrEqual(2)
    w.unmount()
  })

  it('{messages:[...]} 包装形态同样收（导出产物兼容）', async () => {
    importSessionMock.mockResolvedValue({ imported: 1 })
    const w = await mountWith([])
    await stubFiles(w, JSON.stringify({ messages: [{ role: 'user' }] }))
    await flushPromises()
    expect(importSessionMock).toHaveBeenCalledWith({ messages: [{ role: 'user' }] })
    w.unmount()
  })

  it('非法 JSON / 形态不对 / 导入失败（Error 与字符串两形态）→ 对应 toast', async () => {
    const w = await mountWith([])
    // 非法 JSON
    await stubFiles(w, '{oops')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('文件不是合法 JSON'))).toBe(true)
    // 形态不对
    await stubFiles(w, JSON.stringify({ foo: 1 }))
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('文件格式不对'))).toBe(true)
    // 导入失败（Error）
    importSessionMock.mockRejectedValueOnce(new Error('backend reject'))
    await stubFiles(w, '[]')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('backend reject'))).toBe(true)
    // 导入失败（字符串）
    importSessionMock.mockRejectedValueOnce('plain string')
    await stubFiles(w, '[]')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('plain string'))).toBe(true)
    expect(importSessionMock).toHaveBeenCalledTimes(2) // 形态不对的那次没发请求
    w.unmount()
  })

  it('未选文件（files 空）：零请求', async () => {
    const w = await mountWith([])
    await stubFiles(w, null)
    await flushPromises()
    expect(importSessionMock).not.toHaveBeenCalled()
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// B11 暂停/恢复 + 徽标 + detach
// ---------------------------------------------------------------------------
describe('SessionSidebar 暂停（B11）与徽标', () => {
  const FUTURE = new Date(Date.now() + 10 * 60000).toISOString()

  it('暂停中行渲染：snoozed 弱化 + ⏰ + 💤9+ / 未送达 3 / 🔓 / snoozeTip 备注', async () => {
    const w = await mountWith([
      sess('z1', {
        lastTime: new Date(Date.now() - 30 * 1000).toISOString(), // relTime 刚刚
        snoozedUntil: FUTURE,
        snoozeNote: '跑长任务',
        pendingCount: 12,
        undelivered: 3,
        allow_all: true,
      }),
      sess('z2', { pendingCount: 5, undelivered: 12 }),
    ])
    const z1 = w.find('.session-item[data-session-id="z1"]')
    expect(z1.classes()).toContain('snoozed')
    expect(z1.attributes('title')).toContain('已暂停（10 分钟后自动恢复）')
    expect(z1.attributes('title')).toContain('跑长任务')
    expect(z1.text()).toContain('⏰')
    expect(z1.text()).toContain('💤9+')
    expect(z1.text()).toContain('3')
    expect(z1.text()).toContain('🔓')
    expect(z1.text()).toContain('刚刚')
    // z2：pending 5 → 💤5；undelivered 12 → 9+；无暂停
    const z2 = w.find('.session-item[data-session-id="z2"]')
    expect(z2.classes()).not.toContain('snoozed')
    expect(z2.text()).toContain('💤5')
    expect(z2.text()).toContain('9+')
    w.unmount()
  })

  it('snoozeSession 时长形态：分钟/h/天/中文单位/RFC3339 透传', async () => {
    const w = await mountWith([sess('a1')])
    const run = async (input: string) => {
      snoozeMock.mockReset().mockResolvedValue({})
      promptSpy.mockReturnValueOnce(input)
      await clickRowMenu(w, 'a1', '暂停会话')
      await flushPromises()
      return snoozeMock.mock.calls[0]?.[1]
    }
    expect(await run('45')).toBe('2700')
    expect(await run('2h')).toBe('7200')
    expect(await run('1d')).toBe('86400')
    expect(await run('90 分钟')).toBe('5400')
    expect(await run('2 小时')).toBe('7200')
    expect(await run('3 天')).toBe('259200')
    const iso = await run('2026-12-01T00:00:00Z')
    expect(iso).toBe('2026-12-01T00:00:00.000Z')
    w.unmount()
  })

  it('snoozeSession 守卫与失败：取消/空串/0 → 拦截；垃圾串 → 无法识别；失败 toast', async () => {
    const w = await mountWith([sess('a1')])
    // prompt 取消 / 空串 → 零调用
    promptSpy.mockReturnValueOnce(null)
    await clickRowMenu(w, 'a1', '暂停会话')
    await flushPromises()
    promptSpy.mockReturnValueOnce('   ')
    await clickRowMenu(w, 'a1', '暂停会话')
    await flushPromises()
    expect(snoozeMock).not.toHaveBeenCalled()

    // '0' → 时长必须为正数
    promptSpy.mockReturnValueOnce('0')
    await clickRowMenu(w, 'a1', '暂停会话')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('时长必须为正数'))).toBe(true)
    expect(snoozeMock).not.toHaveBeenCalled()

    // 垃圾串 → 无法识别
    promptSpy.mockReturnValueOnce('垃圾!!!')
    await clickRowMenu(w, 'a1', '暂停会话')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('无法识别的时长或时刻'))).toBe(true)
    expect(snoozeMock).not.toHaveBeenCalled()

    // 失败（Error 与字符串两形态）
    promptSpy.mockReturnValueOnce('30')
    snoozeMock.mockRejectedValueOnce(new Error('busy'))
    await clickRowMenu(w, 'a1', '暂停会话')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('busy'))).toBe(true)
    promptSpy.mockReturnValueOnce('30')
    snoozeMock.mockRejectedValueOnce('plain')
    await clickRowMenu(w, 'a1', '暂停会话')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('plain'))).toBe(true)

    // 成功：toast + 强刷
    snoozeMock.mockResolvedValueOnce({})
    promptSpy.mockReturnValueOnce('30')
    await clickRowMenu(w, 'a1', '暂停会话')
    await flushPromises()
    expect(snoozeMock).toHaveBeenLastCalledWith('a1', '1800')
    expect(useToast().toasts.some((t) => t.type === 'success' && t.message.includes('已暂停「会话 a1」'))).toBe(true)
    expect(listMock.mock.calls.length).toBeGreaterThanOrEqual(2)
    w.unmount()
  })

  it('unsnoozeSession：暂停中的行走「取消暂停」入口；成功/失败', async () => {
    const w = await mountWith([sess('a1', { snoozedUntil: FUTURE, snoozeNote: '备注' })])
    await clickRowMenu(w, 'a1', '取消暂停')
    await flushPromises()
    expect(unsnoozeMock).toHaveBeenCalledWith('a1')
    expect(useToast().toasts.some((t) => t.type === 'success' && t.message.includes('「会话 a1」已恢复'))).toBe(true)
    expect(listMock.mock.calls.length).toBeGreaterThanOrEqual(2)

    // 失败臂：无 message 对象 → 兜底「恢复失败」
    unsnoozeMock.mockRejectedValueOnce({})
    await clickRowMenu(w, 'a1', '取消暂停')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('恢复失败'))).toBe(true)
    w.unmount()
  })

  it('detachSession：confirm 拒绝零调用；确认 → 脱离 + toast；失败 toast', async () => {
    const w = await mountWith([
      sess('child', { parent: 'agent:main:session:p0', parentTitle: '父会话' }),
    ])
    const confirmSpy = vi.mocked(window.confirm)

    confirmSpy.mockReturnValueOnce(false)
    await clickRowMenu(w, 'child', '脱离父分支')
    expect(confirmSpy).toHaveBeenCalledTimes(1)
    expect(detachMock).not.toHaveBeenCalled()

    confirmSpy.mockReturnValueOnce(true)
    await clickRowMenu(w, 'child', '脱离父分支')
    await flushPromises()
    expect(detachMock).toHaveBeenCalledWith('child')
    expect(useToast().toasts.some((t) => t.type === 'success' && t.message.includes('已脱离父分支'))).toBe(true)

    // 失败臂：无 message 对象 → 兜底「脱离失败」
    detachMock.mockRejectedValueOnce({})
    await clickRowMenu(w, 'child', '脱离父分支')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('脱离失败'))).toBe(true)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 会话信息弹窗
// ---------------------------------------------------------------------------
describe('SessionSidebar 会话信息弹窗', () => {
  it('全字段渲染：用量/费用/模型/父分支/谱系/回退/分支摘要；关闭', async () => {
    const w = await mountWith([
      sess('i1', {
        startTime: '2026-11-01T08:00:00Z',
        lastTime: '2026-11-02T09:00:00Z',
        messageCount: 12,
        tokens: 12345,
        cost: 1.23456,
        model: 'glm-x',
        parent: 'agent:main:session:p9',
        parentTitle: '父标题',
        forkedAtTurn: 2,
        forkReason: '试验',
        lineage: { parent_session_id: 'p9', reason: '试验', ancestors: [{ id: 'p0', title: '根' }, { id: 'p9', title: '父标题' }] },
        lastRewind: { at_index: 3, dropped_turns: 2, dropped_rows: 5, ts: '2026-11-02T10:00:00Z' },
        branchSummaryPreview: '摘要预览',
      }),
    ])
    await clickRowMenu(w, 'i1', '会话信息')
    const box = w.find('.info-box')
    expect(box.exists()).toBe(true)
    expect(box.text()).toContain('会话 i1')
    expect(box.text()).toContain('对话') // projectOf：无 projectId → 对话
    expect(box.text()).toContain('12,345') // fmtTokens zh-CN
    expect(box.text()).toContain('$1.2346') // fmtCost
    expect(box.text()).toContain('glm-x')
    expect(box.text()).toContain('父标题 · 第 2 轮 · 缘由：试验')
    expect(box.text()).toContain('↳ 根')
    expect(box.text()).toContain('↳ 父标题')
    expect(box.text()).toContain('· 会话 i1')
    expect(box.text()).toContain('第 3 行起截断')
    expect(box.text()).toContain('弃 2 轮 / 5 行')
    expect(box.text()).toContain('摘要预览')

    await box.find('.info-close').trigger('click')
    expect(w.find('.info-box').exists()).toBe(false)
    w.unmount()
  })

  it('projectOf 三态：注册项目名 / 孤儿「已移除」/ 无归属「对话」；非法时间原样显示', async () => {
    const w = await mountWith(
      [
        sess('r1', { projectId: 'p1' }),
        sess('o1', { projectId: 'gone' }),
        sess('n1', { startTime: 'not-a-date' }),
      ],
      [P1],
    )
    await clickRowMenu(w, 'r1', '会话信息')
    expect(w.find('.info-box').text()).toContain('项目一')
    await w.find('.info-box .info-close').trigger('click')

    await clickRowMenu(w, 'o1', '会话信息')
    expect(w.find('.info-box').text()).toContain('已移除')
    await w.find('.info-box .info-close').trigger('click')

    await clickRowMenu(w, 'n1', '会话信息')
    expect(w.find('.info-box').text()).toContain('not-a-date') // fmtTime isNaN → 原样
    w.unmount()
  })

  it('缺省字段：tokens/cost/model 缺失 → — 兜底', async () => {
    const w = await mountWith([sess('b1', { tokens: undefined, cost: undefined, model: '' })])
    await clickRowMenu(w, 'b1', '会话信息')
    const box = w.find('.info-box')
    expect(box.text()).toContain('—')
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// wf_edit 会话隐藏（localStorage 登记表 + 服务端 bindings 并集）
// ---------------------------------------------------------------------------
describe('SessionSidebar wf_edit 会话隐藏', () => {
  it('本地登记表命中 → 主聊天侧栏隐藏；服务端 wf_agentgen 绑定命中同样隐藏', async () => {
    const { useWfEditSessions } = await import('../../composables/wfEditSessions')
    useWfEditSessions().register('wf-local-1', 'wfA')
    const w = await mountWith(
      [sess('wf-local-1'), sess('wf-bound-2'), sess('normal-3')],
      [],
      { bindings: { 'wf_agentgen:wfA': 'wf-bound-2' } },
    )
    const ids = w.findAll('.session-item').map((s: any) => s.attributes('data-session-id'))
    expect(ids).toEqual(['normal-3'])
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 验证命令只跑本 spec（不并入既有 5 个 SessionSidebar spec）——既有覆盖面
// 在本文件内补齐：顶部路由 / 排序循环与三档 / 会话与项目置顶 / 行操作
// （删除/重命名/清空）/ 组内新建成功 / 批量管理 / 变更对比成功臂 / fork
// 跳转 / Show more。
// ---------------------------------------------------------------------------
describe('SessionSidebar 顶部功能区与排序', () => {
  it('行1 功能按钮 → 路由跳转（定时/Skills/MCP/命令）', async () => {
    const w = await mountWith([])
    const btns = w.findAll('.fn-btn')
    await btns[0].trigger('click')
    expect(routerPushMock).toHaveBeenCalledWith('/tasks')
    await btns[1].trigger('click')
    expect(routerPushMock).toHaveBeenCalledWith('/skills')
    await btns[2].trigger('click')
    expect(routerPushMock).toHaveBeenCalledWith('/mcp')
    await btns[3].trigger('click')
    expect(routerPushMock).toHaveBeenCalledWith('/commands')
    w.unmount()
  })

  it('排序循环按钮：recent → created → name → recent，每步持久化 localStorage', async () => {
    const w = await mountWith([sess('a', { firstMessage: '唯一会话' })])
    expect(w.find('.sort-toggle').text()).toContain('最近')
    await w.find('.sort-toggle').trigger('click')
    expect(localStorage.getItem('nb_session_sort')).toBe('created')
    expect(w.find('.sort-toggle').text()).toContain('创建')
    await w.find('.sort-toggle').trigger('click')
    expect(localStorage.getItem('nb_session_sort')).toBe('name')
    expect(w.find('.sort-toggle').text()).toContain('名称')
    await w.find('.sort-toggle').trigger('click')
    expect(localStorage.getItem('nb_session_sort')).toBe('recent')
    w.unmount()
  })

  it('三档排序键：recent 按 lastTime 降序 / created 按 startTime 降序 / name 按标题字典序', async () => {
    const sessions = [
      sess('a', { firstMessage: 'zeta 会话', lastTime: '2026-09-02T10:00:00Z', startTime: '2026-09-01T10:00:00Z' }),
      sess('b', { firstMessage: 'alpha 会话', lastTime: '2026-09-03T10:00:00Z', startTime: '2026-09-02T10:00:00Z' }),
      sess('c', { firstMessage: 'mid 会话', lastTime: '2026-09-01T10:00:00Z', startTime: '2026-09-03T10:00:00Z' }),
    ]
    const titles = (w: any) => w.findAll('.session-item .session-title').map((t: any) => t.text().trim())

    let w = await mountWith(sessions)
    expect(titles(w)).toEqual(['alpha 会话', 'zeta 会话', 'mid 会话']) // lastTime 降序
    w.unmount()

    localStorage.setItem('nb_session_sort', 'created')
    w = await mountWith(sessions)
    expect(titles(w)).toEqual(['mid 会话', 'alpha 会话', 'zeta 会话']) // startTime 降序
    w.unmount()

    localStorage.setItem('nb_session_sort', 'name')
    w = await mountWith(sessions)
    expect(titles(w)).toEqual(['alpha 会话', 'mid 会话', 'zeta 会话']) // zh-CN 字典序
    w.unmount()
  })

  it('Show more：>8 条截 8 + footer 计数；展开/收起（expandedGroups）', async () => {
    const sessions = Array.from({ length: 10 }, (_, i) => sess(`s${i}`))
    const w = await mountWith(sessions)
    expect(w.findAll('.session-item')).toHaveLength(8)
    expect(w.findAll('.show-more')).toHaveLength(1)
    expect(w.find('.show-more').text()).toContain('显示更多 (2)')
    await w.find('.show-more').trigger('click')
    expect(w.findAll('.session-item')).toHaveLength(10)
    expect(w.find('.show-more').text()).toContain('收起')
    await w.find('.show-more').trigger('click')
    expect(w.findAll('.session-item')).toHaveLength(8)
    w.unmount()
  })
})

describe('SessionSidebar 会话置顶与项目置顶', () => {
  it('会话 pin：菜单置顶 → 📌 + 组内置顶 + localStorage；取消置顶还原', async () => {
    const w = await mountWith([
      sess('old', { firstMessage: '旧会话', lastTime: '2026-09-01T10:00:00Z' }),
      sess('new', { firstMessage: '新会话', lastTime: '2026-09-02T10:00:00Z' }),
    ])
    // 默认 recent：新会话在前
    expect(w.findAll('.session-item')[0].attributes('data-session-id')).toBe('new')
    expect(w.find('.pin-flag').exists()).toBe(false)

    await clickRowMenu(w, 'old', '置顶')
    await flushPromises()
    expect(JSON.parse(localStorage.getItem('nb_pinned_sessions') || '[]')).toEqual(['old'])
    // pinned 置顶（组内），带 📌 标记
    expect(w.findAll('.session-item')[0].attributes('data-session-id')).toBe('old')
    expect(w.find('.session-item .pin-flag').exists()).toBe(true)

    // 取消置顶 → 回排序键序
    await clickRowMenu(w, 'old', '取消置顶')
    await flushPromises()
    expect(JSON.parse(localStorage.getItem('nb_pinned_sessions') || '[]')).toEqual([])
    expect(w.findAll('.session-item')[0].attributes('data-session-id')).toBe('new')
    w.unmount()
  })

  it('项目 pin：组头菜单置顶项目 → 排到项目区最前 + 📌；取消置顶还原注册序', async () => {
    const P2 = { id: 'p2', name: '项目二', path: 'C:/x/2', running: true }
    const w = await mountWith([], [P1, P2])
    const names = () => w.findAll('.group-header .group-name').map((n: any) => n.text().trim())
    expect(names()).toEqual(['项目一', '项目二'])

    const headers = w.findAll('.group-header')
    await headers[1].find('.group-more').trigger('click')
    await headers[1].findAll('.group-menu .menu-item').find((b: any) => b.text().includes('置顶项目'))!.trigger('click')
    await flushPromises()
    expect(JSON.parse(localStorage.getItem('nb_pinned_projects') || '[]')).toEqual(['p2'])
    expect(names()).toEqual(['项目二', '项目一'])
    expect(w.find('.group-header .pin-flag').exists()).toBe(true)

    await w.findAll('.group-header')[0].find('.group-more').trigger('click')
    await w.findAll('.group-header')[0].findAll('.group-menu .menu-item').find((b: any) => b.text().includes('取消置顶'))!.trigger('click')
    await flushPromises()
    expect(JSON.parse(localStorage.getItem('nb_pinned_projects') || '[]')).toEqual([])
    expect(names()).toEqual(['项目一', '项目二'])
    w.unmount()
  })

  it('移除项目成功：组头消失、会话落孤儿组；置顶项目顺带清 pin', async () => {
    localStorage.setItem('nb_pinned_projects', JSON.stringify(['p1']))
    const w = await mountWith([sess('x1', { projectId: 'p1' })], [P1])
    expect(w.find('.group-header .pin-flag').exists()).toBe(true)
    await clickGroupMenu(w, 'p1', '移除项目')
    await flushPromises()
    expect(removeProjectMock).toHaveBeenCalledWith('p1')
    expect(localStorage.getItem('nb_pinned_projects')).toBe('[]') // unpinProjectIfRemoved
    expect(w.findAll('.group-header .group-name').map((n: any) => n.text().trim())).toEqual(['已移除'])
    expect(w.find('.session-item[data-session-id="x1"]').exists()).toBe(true)
    w.unmount()
  })
})

describe('SessionSidebar 会话行操作（删除/重命名/清空/组内新建）', () => {
  it('del：confirm → store.remove → 行消失；pinned 会话删除后 pin 顺带清', async () => {
    localStorage.setItem('nb_pinned_sessions', JSON.stringify(['a1']))
    const w = await mountWith([sess('a1')])
    await clickRowMenu(w, 'a1', '删除会话')
    await flushPromises()
    expect(deleteMock).toHaveBeenCalledWith('a1', undefined, undefined)
    expect(JSON.parse(localStorage.getItem('nb_pinned_sessions') || '[]')).toEqual([]) // unpinIfDeleted
    expect(w.find('.session-item[data-session-id="a1"]').exists()).toBe(false)
    w.unmount()
  })

  it('del：confirm 拒绝 → 零删除调用；删除失败 → toast + 行与 pin 保留', async () => {
    const w = await mountWith([sess('a1')])
    vi.mocked(window.confirm).mockReturnValueOnce(false)
    await clickRowMenu(w, 'a1', '删除会话')
    await flushPromises()
    expect(deleteMock).not.toHaveBeenCalled()
    expect(w.find('.session-item[data-session-id="a1"]').exists()).toBe(true)

    deleteMock.mockRejectedValueOnce(new Error('backend down'))
    await clickRowMenu(w, 'a1', '删除会话')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('删除会话失败'))).toBe(true)
    expect(w.find('.session-item[data-session-id="a1"]').exists()).toBe(true)
    w.unmount()
  })

  it('renameSession：prompt 命名 → 行标题更新；取消/空串零调用；失败 toast', async () => {
    const w = await mountWith([sess('a1')])
    // 取消（null）
    promptSpy.mockReturnValueOnce(null)
    await clickRowMenu(w, 'a1', '重命名')
    await flushPromises()
    expect(renameMock).not.toHaveBeenCalled()
    // 空串
    promptSpy.mockReturnValueOnce('   ')
    await clickRowMenu(w, 'a1', '重命名')
    await flushPromises()
    expect(renameMock).not.toHaveBeenCalled()
    // 成功
    promptSpy.mockReturnValueOnce('新标题')
    await clickRowMenu(w, 'a1', '重命名')
    await flushPromises()
    expect(renameMock).toHaveBeenCalledWith('a1', '新标题')
    expect(w.find('.session-item[data-session-id="a1"]').text()).toContain('新标题')
    // 失败
    renameMock.mockRejectedValueOnce(new Error('db down'))
    promptSpy.mockReturnValueOnce('再改')
    await clickRowMenu(w, 'a1', '重命名')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('重命名失败'))).toBe(true)
    w.unmount()
  })

  it('clearSession：confirm → api.clear；拒绝零调用；失败 toast', async () => {
    const w = await mountWith([sess('a1')])
    vi.mocked(window.confirm).mockReturnValueOnce(false)
    await clickRowMenu(w, 'a1', '清空消息')
    expect(clearMock).not.toHaveBeenCalled()

    await clickRowMenu(w, 'a1', '清空消息')
    await flushPromises()
    expect(clearMock).toHaveBeenCalledWith('a1')

    clearMock.mockRejectedValueOnce(new Error('locked'))
    await clickRowMenu(w, 'a1', '清空消息')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('清空失败'))).toBe(true)
    w.unmount()
  })

  it('组内「＋」就地新建成功：乐观行落组内 + currentId 跟随', async () => {
    const w = await mountWith([], [P1])
    const header = w.findAll('.group-header')[0]
    await header.find('.add-in-group').trigger('click')
    await flushPromises()
    expect(createMock).toHaveBeenCalledWith(undefined, 'p1')
    expect(w.findAll('.session-item .session-title').map((t: any) => t.text().trim())).toEqual(['新对话'])
    expect(useSessionStore().currentId).toBe('s9')
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 批量管理（验证命令只跑本 spec → 既有 batch spec 覆盖面在此补齐）
// ---------------------------------------------------------------------------
describe('SessionSidebar 批量管理', () => {
  it('进入批量模式：工具条出现、行首勾选框可见、行菜单隐藏；行点击=勾选切换', async () => {
    const w = await mountWith([sess('a1'), sess('a2')])
    expect(w.find('[data-test="batch-toolbar"]').exists()).toBe(false)
    expect(w.find('[data-test="batch-check"]').exists()).toBe(false)
    expect(w.find('.row-more').exists()).toBe(true)

    await w.find('[data-test="batch-toggle"]').trigger('click')
    expect(w.find('[data-test="batch-toolbar"]').exists()).toBe(true)
    expect(w.findAll('[data-test="batch-check"]').length).toBe(2)
    expect(w.find('.row-more').exists()).toBe(false)
    expect(w.find('[data-test="batch-select-chat"]').exists()).toBe(true)

    const row = w.find('.session-item[data-session-id="a1"]')
    await row.trigger('click')
    expect(row.classes()).toContain('picked')
    expect(w.find('.session-item[data-session-id="a1"]').text()).toContain('☑')
    expect(useSessionStore().currentId).toBeNull() // 批量模式不切换会话

    // 取消 → 退出模式
    await w.find('[data-test="batch-cancel"]').trigger('click')
    expect(w.find('[data-test="batch-toolbar"]').exists()).toBe(false)
    expect(w.find('[data-test="batch-check"]').exists()).toBe(false)
    expect(deleteMock).not.toHaveBeenCalled()
    w.unmount()
  })

  it('全选链路：对话区全选 / 组头全选本组（含孤儿）/ 工具条全选', async () => {
    const w = await mountWith(
      [sess('a1'), sess('a2'), sess('o1', { projectId: 'gone-p' })],
      [{ id: 'p1', name: 'MyProj', path: 'C:\\x', running: true }],
    )
    await w.find('[data-test="batch-toggle"]').trigger('click')

    await w.find('[data-test="batch-select-chat"]').trigger('click')
    expect(w.find('.session-item[data-session-id="a1"]').classes()).toContain('picked')
    expect(w.find('.session-item[data-session-id="o1"]').classes()).not.toContain('picked')

    const orphanSelect = w.find('.group-header.orphan [data-test="batch-select-group"]')
    expect(orphanSelect.exists()).toBe(true)
    await orphanSelect.trigger('click')
    expect(w.find('.session-item[data-session-id="o1"]').classes()).toContain('picked')
    expect(w.find('[data-test="batch-delete"]').text()).toContain('删除所选(3)')

    await w.find('[data-test="batch-select-all"]').trigger('click')
    expect(w.find('[data-test="batch-delete"]').text()).toContain('删除所选(3)')
    w.unmount()
  })

  it('批量删除：confirm → 逐条顺序 delete(source=batch) → success toast + 强刷 + 退出', async () => {
    listMock
      .mockReset()
      .mockResolvedValueOnce({ sessions: [sess('a1'), sess('a2')] })
      .mockResolvedValue({ sessions: [] })
    const w = mount(SessionSidebar)
    await flushPromises()
    await w.find('[data-test="batch-toggle"]').trigger('click')
    await w.find('.session-item[data-session-id="a1"]').trigger('click')
    await w.find('.session-item[data-session-id="a2"]').trigger('click')

    await w.find('[data-test="batch-delete"]').trigger('click')
    await flushPromises()
    expect(window.confirm).toHaveBeenCalledTimes(1)
    expect(deleteMock).toHaveBeenNthCalledWith(1, 'a1', undefined, 'batch')
    expect(deleteMock).toHaveBeenNthCalledWith(2, 'a2', undefined, 'batch')
    expect(w.find('[data-test="batch-toolbar"]').exists()).toBe(false)
    expect(useToast().toasts.some((t) => t.type === 'success' && t.message.includes('已将 2 个会话移入回收站'))).toBe(true)
    w.unmount()
  })

  it('批量删除：单条失败不中断 → 汇总 error toast；confirm 拒绝 → 零调用', async () => {
    deleteMock.mockImplementation((id: string) =>
      id === 'b1' ? Promise.reject('backend boom') : Promise.resolve({ deleted: id, trashed: true }),
    )
    const w = await mountWith([sess('a1'), sess('b1')])
    await w.find('[data-test="batch-toggle"]').trigger('click')
    await w.find('.session-item[data-session-id="a1"]').trigger('click')
    await w.find('.session-item[data-session-id="b1"]').trigger('click')
    await w.find('[data-test="batch-delete"]').trigger('click')
    await flushPromises()
    expect(deleteMock).toHaveBeenCalledTimes(2)
    const err = useToast().toasts.find((t) => t.type === 'error')
    expect(err).toBeTruthy()
    expect(err!.message).toContain('成功 1 个')
    expect(err!.message).toContain('失败 1 个')
    expect(err!.message).toContain('b1')
    w.unmount()
  })

  it('批量删除：confirm 拒绝 → 零调用、模式与选择保留', async () => {
    // 独立用例：store.fetchList 有 5s 缓存，同用例二次挂载会读到旧列表。
    vi.mocked(window.confirm).mockReturnValueOnce(false)
    const w = await mountWith([sess('c1')])
    await w.find('[data-test="batch-toggle"]').trigger('click')
    await w.find('.session-item[data-session-id="c1"]').trigger('click')
    expect(w.find('.session-item[data-session-id="c1"]').classes()).toContain('picked')
    await w.find('[data-test="batch-delete"]').trigger('click')
    await flushPromises()
    expect(deleteMock).not.toHaveBeenCalled()
    expect(w.find('[data-test="batch-toolbar"]').exists()).toBe(true)
    expect(w.find('.session-item[data-session-id="c1"]').classes()).toContain('picked')
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 变更对比成功臂（hasPrChanges 两分支）
// ---------------------------------------------------------------------------
describe('SessionSidebar 变更对比成功臂', () => {
  it('有 file_changes：SessionChangesPanel 收到 messages，无空态提示', async () => {
    exportMock.mockResolvedValue({
      messages: [{ role: 'assistant', content: 'x', file_changes: [{ path: 'a.txt', kind: 'write' }] }],
    })
    const w = await mountWith([sess('a1')])
    await clickRowMenu(w, 'a1', '变更对比')
    await flushPromises()
    expect(w.find('.pr-box').exists()).toBe(true)
    expect(w.findComponent(SessionChangesPanel).props('messages')).toEqual([
      { role: 'assistant', content: 'x', file_changes: [{ path: 'a.txt', kind: 'write' }] },
    ])
    expect(w.find('.pr-box').text()).not.toContain('本会话没有文件变更记录')
    w.unmount()
  })

  it('零变更：诚实空态提示行', async () => {
    exportMock.mockResolvedValue({ messages: [{ role: 'user', content: '纯文本' }] })
    const w = await mountWith([sess('a1')])
    await clickRowMenu(w, 'a1', '变更对比')
    await flushPromises()
    expect(w.find('.pr-box').exists()).toBe(true)
    expect(w.find('.pr-box').text()).toContain('本会话没有文件变更记录')
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 谱系树行 + fork 跳转（arrangeLineage / lineageDepth / goParent）
// ---------------------------------------------------------------------------
describe('SessionSidebar 谱系树与 fork 跳转', () => {
  it('子会话紧跟父会话 + 缩进；点击血缘行跳转父会话', async () => {
    const w = await mountWith([
      sess('ch', { firstMessage: '子会话', parent: 'agent:main:session:pa', parentTitle: '父会话', forkedAtTurn: 1 }),
      sess('pa', { firstMessage: '父会话' }),
    ])
    const rows = w.findAll('.session-item')
    expect(rows[0].attributes('data-session-id')).toBe('pa') // 树排：父在前
    expect(rows[1].attributes('data-session-id')).toBe('ch')
    expect(rows[1].classes()).toContain('branched') // lineageDepth > 0

    const line = rows[1].find('.session-fork-line')
    expect(line.classes()).toContain('link') // 父在列表 → 可点
    await line.trigger('click')
    expect(useSessionStore().currentId).toBe('pa') // goParent
    w.unmount()
  })
})
