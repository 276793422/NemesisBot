import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

// W2 P3 前端（2026-08-31）：看板页签的 dispatch 级测试。
// - BoardKanban：列渲染 + 拖拽 → issue.move payload + 非法转移前端拦截 + 点击卡片开详情；
// - InboxPanel：列表/未读徽标/单条已读/全部已读 + dispatch_failed 徽标（P4）；
// - ProjectPanel：创建 payload + 归档走 project.update status=archived；
// - IssueDetailModal：评论线程（parent_id 一层）+ 回复自带 @作者 + 提交带 parent_id；
// - AutopilotPanel（P4）：创建/编辑/启停/立即运行/删除/run 历史。
// 后端行为由 crates/nemesis-web/src/handlers/board/tests.rs 钉住（后端唯一真相源）。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import BoardKanban from '../BoardKanban.vue'
import InboxPanel from '../InboxPanel.vue'
import ProjectPanel from '../ProjectPanel.vue'
import IssueDetailModal from '../IssueDetailModal.vue'
import AutopilotPanel from '../AutopilotPanel.vue'
import BoardTabs from '../BoardTabs.vue'
import IssueListView from '../../../views/IssueListView.vue'

function issue(over: Record<string, unknown> = {}) {
  return {
    id: 1,
    number: 'NB-1',
    title: '任务一',
    description: '',
    status: 'backlog',
    priority: 1,
    assignee: null,
    assignee_id: null,
    creator: { kind: 'admin', id: 'admin' },
    project_id: null,
    due_date: null,
    position: 1,
    acceptance_criteria: null,
    origin: null,
    created_at: 1700000000,
    updated_at: 1700000000,
    ...over,
  }
}

const emptyIssueR = { issue: issue() }

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'issue.list') return Promise.resolve({ issues: [], total: 0 })
    if (cmd === 'project.list') return Promise.resolve({ projects: [] })
    if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
    if (cmd === 'inbox.list') return Promise.resolve({ notifications: [], unread: 0 })
    if (cmd === 'issue.get') return Promise.resolve(emptyIssueR)
    if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
    return Promise.resolve({})
  })
})

describe('BoardKanban（看板）', () => {
  async function mountKanban(issues: any[]) {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.list') return Promise.resolve({ issues, total: issues.length })
      if (cmd === 'project.list') return Promise.resolve({ projects: [] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      if (cmd === 'issue.get') return Promise.resolve({ issue: issues[0] })
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      return Promise.resolve({})
    })
    const w = mount(BoardKanban)
    await flushPromises()
    return w
  }

  it('渲染 7 列，issue 落在正确列', async () => {
    const w = await mountKanban([
      issue({ status: 'backlog' }),
      issue({ id: 2, number: 'NB-2', title: '任务二', status: 'in_progress' }),
    ])
    const cols = w.findAll('.kanban-col')
    expect(cols.length).toBe(7)
    expect(cols[0].text()).toContain('NB-1')
    expect(cols[2].text()).toContain('NB-2')
    expect(cols[0].text()).not.toContain('NB-2')
  })

  it('拖拽到目标列 → issue.move（追加末尾 position）', async () => {
    const w = await mountKanban([
      issue({ status: 'backlog', position: 1 }),
      issue({ id: 2, number: 'NB-2', title: '任务二', status: 'todo', position: 1 }),
    ])
    // 拖 backlog 的 NB-1 → 丢到 Todo 列空白处（列内 maxPos=1 → 追加 position=2）。
    const card = w.findAll('.kanban-card').find((c) => c.text().includes('NB-1'))!
    await card.trigger('dragstart')
    const todoCol = w.findAll('.kanban-col')[1]
    await todoCol.trigger('drop')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'issue.move')!
    expect(call[2]).toEqual({ id: 1, status: 'todo', position: 2 })
  })

  it('非法转移前端拦截（done 卡片拖去 backlog → warn，不发 issue.move）', async () => {
    const w = await mountKanban([issue({ status: 'done' })])
    const card = w.findAll('.kanban-card')[0]
    await card.trigger('dragstart')
    const backlogCol = w.findAll('.kanban-col')[0]
    await backlogCol.trigger('drop')
    await flushPromises()
    expect(requestMock.mock.calls.some((c) => c[1] === 'issue.move')).toBe(false)
    expect(useToast().toasts.some((t) => t.type === 'warn')).toBe(true)
  })

  it('点击卡片 → 打开共享详情弹窗（issue.get 被调）', async () => {
    const w = await mountKanban([issue()])
    await w.findAll('.kanban-card')[0].trigger('click')
    await flushPromises()
    expect(w.find('.modal-backdrop').exists()).toBe(true)
    expect(requestMock.mock.calls.some((c) => c[1] === 'issue.get')).toBe(true)
  })
})

