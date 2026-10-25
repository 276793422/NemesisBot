import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// 交付审核页最小渲染 spec（P4 人审闭环，2026-10-25）：列表渲染（状态/种类
// 徽标 + PR 链接）、过滤、确认/拒绝裁决请求形态、发布模态请求形态
// （task_id 从幂等键 `{task_id}:branch` 提取）、发布三分支 outcome 展示
// （published / already_published / failed）、详情模态 patch 按需拉取。
// WSAPI 契约（deliveries.list/get/patch/confirm/reject + board.delivery.publish
// 响应形态）由后端钉住，这里只锁前端渲染分支与请求形态。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import DeliveriesView from '../DeliveriesView.vue'
import { useToast } from '../../composables/useToast'

function row(extra: Record<string, unknown> = {}) {
  return {
    id: 'd-1',
    run_id: 't-42',
    kind: 'branch',
    idempotency_key: 'task-1:branch',
    status: 'delivered',
    created_at: '2026-10-25T10:00:00+08:00',
    delivered_at: '2026-10-25T10:00:01+08:00',
    confirmed_at: null,
    rejected_at: null,
    reject_reason: null,
    notify_state: 'channel_accepted',
    notify_attempts: 0,
    notify_last_error: null,
    branch: 'nemesis/task-task-1',
    node: 'node-a',
    files_changed: 2,
    insertions: 10,
    deletions: 1,
    patch_truncated: false,
    patch_path: null,
    patch_present: true,
    commit: 'abc123',
    head_tree: 'tree-42',
    base_tree: 'tree-41',
    pr_number: null,
    pr_url: null,
    published_at: null,
    ...extra,
  }
}

function defaultImpl() {
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'list') {
      return Promise.resolve({
        deliveries: [
          row(),
          row({ id: 'd-2', idempotency_key: 'task-2:branch', status: 'published', pr_number: 7, pr_url: 'http://x/pr/7', published_at: '2026-10-25T11:00:00+08:00' }),
          row({ id: 'd-3', idempotency_key: 'run-9:review', kind: 'review', branch: null, files_changed: null, patch_present: false }),
          row({ id: 'd-4', idempotency_key: 'task-4:branch', status: 'rejected', reject_reason: '测试不完整' }),
        ],
      })
    }
    if (cmd === 'patch') return Promise.resolve({ patch: 'diff --git a/f b/f\n' })
    return Promise.resolve({})
  })
}

beforeEach(() => {
  requestMock.mockReset()
  defaultImpl()
  useToast().toasts.splice(0)
})

async function mountView() {
  const w = mount(DeliveriesView)
  await flushPromises()
  return w
}

