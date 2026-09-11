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

// useSSE 打桩（连接层不在本测试范围）：记录订阅，测试内手动触发
// board-changed 验证 DiscussionPanel 的增量拉取链路。
const sseHandlers = new Map<string, (data?: unknown) => void>()
vi.mock('../../../composables/useSSE', () => ({
  on: vi.fn((type: string, handler: (data?: unknown) => void) => {
    sseHandlers.set(type, handler)
  }),
  off: vi.fn((type: string) => {
    sseHandlers.delete(type)
  }),
}))

import BoardKanban from '../BoardKanban.vue'
import InboxPanel from '../InboxPanel.vue'
import ProjectPanel from '../ProjectPanel.vue'
import IssueDetailModal from '../IssueDetailModal.vue'
import AutopilotPanel from '../AutopilotPanel.vue'
import BoardTabs from '../BoardTabs.vue'
import DiscussionPanel from '../DiscussionPanel.vue'
import BoardConfigPanel from '../BoardConfigPanel.vue'
import AuditPanel from '../AuditPanel.vue'
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

describe('DiscussionPanel（讨论 M3 批次 E）', () => {
  function chan(over: Record<string, unknown> = {}) {
    return { id: 1, name: '#dev', description: '开发协作', created_at: 1700000000, ...over }
  }
  function msg(over: Record<string, unknown> = {}) {
    return {
      id: 1,
      channel_id: 1,
      sender: { kind: 'agent', id: 'node-b' },
      content: '大家好',
      parent_id: null,
      mtype: 'text',
      created_at: 1700000000,
      ...over,
    }
  }

  it('频道列表 + 默认选第一个 + 初始拉取（after_id=0）+ sender 三色徽标 + 回复缩进', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'channel.list')
        return Promise.resolve({ channels: [chan(), chan({ id: 2, name: '#general' })] })
      if (cmd === 'channel.messages')
        return Promise.resolve({
          messages: [
            msg(),
            msg({ id: 2, sender: { kind: 'admin', id: 'zoo' }, content: '收到' }),
            msg({ id: 3, sender: { kind: 'system', id: 'board' }, content: 'NB-1 已派发', mtype: 'system' }),
            msg({ id: 4, content: '跟进', parent_id: 1 }),
          ],
        })
      return Promise.resolve({})
    })
    const w = mount(DiscussionPanel)
    await flushPromises()
    expect(
      requestMock.mock.calls.find((c) => c[1] === 'channel.messages')![2],
    ).toEqual({ channel_id: 1, after_id: 0, limit: 500 })
    expect(w.text()).toContain('agent/node-b')
    expect(w.text()).toContain('admin/zoo')
    // 三色徽标：admin=info / agent=success / system=neutral
    expect(w.find('.badge-info').exists()).toBe(true)
    expect(w.find('.badge-success').exists()).toBe(true)
    expect(w.find('.message-item.system').exists()).toBe(true)
    // parent_id 非空 → 缩进线程行
    expect(w.find('.message-item.reply').text()).toContain('跟进')
  })

  it('发言 → channel.post 后立即增量拉取并清空输入框', async () => {
    let server: any[] = [msg()]
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'channel.list') return Promise.resolve({ channels: [chan()] })
      if (cmd === 'channel.messages')
        return Promise.resolve({ messages: server.filter((m) => m.id > data.after_id) })
      if (cmd === 'channel.post') {
        server = [...server, msg({ id: 4, sender: { kind: 'admin', id: 'zoo' }, content: data.content })]
        return Promise.resolve({ posted: { message_id: 4, seq: 9 } })
      }
      return Promise.resolve({})
    })
    const w = mount(DiscussionPanel)
    await flushPromises()
    const ta = w.find('textarea.composer-input')
    await ta.setValue('帮忙看下 NB-1')
    await ta.trigger('keydown.enter')
    await flushPromises()
    expect(
      requestMock.mock.calls.find((c) => c[1] === 'channel.post')![2],
    ).toEqual({ channel_id: 1, content: '帮忙看下 NB-1' })
    // 不等 SSE 推送：发言后立即按游标补拉（最后一次拉取 after_id=首条 id）。
    const pulls = requestMock.mock.calls.filter((c) => c[1] === 'channel.messages')
    expect(pulls[pulls.length - 1]![2].after_id).toBe(1)
    expect(w.text()).toContain('帮忙看下 NB-1')
    expect((w.find('textarea.composer-input').element as HTMLTextAreaElement).value).toBe('')
  })

  it('board-changed 推送（200ms 防抖）→ 按 lastId 游标拉增量并追加', async () => {
    vi.useFakeTimers()
    try {
      let server: any[] = [msg()]
      requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
        if (cmd === 'channel.list') return Promise.resolve({ channels: [chan()] })
        if (cmd === 'channel.messages')
          return Promise.resolve({ messages: server.filter((m) => m.id > data.after_id) })
        return Promise.resolve({})
      })
      const w = mount(DiscussionPanel)
      await flushPromises()
      // worker 上行新消息落库 → SSE 广播 → 防抖 200ms 后增量拉取。
      server = [...server, msg({ id: 2, content: '@zoo 收到，马上看' })]
      sseHandlers.get('board-changed')!()
      await vi.advanceTimersByTimeAsync(200)
      const pulls = requestMock.mock.calls.filter((c) => c[1] === 'channel.messages')
      expect(pulls).toHaveLength(2)
      expect(pulls[1]![2].after_id).toBe(1)
      expect(w.text()).toContain('@zoo 收到，马上看')
    } finally {
      vi.useRealTimers()
    }
  })

  it('channel.post 被拒（额度）→ toast 原样透出且输入保留', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'channel.list') return Promise.resolve({ channels: [chan()] })
      if (cmd === 'channel.messages') return Promise.resolve({ messages: [] })
      if (cmd === 'channel.post')
        return Promise.reject('[quota_exhausted] 本线程发言额度已用尽')
      return Promise.resolve({})
    })
    const w = mount(DiscussionPanel)
    await flushPromises()
    const ta = w.find('textarea.composer-input')
    await ta.setValue('第二条')
    await w.find('button.composer-send').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.message.includes('quota_exhausted'))).toBe(true)
    expect((w.find('textarea.composer-input').element as HTMLTextAreaElement).value).toBe('第二条')
  })
})