describe('InboxPanel（收件箱）', () => {
  it('渲染通知 + 未读数；点击未读条目 → inbox.mark_read {id}', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'inbox.list')
        return Promise.resolve({
          notifications: [
            { id: 1, recipient: { kind: 'admin', id: 'admin' }, kind: 'commented', title: 'NB-1 新评论', content: '正文', issue_id: 1, read: false, created_at: 1700000000 },
            { id: 2, recipient: { kind: 'admin', id: 'admin' }, kind: 'assigned', title: 'NB-2 指派', content: '', issue_id: 2, read: true, created_at: 1700000001 },
          ],
          unread: 1,
        })
      return Promise.resolve({ marked: 1, unread: 0 })
    })
    const w = mount(InboxPanel)
    await flushPromises()
    expect(w.text()).toContain('NB-1 新评论')
    expect(w.text()).toContain('未读 1')

    await w.findAll('.inbox-item')[0].trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'inbox.mark_read')!
    expect(call[2]).toEqual({ id: 1 })
  })

  it('全部已读 → inbox.mark_read {all:true}', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'inbox.list')
        return Promise.resolve({
          notifications: [
            { id: 1, recipient: { kind: 'admin', id: 'admin' }, kind: 'mentioned', title: 't', content: 'c', issue_id: null, read: false, created_at: 1700000000 },
          ],
          unread: 1,
        })
      return Promise.resolve({ marked: 1, unread: 0 })
    })
    const w = mount(InboxPanel)
    await flushPromises()
    await w.findAll('button').find((b) => b.text().includes('全部已读'))!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'inbox.mark_read')!
    expect(call[2]).toEqual({ all: true })
  })

  it('P4 dispatch_failed 通知 → 「派发失败」徽标（badge-error）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'inbox.list')
        return Promise.resolve({
          notifications: [
            { id: 1, recipient: { kind: 'admin', id: 'admin' }, kind: 'dispatch_failed', title: 'NB-3 派发失败', content: '派发超时（3600s 无回报）', issue_id: 3, read: false, created_at: 1700000000 },
          ],
          unread: 1,
        })
      return Promise.resolve({})
    })
    const w = mount(InboxPanel)
    await flushPromises()
    expect(w.text()).toContain('派发失败')
    expect(w.find('.badge-error').exists()).toBe(true)
    expect(w.text()).toContain('NB-3 派发失败')
  })
})

describe('ProjectPanel（项目）', () => {
  function proj(over: Record<string, unknown> = {}) {
    return { id: 1, name: '主项目', description: 'd', status: 'active', icon: '🚀', created_at: 1700000000, ...over }
  }

  it('创建项目 → project.create payload', async () => {
    const w = mount(ProjectPanel)
    await flushPromises()
    await w.findAll('button').find((b) => b.text().includes('新建项目'))!.trigger('click')
    const input = w.findAll('input.form-input').find((i) => i.attributes('placeholder')?.includes('项目名'))!
    await input.setValue('新项目')
    await w.findAll('button').find((b) => b.text() === '创建')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'project.create')!
    expect(call[2].name).toBe('新项目')
  })

  it('归档 → project.update status=archived；已归档显示恢复按钮', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'project.list') return Promise.resolve({ projects: [proj()] })
      return Promise.resolve({})
    })
    const w = mount(ProjectPanel)
    await flushPromises()
    await w.findAll('button').find((b) => b.text() === '归档')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.find((c) => c[1] === 'project.update')![2]).toEqual({ id: 1, status: 'archived' })

    // 已归档项目 → 恢复按钮
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'project.list') return Promise.resolve({ projects: [proj({ status: 'archived' })] })
      return Promise.resolve({})
    })
    const w2 = mount(ProjectPanel)
    await flushPromises()
    expect(w2.text()).toContain('已归档')
    expect(w2.findAll('button').some((b) => b.text() === '恢复')).toBe(true)
  })
})