describe('DeliveriesView 列表与徽标', () => {
  it('拉取 deliveries.list 并渲染全部行；徽标与统计列诚实展示', async () => {
    const w = await mountView()
    const rows = w.findAll('[data-test="dl-row"]')
    expect(rows).toHaveLength(4)
    // 状态徽标文案（delivered/published/rejected 映射）
    expect(rows[0].find('[data-test="dl-status"]').text()).toBe('已送达')
    expect(rows[1].find('[data-test="dl-status"]').text()).toBe('已发布')
    expect(rows[3].find('[data-test="dl-status"]').text()).toBe('已拒绝')
    // published 行带 PR 链接
    expect(rows[1].find('[data-test="dl-pr-link"]').attributes('href')).toBe('http://x/pr/7')
    // diffstat
    expect(rows[0].text()).toContain('+10')
    expect(rows[0].text()).toContain('-1')
    w.unmount()
  })

  it('状态过滤只留匹配行', async () => {
    const w = await mountView()
    await w.find('[data-test="filter-status"]').setValue('rejected')
    const rows = w.findAll('[data-test="dl-row"]')
    expect(rows).toHaveLength(1)
    expect(rows[0].text()).toContain('已拒绝')
    w.unmount()
  })

  it('裁决按钮可见性：rejected/published 行无确认/拒绝/发布按钮', async () => {
    const w = await mountView()
    const rows = w.findAll('[data-test="dl-row"]')
    // delivered 行：确认 + 拒绝 + 发布都在
    expect(rows[0].find('[data-test="dl-confirm"]').exists()).toBe(true)
    expect(rows[0].find('[data-test="dl-reject"]').exists()).toBe(true)
    expect(rows[0].find('[data-test="dl-publish"]').exists()).toBe(true)
    // published 行：三按钮全无（终态）
    expect(rows[1].find('[data-test="dl-confirm"]').exists()).toBe(false)
    expect(rows[1].find('[data-test="dl-reject"]').exists()).toBe(false)
    expect(rows[1].find('[data-test="dl-publish"]').exists()).toBe(false)
    // rejected 行：三按钮全无（终态）
    expect(rows[3].find('[data-test="dl-confirm"]').exists()).toBe(false)
    expect(rows[3].find('[data-test="dl-publish"]').exists()).toBe(false)
    // review 行：无发布（kind≠branch），有拒绝
    expect(rows[2].find('[data-test="dl-publish"]').exists()).toBe(false)
    expect(rows[2].find('[data-test="dl-reject"]').exists()).toBe(true)
    w.unmount()
  })

  it('列表失败：整页错误卡 + 重试', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.reject('boom')
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(w.find('[data-test="load-error"]').text()).toContain('boom')
    w.unmount()
  })
})

describe('DeliveriesView 裁决面', () => {
  it('确认：deliveries.confirm 带幂等键，成功后刷新列表', async () => {
    const w = await mountView()
    await w.findAll('[data-test="dl-confirm"]')[0].trigger('click')
    await flushPromises()
    const calls = requestMock.mock.calls.filter((c: unknown[]) => c[1] === 'confirm')
    expect(calls).toHaveLength(1)
    expect(calls[0][0]).toBe('deliveries')
    expect(calls[0][2]).toEqual({ idempotency_key: 'task-1:branch' })
    w.unmount()
  })

  it('拒绝：模态带原因提交到 deliveries.reject', async () => {
    const w = await mountView()
    await w.findAll('[data-test="dl-reject"]')[0].trigger('click')
    await w.find('[data-test="reject-reason"]').setValue('锚点没过')
    await w.find('[data-test="reject-submit"]').trigger('click')
    await flushPromises()
    const calls = requestMock.mock.calls.filter((c: unknown[]) => c[1] === 'reject')
    expect(calls).toHaveLength(1)
    expect(calls[0][2]).toEqual({ idempotency_key: 'task-1:branch', reason: '锚点没过' })
    w.unmount()
  })
})