describe('IssueDetailModal ctype 徽标（M3 批次 E）', () => {
  function detailIssue(comments: any[]) {
    return { ...issue(), comments, activity: [], subscribers: [] }
  }

  it('delivery=交付(badge-info+底色)、question=提问(badge-warning)、回复行 discussion 徽标', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'issue.get')
        return Promise.resolve({
          issue: detailIssue([
            { id: 10, author: { kind: 'worker', id: 'node-b' }, content: '## 结论\n完成', parent_id: null, ctype: 'delivery', created_at: 1700000000 },
            { id: 11, author: { kind: 'admin', id: 'alice' }, content: '为什么选方案 X？', parent_id: null, ctype: 'question', created_at: 1700000001 },
            { id: 12, author: { kind: 'worker', id: 'node-b' }, content: '跟进说明', parent_id: 10, ctype: 'discussion', created_at: 1700000002 },
          ]),
        })
      if (cmd === 'attachment.list') return Promise.resolve({ attachments: [] })
      if (cmd === 'nodes.list') return Promise.resolve({ nodes: [] })
      return Promise.resolve({})
    })
    const w = mount(IssueDetailModal, { props: { issueId: 1 } })
    await flushPromises()
    expect(w.text()).toContain('交付')
    expect(w.text()).toContain('提问')
    const delivery = w.findAll('.comment-item').find((c) => c.text().includes('## 结论'))!
    expect(delivery.find('.badge-info').exists()).toBe(true)
    expect(delivery.find('.comment-body-report').exists()).toBe(true)
    const reply = w.find('.reply-item')
    expect(reply.text()).toContain('讨论')
    expect(reply.find('.badge-info').exists()).toBe(true)
  })
})