describe('IssueDetailModal（详情弹窗 P3 增强）', () => {
  function detailIssue(comments: any[]) {
    return { ...issue(), comments, activity: [], subscribers: [{ subscriber: { kind: 'admin', id: 'alice' } }] }
  }

  async function mountModal(comments: any[]) {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: detailIssue(comments) })
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      return Promise.resolve({})
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    return w
  }

  it('评论线程：回复（parent_id）嵌套在父评论下', async () => {
    const w = await mountModal([
      { id: 10, author: { kind: 'admin', id: 'alice' }, content: '顶层', parent_id: null, ctype: 'comment', created_at: 1700000000 },
      { id: 11, author: { kind: 'worker', id: 'node-b' }, content: '这是回复', parent_id: 10, ctype: 'comment', created_at: 1700000001 },
    ])
    const parent = w.findAll('.comment-item').find((c) => c.text().includes('顶层'))!
    expect(parent.text()).toContain('这是回复')
    // 顶层计数不含回复
    expect(w.text()).toContain('评论（1）')
  })

  it('回复评论：textarea 预填 @作者，提交带 parent_id', async () => {
    const w = await mountModal([
      { id: 10, author: { kind: 'admin', id: 'alice' }, content: '顶层', parent_id: null, ctype: 'comment', created_at: 1700000000 },
    ])
    await w.findAll('button').find((b) => b.text() === '回复')!.trigger('click')
    const textarea = w.find('textarea.form-textarea')
    expect((textarea.element as HTMLTextAreaElement).value.startsWith('@alice ')).toBe(true)
    await textarea.setValue('@alice 收到')
    await w.findAll('button').find((b) => b.text() === '发表评论')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'comment.add')!
    expect(call[2]).toEqual({ issue_id: 1, content: '@alice 收到', parent_id: 10 })
  })

  it('@提及辅助：点击候选 → textarea 插入 @id', async () => {
    const w = await mountModal([])
    const btn = w.findAll('.mention-row button').find((b) => b.text() === '@alice')!
    await btn.trigger('click')
    const textarea = w.find('textarea.form-textarea')
    expect((textarea.element as HTMLTextAreaElement).value).toBe('@alice ')
  })

  it('附件：列表渲染 + 上传按钮存在', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.get') return Promise.resolve({ issue: detailIssue([]) })
      if (cmd === 'attachment.list')
        return Promise.resolve({
          attachments: [
            { id: 5, issue_id: 1, filename: 'log.txt', storage_path: 'board/files/issue_1/1_log.txt', size: 2048, uploaded_by: { kind: 'admin', id: 'admin' }, created_at: 1700000000 },
          ],
        })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      return Promise.resolve({})
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    expect(w.text()).toContain('log.txt')
    expect(w.text()).toContain('2.0 KB')
    expect(w.findAll('button').some((b) => b.text() === '下载')).toBe(true)
    expect(w.text()).toContain('上传附件')
  })
})

