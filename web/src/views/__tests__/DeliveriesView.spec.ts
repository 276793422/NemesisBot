import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// 交付审核页最小渲染 spec（P4 人审闭环，2026-10-25）：列表渲染（状态/种类
// 徽标 + 发布条目链接）、过滤、确认/拒绝裁决请求形态、详情模态 patch 按
// 需拉取。发布表单随域剥离批（2026-11-02）退役——发布动作由 flow 模板
// publish 步声明驱动，本页只留发布留痕展示。WSAPI 契约
// （deliveries.list/get/patch/confirm/reject 响应形态）由后端钉住，这里
// 只锁前端渲染分支与请求形态。

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
    external_ref: null,
    external_url: null,
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
          row({ id: 'd-2', idempotency_key: 'task-2:branch', status: 'published', external_ref: 7, external_url: 'http://x/pr/7', published_at: '2026-10-25T11:00:00+08:00' }),
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
    // published 行带外部链接
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

  it('裁决按钮可见性：rejected/published 行无确认/拒绝按钮', async () => {
    const w = await mountView()
    const rows = w.findAll('[data-test="dl-row"]')
    // delivered 行：确认 + 拒绝都在
    expect(rows[0].find('[data-test="dl-confirm"]').exists()).toBe(true)
    expect(rows[0].find('[data-test="dl-reject"]').exists()).toBe(true)
    // published 行：两按钮全无（终态）
    expect(rows[1].find('[data-test="dl-confirm"]').exists()).toBe(false)
    expect(rows[1].find('[data-test="dl-reject"]').exists()).toBe(false)
    // rejected 行：两按钮全无（终态）
    expect(rows[3].find('[data-test="dl-confirm"]').exists()).toBe(false)
    expect(rows[3].find('[data-test="dl-reject"]').exists()).toBe(false)
    // review 行有拒绝
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