describe('BoardConfigPanel（配置 全自动流转 P1/A4）', () => {
  const fullFlags = {
    auto_review: true,
    auto_accept: false,
    auto_close_parent: false,
    unlimited_mode: false,
    max_redispatch: 2,
    dispatch_timeout_secs: 3600,
    plan: { auto_confirm: false, model: null },
    review: { max_turns: 1, selfcheck: false, auto_close_project: false },
    budget: { max_subissues_per_parent: 20, max_total_redispatch: 0, wall_clock_budget_secs: 0 },
    discussion: {
      retention_days: 30,
      max_agent_turns_per_thread: 12,
      hourly_budget_per_node: 20,
      rate_limit_per_min: 6,
    },
  }

  async function mountPanel(flags: Record<string, unknown>) {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.get') return Promise.resolve({ ...fullFlags, ...flags })
      return Promise.resolve({ updated: true })
    })
    const w = mount(BoardConfigPanel)
    await flushPromises()
    return w
  }

  it('渲染 7 个自动化开关 + 参数默认值', async () => {
    const w = await mountPanel({})
    expect(w.text()).toContain('拆解自动发车')
    expect(w.text()).toContain('自动验收')
    expect(w.text()).toContain('PASS 自动收货')
    expect(w.text()).toContain('父单自动收口')
    expect(w.text()).toContain('验收取证')
    expect(w.text()).toContain('项目自动收口')
    expect(w.text()).toContain('无限模式')
    expect(w.text()).toContain('验收 FAIL 重派上限')
    expect(w.text()).toContain('预算护栏')
    expect(w.text()).toContain('任务墙钟时限')
    const checked = w.findAll('input[type="checkbox"]')
    expect(checked.length).toBe(7)
    // fullFlags：auto_review=true 开，其余关（toggles 顺序：[0]=plan.auto_confirm、[1]=auto_review）。
    expect((checked[0].element as HTMLInputElement).checked).toBe(false)
    expect((checked[1].element as HTMLInputElement).checked).toBe(true)
  })

  it('开关切换 → config.set {key, value} 即时保存 + 成功 toast', async () => {
    const w = await mountPanel({})
    await w.findAll('input[type="checkbox"]')[2].setValue(true) // auto_accept
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'config.set')!
    expect(call[2]).toEqual({ key: 'auto_accept', value: true })
    expect(useToast().toasts.some((t) => t.type === 'success')).toBe(true)
  })

  it('保存失败 → error toast + 重新 config.get 回滚显示', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'config.get') return Promise.resolve({ ...fullFlags })
      if (cmd === 'config.set') return Promise.reject('未知或不允许的 board 配置键')
      return Promise.resolve({})
    })
    const w = mount(BoardConfigPanel)
    await flushPromises()
    await w.findAll('input[type="checkbox"]')[1].setValue(false) // auto_review → false 被拒
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error')).toBe(true)
    // 失败后重拉，开关回滚为服务端真实值 true。
    const gets = requestMock.mock.calls.filter((c) => c[1] === 'config.get')
    expect(gets.length).toBe(2)
    expect((w.findAll('input[type="checkbox"]')[1].element as HTMLInputElement).checked).toBe(true)
  })

  it('无限模式开 → 警示条出现；auto_accept 关附 PASS 仍等人工提示', async () => {
    const w = await mountPanel({ unlimited_mode: true })
    expect(w.text()).toContain('无限模式已开启')
    expect(w.text()).toContain('PASS 自动收货未开启')
  })

  it('无限模式开且 auto_accept 开 → 警示条无「未开启」提示', async () => {
    const w = await mountPanel({ unlimited_mode: true, auto_accept: true })
    expect(w.text()).toContain('无限模式已开启')
    expect(w.text()).not.toContain('PASS 自动收货未开启')
  })

  it('数字参数修改 → config.set 提交数值', async () => {
    const w = await mountPanel({})
    const numInput = w.findAll('input[type="number"]')[0] // max_redispatch
    await numInput.setValue('5')
    await numInput.trigger('change')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'config.set')!
    expect(call[2]).toEqual({ key: 'max_redispatch', value: 5 })
  })

  it('数字参数非法（负数）→ warn toast 不发 config.set', async () => {
    const w = await mountPanel({})
    const numInput = w.findAll('input[type="number"]')[0]
    await numInput.setValue('-1')
    await numInput.trigger('change')
    await flushPromises()
    expect(requestMock.mock.calls.some((c) => c[1] === 'config.set')).toBe(false)
    expect(useToast().toasts.some((t) => t.type === 'warn')).toBe(true)
  })

  it('模型别名清空 → config.set plan.model=null；填值 → 字符串', async () => {
    const w = await mountPanel({ plan: { auto_confirm: false, model: 'glm-4.7' } })
    const modelInput = w.findAll('input[type="text"]').find((i) =>
      (i.element as HTMLInputElement).value === 'glm-4.7',
    )!
    await modelInput.setValue('')
    await modelInput.trigger('change')
    await flushPromises()
    expect(requestMock.mock.calls.find((c) => c[1] === 'config.set')![2]).toEqual({
      key: 'plan.model',
      value: null,
    })
  })
})