describe('DeliveriesView 发布面', () => {
  it('发布模态：task_id 从幂等键提取，提交走 board.delivery.publish', async () => {
    const w = await mountView()
    await w.findAll('[data-test="dl-publish"]')[0].trigger('click')
    expect((w.find('[data-test="pub-task-id"]').element as HTMLInputElement).value).toBe('task-1')
    await w.find('[data-test="pub-owner"]').setValue('o')
    await w.find('[data-test="pub-repo"]').setValue('r')
    await w.find('[data-test="pub-base"]').setValue('main')
    requestMock.mockImplementation((m: string, cmd: string) => {
      if (m === 'board' && cmd === 'delivery.publish') {
        return Promise.resolve({ status: 'published', pr_number: 9, pr_url: 'http://x/pr/9', commit: 'c', branch: 'b', detail: 'ok' })
      }
      if (cmd === 'list') return Promise.resolve({ deliveries: [] })
      return Promise.resolve({})
    })
    await w.find('[data-test="publish-submit"]').trigger('click')
    await flushPromises()
    const calls = requestMock.mock.calls.filter((c: unknown[]) => c[1] === 'delivery.publish')
    expect(calls).toHaveLength(1)
    expect(calls[0][0]).toBe('board')
    expect(calls[0][2]).toEqual({ task_id: 'task-1', connector: 'gitea', owner: 'o', repo: 'r', base: 'main' })
    w.unmount()
  })

  it('必填校验：缺 owner/repo 不发请求，错误留在模态', async () => {
    const w = await mountView()
    await w.findAll('[data-test="dl-publish"]')[0].trigger('click')
    await w.find('[data-test="publish-submit"]').trigger('click')
    expect(w.find('[data-test="publish-error"]').text()).toContain('owner')
    const calls = requestMock.mock.calls.filter((c: unknown[]) => c[1] === 'delivery.publish')
    expect(calls).toHaveLength(0)
    w.unmount()
  })

  it('failed outcome：错误详情留在模态不关；页面 outcome 卡诚实展示', async () => {
    const w = await mountView()
    await w.findAll('[data-test="dl-publish"]')[0].trigger('click')
    await w.find('[data-test="pub-owner"]').setValue('o')
    await w.find('[data-test="pub-repo"]').setValue('r')
    requestMock.mockImplementation((m: string, cmd: string) => {
      if (m === 'board' && cmd === 'delivery.publish') {
        return Promise.resolve({ status: 'failed', pr_number: null, detail: '基线漂移：副本 HEAD aa ≠ 交付基线 bb' })
      }
      if (cmd === 'list') return Promise.resolve({ deliveries: [] })
      return Promise.resolve({})
    })
    await w.find('[data-test="publish-submit"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-test="publish-error"]').text()).toContain('基线漂移')
    expect(w.find('[data-test="publish-outcome"]').text()).toContain('发布失败')
    w.unmount()
  })

  it('already_published 幂等短路：toast + 模态关闭', async () => {
    const w = await mountView()
    await w.findAll('[data-test="dl-publish"]')[0].trigger('click')
    await w.find('[data-test="pub-owner"]').setValue('o')
    await w.find('[data-test="pub-repo"]').setValue('r')
    requestMock.mockImplementation((m: string, cmd: string) => {
      if (m === 'board' && cmd === 'delivery.publish') {
        return Promise.resolve({ status: 'already_published', pr_number: 3, pr_url: 'http://x/pr/3', detail: '该交付已发布（幂等跳过，不重复建 PR）' })
      }
      if (cmd === 'list') return Promise.resolve({ deliveries: [] })
      return Promise.resolve({})
    })
    await w.find('[data-test="publish-submit"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-test="publish-modal"]').exists()).toBe(false)
    expect(w.find('[data-test="publish-outcome"]').text()).toContain('已发布过')
    w.unmount()
  })
})

describe('DeliveriesView 详情模态', () => {
  it('patch_present 行按需拉 patch 并展示；无 patch 行不请求', async () => {
    const w = await mountView()
    const rows = w.findAll('[data-test="dl-row"]')
    await rows[0].find('[data-test="dl-detail"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-test="patch-view"]').text()).toContain('diff --git')
    const patchCalls = requestMock.mock.calls.filter((c: unknown[]) => c[1] === 'patch')
    expect(patchCalls).toHaveLength(1)
    await w.find('[data-test="detail-close"]').trigger('click')
    // review 行无 patch_present——打开详情不再发 patch 请求
    await rows[2].find('[data-test="dl-detail"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter((c: unknown[]) => c[1] === 'patch')).toHaveLength(1)
    w.unmount()
  })

  it('patch 拉取失败：错误原文诚实展示', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve({ deliveries: [row()] })
      if (cmd === 'patch') return Promise.reject('交付 payload 无内联 patch 文本')
      return Promise.resolve({})
    })
    const w = await mountView()
    await w.find('[data-test="dl-detail"]').trigger('click')
    await flushPromises()
    expect(w.find('.load-error').text()).toContain('无内联 patch')
    w.unmount()
  })
})