describe('AutopilotPanel（自动化 P4）', () => {
  function ap(over: Record<string, unknown> = {}) {
    return {
      id: 1,
      name: '每日站会',
      title: '每日站会 {date}',
      cron: '0 9 * * *',
      description: '',
      priority: 1,
      project_id: null,
      target: '',
      enabled: true,
      cron_job_id: 'job-1',
      last_run_at: null,
      created_at: 1700000000,
      updated_at: 1700000000,
      ...over,
    }
  }

  async function mountPanel(aps: any[]) {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'autopilot.list') return Promise.resolve({ autopilots: aps })
      return Promise.resolve({})
    })
    const w = mount(AutopilotPanel)
    await flushPromises()
    return w
  }

  it('渲染规则列表（cron/目标/状态/从未运行）', async () => {
    const w = await mountPanel([ap()])
    expect(w.text()).toContain('每日站会')
    expect(w.text()).toContain('0 9 * * *')
    expect(w.text()).toContain('仅建单')
    expect(w.text()).toContain('从未运行')
  })

  it('创建 → autopilot.create payload（含 cron/title/target）', async () => {
    const w = await mountPanel([])
    await w.findAll('button').find((b) => b.text().includes('新建规则'))!.trigger('click')
    const inputs = w.findAll('input.form-input')
    await inputs[0].setValue('周报整理') // 规则名
    await inputs[1].setValue('0 18 * * 5') // cron
    await inputs[2].setValue('周报整理 {date}') // 标题模板
    await inputs[3].setValue('node-b') // 派发目标
    await w.findAll('button').find((b) => b.text() === '创建')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'autopilot.create')!
    expect(call[2].name).toBe('周报整理')
    expect(call[2].cron).toBe('0 18 * * 5')
    expect(call[2].title).toBe('周报整理 {date}')
    expect(call[2].target).toBe('node-b')
    expect(call[2].enabled).toBe(true)
  })

  it('编辑 → 预填 + autopilot.update 带 id', async () => {
    const w = await mountPanel([ap()])
    await w.findAll('button').find((b) => b.text() === '编辑')!.trigger('click')
    const inputs = w.findAll('input.form-input')
    expect((inputs[0].element as HTMLInputElement).value).toBe('每日站会')
    await inputs[0].setValue('每日站会 v2')
    await w.findAll('button').find((b) => b.text() === '保存')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'autopilot.update')!
    expect(call[2].id).toBe(1)
    expect(call[2].name).toBe('每日站会 v2')
  })

  it('启停 → autopilot.update {id, enabled:false}', async () => {
    const w = await mountPanel([ap()])
    await w.findAll('button').find((b) => b.text() === '停用')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'autopilot.update')!
    expect(call[2]).toEqual({ id: 1, enabled: false })
  })

  it('立即运行 → autopilot.run；无派发目标 toast「未配置派发目标」', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'autopilot.list') return Promise.resolve({ autopilots: [ap()] })
      if (cmd === 'autopilot.run')
        return Promise.resolve({ ran: true, issue_id: 9, issue_number: 'NB-9', dispatch: null })
      return Promise.resolve({})
    })
    const w = mount(AutopilotPanel)
    await flushPromises()
    await w.findAll('button').find((b) => b.text() === '立即运行')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.find((c) => c[1] === 'autopilot.run')![2]).toEqual({ id: 1 })
    expect(useToast().toasts.some((t) => t.type === 'success' && t.message.includes('NB-9'))).toBe(true)
    expect(useToast().toasts.some((t) => t.message.includes('未配置派发目标'))).toBe(true)
  })

  it('立即运行（有派发）→ toast「已建单并派发」', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'autopilot.list') return Promise.resolve({ autopilots: [ap({ target: 'node-b' })] })
      if (cmd === 'autopilot.run')
        return Promise.resolve({ ran: true, issue_id: 9, issue_number: 'NB-9', dispatch: { dispatched: true, task_id: 't1' } })
      return Promise.resolve({})
    })
    const w = mount(AutopilotPanel)
    await flushPromises()
    await w.findAll('button').find((b) => b.text() === '立即运行')!.trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'success' && t.message.includes('并派发'))).toBe(true)
  })

  it('删除（confirm）→ autopilot.remove {id}', async () => {
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true)
    const w = await mountPanel([ap()])
    await w.findAll('button').find((b) => b.text() === '删除')!.trigger('click')
    await flushPromises()
    expect(confirmSpy).toHaveBeenCalled()
    expect(requestMock.mock.calls.find((c) => c[1] === 'autopilot.remove')![2]).toEqual({ id: 1 })
    confirmSpy.mockRestore()
  })

  it('删除（confirm 取消）→ 不发请求', async () => {
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false)
    const w = await mountPanel([ap()])
    await w.findAll('button').find((b) => b.text() === '删除')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some((c) => c[1] === 'autopilot.remove')).toBe(false)
    confirmSpy.mockRestore()
  })

  it('run 历史 → autopilot.runs {id}，渲染 issue 号 + 状态徽标', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'autopilot.list') return Promise.resolve({ autopilots: [ap()] })
      if (cmd === 'autopilot.runs')
        return Promise.resolve({
          issues: [
            { id: 9, number: 'NB-9', title: '每日站会 2026-08-31', status: 'in_progress', created_at: 1700000000 },
            { id: 8, number: 'NB-8', title: '每日站会 2026-08-30', status: 'done', created_at: 1699913600 },
          ],
        })
      return Promise.resolve({})
    })
    const w = mount(AutopilotPanel)
    await flushPromises()
    await w.findAll('button').find((b) => b.text() === '历史')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.find((c) => c[1] === 'autopilot.runs')![2]).toEqual({ id: 1 })
    expect(w.text()).toContain('NB-9')
    expect(w.text()).toContain('每日站会 2026-08-31')
    expect(w.text()).toContain('进行中')
  })
})

describe('BoardTabs（页签顺序）', () => {
  it('从左到右 = 使用依赖链：项目 → 列表 → 看板 → 收件箱 → 自动化', () => {
    const w = mount(BoardTabs, { props: { modelValue: 'projects' } })
    const labels = w.findAll('button').map((b) => b.text())
    expect(labels).toEqual(['项目', '列表', '看板', '收件箱', '自动化'])
  })

  it('点击页签 emit update:modelValue', async () => {
    const w = mount(BoardTabs, { props: { modelValue: 'projects' } })
    await w.findAll('button').find((b) => b.text() === '看板')!.trigger('click')
    expect(w.emitted('update:modelValue')![0]).toEqual(['kanban'])
  })
})