describe('BoardTabs（页签顺序）', () => {
  it('从左到右 = 使用依赖链：项目 → 列表 → 看板 → 收件箱 → 自动化 → 讨论 → 决策流 → 配置', () => {
    const w = mount(BoardTabs, { props: { modelValue: 'projects' } })
    const labels = w.findAll('button').map((b) => b.text())
    expect(labels).toEqual(['项目', '列表', '看板', '收件箱', '自动化', '讨论', '决策流', '配置'])
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

// ---------------------------------------------------------------------------
// 全自动流转 P3 前端：项目「验收标准 + 自动启动」/ autopilot「auto_plan」
// ---------------------------------------------------------------------------

describe('P3 全自动流转表单字段', () => {
  it('ProjectPanel：验收标准 + 自动启动 → project.create payload', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'project.list') return Promise.resolve({ projects: [] })
      if (cmd === 'project.create')
        return Promise.resolve({ project: { id: 1 }, auto_start: { issue_id: 9, issue_number: 'NB-9' } })
      return Promise.resolve({})
    })
    const w = mount(ProjectPanel)
    await flushPromises()
    await w.findAll('button').find((b) => b.text().includes('新建项目'))!.trigger('click')
    await w.findAll('input.form-input').find((i) => i.attributes('placeholder')?.includes('项目名'))!.setValue('自动启动项目')
    await w.findAll('textarea').find((t) => t.attributes('placeholder')?.includes('验收标准'))!.setValue('1. 父单自动建\n2. 拆解可发车')
    const autoStartBox = w.findAll('input[type="checkbox"]').find((c) => c.element.parentElement?.textContent?.includes('自动启动'))!
    expect(autoStartBox.attributes('disabled')).toBeUndefined()
    await autoStartBox.setValue(true)
    await w.findAll('button').find((b) => b.text() === '创建')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'project.create')!
    expect(call[2].acceptance_criteria).toBe('1. 父单自动建\n2. 拆解可发车')
    expect(call[2].auto_start).toBe(true)
    // 后端返回 auto_start.issue_number → toast 明示父单号。
    const toasts = useToast().toasts.map((t) => t.message).join('\n')
    expect(toasts).toContain('NB-9')
  })

  it('ProjectPanel：默认不勾自动启动 → auto_start=false（保守默认）', async () => {
    const w = mount(ProjectPanel)
    await flushPromises()
    await w.findAll('button').find((b) => b.text().includes('新建项目'))!.trigger('click')
    await w.findAll('input.form-input').find((i) => i.attributes('placeholder')?.includes('项目名'))!.setValue('普通项目')
    await w.findAll('button').find((b) => b.text() === '创建')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'project.create')!
    expect(call[2].auto_start).toBe(false)
    expect(call[2].acceptance_criteria).toBe('')
  })

  it('AutopilotPanel：勾选 auto_plan → autopilot.create payload 带字段；列表显示「建单 + 自动拆解」', async () => {
    function ap(over: Record<string, unknown> = {}) {
      return {
        id: 1, name: '每日站会', title: '每日站会 {date}', cron: '0 9 * * *',
        description: '', priority: 1, project_id: null, target: '',
        auto_plan: false, enabled: true, cron_job_id: 'job-1',
        last_run_at: null, created_at: 1700000000, updated_at: 1700000000,
        ...over,
      }
    }
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'autopilot.list') return Promise.resolve({ autopilots: [ap({ auto_plan: true })] })
      return Promise.resolve({})
    })
    const w = mount(AutopilotPanel)
    await flushPromises()
    // 列表徽标：auto_plan 规则的派发目标列。
    expect(w.text()).toContain('建单 + 自动拆解')

    // 创建表单：填必填项 + 勾选 auto_plan → payload。
    await w.findAll('button').find((b) => b.text().includes('新建规则'))!.trigger('click')
    const inputs = w.findAll('input.form-input')
    await inputs[0].setValue('拆解规则')
    await inputs[1].setValue('0 9 * * *')
    await inputs[2].setValue('拆解任务 {date}')
    const planBox = w.findAll('input[type="checkbox"]').find((c) => c.element.parentElement?.textContent?.includes('auto_plan'))!
    await planBox.setValue(true)
    // 配置派发目标后 auto_plan 应禁用（互斥：直接派发时拆解无意义）。
    await inputs[3].setValue('node-b')
    expect(planBox.attributes('disabled')).toBeDefined()
    await inputs[3].setValue('')
    await w.findAll('button').find((b) => b.text() === '创建')!.trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'autopilot.create')!
    expect(call[2].auto_plan).toBe(true)
  })
})

