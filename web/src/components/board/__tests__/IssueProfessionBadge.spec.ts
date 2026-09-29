// 职能框架 M6：IssueDetailModal 的职能徽标展示契约。
// - issue.required_profession 存在 → 元信息行 🛠 徽标；
// - AI 拆解预览子单 required_profession → plan-node 头 🛠 徽标；
// - 无职能 → 不渲染（避免空徽标）。
// 后端派发匹配真相源 = handlers/board.rs matcher + nemesis-board matcher；
// 本 spec 只钉展示。
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

// useSSE 打桩（连接层不在本测试范围）；计划回执经注册的 handler 驱动（真实链路）。
const sseHandlers = new Map<string, (data: any) => void>()
vi.mock('../../../composables/useSSE', () => ({
  on: (ev: string, fn: (data: any) => void) => sseHandlers.set(ev, fn),
  off: (ev: string) => sseHandlers.delete(ev),
}))

import IssueDetailModal from '../IssueDetailModal.vue'

function issue(over: Record<string, unknown> = {}) {
  return {
    id: 1,
    number: 'NB-1',
    title: '画登录页',
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
    required_profession: null as string | null,
    created_at: 1700000000,
    updated_at: 1700000000,
    comments: [],
    activity: [],
    subscribers: [],
    ...over,
  }
}

function mockBackend(row: Record<string, unknown>) {
  requestMock.mockImplementation(async (_m: string, cmd: string) => {
    if (cmd === 'issue.get') return { issue: row }
    if (cmd === 'attachment.list') return { attachments: [] }
    if (cmd === 'nodes.list') return { nodes: [] }
    return {}
  })
}

async function mountModal(row: Record<string, unknown>) {
  mockBackend(row)
  const w = mount(IssueDetailModal, { props: { issueId: 1 } })
  await flushPromises()
  return w
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
})

describe('IssueDetailModal 职能徽标（M6）', () => {
  it('required_profession 存在 → 元信息行渲染 🛠 徽标', async () => {
    const w = await mountModal(issue({ required_profession: 'dev:cpp' }))
    const meta = w.find('.detail-meta')
    expect(meta.exists()).toBe(true)
    const badges = meta.findAll('.badge').map((b) => b.text())
    expect(badges.some((t) => t.includes('dev:cpp'))).toBe(true)
    w.unmount()
  })

  it('无职能 → 不渲染徽标', async () => {
    const w = await mountModal(issue())
    const meta = w.find('.detail-meta')
    const badges = meta.findAll('.badge').map((b) => b.text())
    expect(badges.some((t) => t.includes('🛠'))).toBe(false)
    w.unmount()
  })

  it('AI 拆解预览：子单 required_profession 渲染 plan-node 徽标', async () => {
    const w = await mountModal(issue())
    // 经 board.plan_ready SSE 回执驱动（真实链路：onPlanReady 灌 planSubs）。
    sseHandlers.get('board.plan_ready')!({
      issue_id: 1,
      plan_id: 'p1',
      subs: [
        {
          title: '子单一',
          description: '',
          required_profession: 'ui-design',
          required_role: '',
          required_tags: [],
          acceptance_criteria: '',
          depends_on: [],
        },
      ],
    })
    await w.vm.$nextTick()
    const node = w.find('.plan-node-head')
    expect(node.exists()).toBe(true)
    expect(node.text()).toContain('ui-design')
    w.unmount()
  })
})