describe('IssueListView（列表）', () => {
  it('回归：加载完成后 loading 必须置回 false 并渲染表格行（2026-08-31 永久 spinner 根因）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.list') return Promise.resolve({ issues: [issue()], total: 1 })
      if (cmd === 'stats') return Promise.resolve({ by_status: { backlog: 1 } })
      return Promise.resolve({})
    })
    const w = mount(IssueListView)
    await flushPromises()
    // 修复前：loading 永真 → 永远停在 spinner 分支，数据到位也不渲染。
    expect(w.find('.spinner').exists()).toBe(false)
    expect(w.find('.table-wrap').exists()).toBe(true)
    expect(w.findAll('tbody tr').length).toBe(1)
    expect(w.text()).toContain('NB-1')
    expect(w.text()).toContain('任务一')
  })

  it('创建成功 → issue.create payload 正确 + toast + 刷新后新行可见', async () => {
    let issues: any[] = []
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'issue.create') {
        issues = [issue({ title: data.title })]
        return Promise.resolve({ created: true, issue: issues[0] })
      }
      if (cmd === 'issue.list') return Promise.resolve({ issues, total: issues.length })
      return Promise.resolve({})
    })
    const w = mount(IssueListView)
    await flushPromises()
    await w.findAll('button').find((b) => b.text() === '+ 新建 Issue')!.trigger('click')
    await w.find('.modal input.form-input').setValue('网络自检任务')
    await w.findAll('button').find((b) => b.text() === '创建')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.find((c) => c[1] === 'issue.create')![2]).toMatchObject({
      title: '网络自检任务',
      priority: 1,
    })
    expect(useToast().toasts.some((t) => t.type === 'success' && t.message.includes('NB-1'))).toBe(true)
    // 创建后 refresh() 已把新 issue 渲染进表格（不再受 loading 永真影响）。
    expect(w.text()).toContain('网络自检任务')
  })

  it('W2.5 一键派发：worker 指派 + 可派发状态 → 行内「派发」按钮 → issue.dispatch {id, target}', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.list')
        return Promise.resolve({ issues: [issue({ assignee: 'worker', assignee_id: 'node-b' })], total: 1 })
      if (cmd === 'issue.dispatch')
        return Promise.resolve({ dispatched: true, task_id: 'task-abcdef12-3456' })
      return Promise.resolve({})
    })
    const w = mount(IssueListView)
    await flushPromises()
    const btn = w.findAll('tbody button').find((b) => b.text().includes('派发'))!
    expect(btn.exists()).toBe(true)

    await btn.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'issue.dispatch')!
    expect(call[2]).toEqual({ id: 1, target: 'node-b' })
    expect(useToast().toasts.some((t) => t.type === 'success' && t.message.includes('node-b'))).toBe(true)
  })

  it('W2.5 一键派发：未指派行不显示按钮（—）；manager_self 行也不显示', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.list')
        return Promise.resolve({
          issues: [
            issue({ id: 1, number: 'NB-1' }),
            issue({ id: 2, number: 'NB-2', assignee: 'manager_self', assignee_id: 'local' }),
            issue({ id: 3, number: 'NB-3', assignee: 'worker', assignee_id: 'node-b', status: 'done' }),
          ],
          total: 3,
        })
      return Promise.resolve({})
    })
    const w = mount(IssueListView)
    await flushPromises()
    expect(w.findAll('tbody button').filter((b) => b.text().includes('派发')).length).toBe(0)
  })

  it('W2.5 创建时指派 worker → 成功后 info toast 引导「派发」（指派 ≠ 派发）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.create') return Promise.resolve({ created: true, issue: issue() })
      if (cmd === 'issue.list') return Promise.resolve({ issues: [issue()], total: 1 })
      return Promise.resolve({})
    })
    const w = mount(IssueListView)
    await flushPromises()
    await w.findAll('button').find((b) => b.text() === '+ 新建 Issue')!.trigger('click')
    const modal = w.find('.modal')
    await modal.findAll('input.form-input').find((i) => i.attributes('placeholder') === '一句话描述任务')!.setValue('派发引导任务')
    // 指派下拉：暂不指派/manager（本机）/worker 节点。
    const assignSelect = modal.findAll('select.form-select').find((s) =>
      s.findAll('option').some((o) => o.element.value === 'worker'),
    )!
    await assignSelect.setValue('worker')
    // 无在线节点列表 → 回退手输节点 id。
    const idInput = modal.findAll('input.form-input').find((i) => i.attributes('placeholder') === '节点 id')!
    await idInput.setValue('node-b')
    await w.findAll('button').find((b) => b.text() === '创建')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.some((c) => c[1] === 'issue.dispatch')).toBe(false) // 只指派，不自动派发
  })
})