describe('AuditPanel（决策流 全自动流转 P5/E2）', () => {
  function auditRow(over: Record<string, unknown> = {}) {
    return {
      id: 1,
      issue_id: 11,
      actor: { kind: 'agent', id: 'node-review' },
      action: 'auto_decide',
      details: JSON.stringify({ decision: 'auto_accept', verdict: 'PASS', note: '锚点全过' }),
      created_at: 1700000000,
      issue_number: 'NB-11',
      issue_title: '任务十一',
      ...over,
    }
  }

  async function mountAudit(rows: any[], extra: Record<string, any> = {}) {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'audit.list') return Promise.resolve({ decisions: rows })
      return Promise.resolve(extra[cmd] ?? {})
    })
    const w = mount(AuditPanel)
    await flushPromises()
    return w
  }

  it('列表渲染：决策词人话标签 + 单号标题 + actor + 时间（新→旧流水）', async () => {
    const w = await mountAudit([
      auditRow(),
      auditRow({
        id: 2,
        issue_number: 'NB-12',
        issue_title: '任务十二',
        details: JSON.stringify({ decision: 'redispatch', verdict: 'FAIL', round: 1 }),
      }),
    ])
    const items = w.findAll('.audit-item')
    expect(items.length).toBe(2)
    expect(items[0].text()).toContain('验收 PASS · 自动收货')
    expect(items[0].text()).toContain('NB-11')
    expect(items[0].text()).toContain('任务十一')
    expect(items[0].text()).toContain('@node-review')
    expect(items[1].text()).toContain('验收 FAIL · 自动重派')
    // 每行都有回滚按钮。
    expect(w.findAll('.rollback-btn').length).toBe(2)
    // details 请求带 limit + 无过滤。
    const call = requestMock.mock.calls.find((c) => c[1] === 'audit.list')!
    expect(call[2]).toEqual({ limit: 200, action: undefined })
  })

  it('未知决策词回退原文 + details 解析失败不炸渲染', async () => {
    const w = await mountAudit([
      auditRow({ details: JSON.stringify({ decision: 'future_word' }) }),
      auditRow({ id: 2, details: 'not-json{{{' }),
    ])
    const items = w.findAll('.audit-item')
    expect(items[0].text()).toContain('future_word')
    // 非 JSON details：无详情按钮、无标签崩溃，行仍渲染。
    expect(items[1].text()).toContain('NB-11')
    expect(items[1].findAll('.expand-btn').length).toBe(0)
  })

  it('详情展开：点击切换显示格式化 JSON', async () => {
    const w = await mountAudit([auditRow()])
    expect(w.find('.audit-details').exists()).toBe(false)
    await w.find('.expand-btn').trigger('click')
    const pre = w.find('.audit-details')
    expect(pre.exists()).toBe(true)
    expect(pre.text()).toContain('"decision": "auto_accept"')
    await w.find('.expand-btn').trigger('click')
    expect(w.find('.audit-details').exists()).toBe(false)
  })

  it('回滚确认流：点回滚 → 弹窗确认 → audit.rollback（带 activity_id）→ 重拉列表', async () => {
    let rollbackPayload: any = null
    const w = await mountAudit([auditRow()], {
      'audit.rollback': Promise.resolve({ rolled_back: true }),
    })
    // 拦截 rollback 调用记录 payload。
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'audit.list') return Promise.resolve({ decisions: [auditRow()] })
      if (cmd === 'audit.rollback') {
        rollbackPayload = data
        return Promise.resolve({ rolled_back: true })
      }
      return Promise.resolve({})
    })
    await w.find('.rollback-btn').trigger('click')
    const modal = w.find('.modal-backdrop')
    expect(modal.exists()).toBe(true)
    expect(modal.text()).toContain('NB-11')
    expect(modal.text()).toContain('验收 PASS · 自动收货')
    // 取消不调后端。
    await modal.findAll('button').find((b) => b.text() === '取消')!.trigger('click')
    expect(w.find('.modal-backdrop').exists()).toBe(false)
    expect(rollbackPayload).toBeNull()

    // 再开 → 确认 → rollback 调用带 activity_id。
    await w.find('.rollback-btn').trigger('click')
    await w.findAll('button').find((b) => b.text() === '确认回滚')!.trigger('click')
    await flushPromises()
    expect(rollbackPayload).toEqual({ activity_id: 1 })
    expect(w.find('.modal-backdrop').exists()).toBe(false)
    // 确认后重拉了列表（audit.list 第二次调用）。
    const listCalls = requestMock.mock.calls.filter((c) => c[1] === 'audit.list')
    expect(listCalls.length).toBeGreaterThanOrEqual(2)
  })

  it('回滚失败：后端拒绝（非 done/已回滚）→ toast 报错，弹窗保留', async () => {
    const w = await mountAudit([auditRow()])
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'audit.list') return Promise.resolve({ decisions: [auditRow()] })
      if (cmd === 'audit.rollback') return Promise.reject(new Error('仅支持回滚已自动收货（done）的决策'))
      return Promise.resolve({})
    })
    await w.find('.rollback-btn').trigger('click')
    await w.findAll('button').find((b) => b.text() === '确认回滚')!.trigger('click')
    await flushPromises()
    const toasts = useToast().toasts
    expect(toasts.some((t: any) => t.type === 'error' && t.message.includes('回滚失败'))).toBe(true)
    expect(w.find('.modal-backdrop').exists()).toBe(true)
  })

  it('空态 + action 过滤参数透传', async () => {
    const w = await mountAudit([])
    expect(w.text()).toContain('暂无自动决策记录')
    // 换过滤选项 → audit.list 带 action。
    await w.find('select.filter-select').setValue('auto_confirm_dispatch')
    await flushPromises()
    const calls = requestMock.mock.calls.filter((c) => c[1] === 'audit.list')
    expect(calls[calls.length - 1][2]).toEqual({
      limit: 200,
      action: 'auto_confirm_dispatch',
    })
  })
})
