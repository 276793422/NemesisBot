import { mount } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import SecurityAudit from '../SecurityAudit.vue'
import type { AuditEntry } from '../mockData'

// SecurityAudit 为纯 props 组件（无 WSAPI/SSE 依赖）：筛选条、表格、
// 详情侧滑、导出。本 spec 覆盖全部筛选分支 / 风险图标分支 / 导出与
// raw JSON 回显（raw 缺席走 fallback 组装分支）。

function makeEntry(over: Partial<AuditEntry> = {}): AuditEntry {
  return {
    id: 'evt-1',
    timestamp: '2026-06-27T10:00:00.123+08:00',
    operation: 'file_write',
    risk_level: 'HIGH',
    target: '/tmp/a.txt',
    result: 'allow',
    decision: 'allow',
    user: 'alice',
    reason: '测试原因',
    policy: 'pol-1',
    raw: { event_id: 'evt-1', extra: 1 },
    ...over,
  }
}

const ENTRIES: AuditEntry[] = [
  makeEntry({ id: 'evt-1', risk_level: 'HIGH', operation: 'file_write', result: 'allow', decision: 'allow' }),
  makeEntry({ id: 'evt-2', risk_level: 'LOW', operation: 'process_exec', result: 'deny', decision: 'deny', user: undefined, reason: undefined, policy: undefined }),
  makeEntry({ id: 'evt-3', risk_level: 'MEDIUM', operation: 'file_write', result: 'allow', decision: 'allow' }),
  makeEntry({ id: 'evt-4', risk_level: 'CRITICAL', operation: 'registry_write', result: 'deny', decision: 'deny', raw: undefined as any }),
]

function mountAudit(entries: AuditEntry[] = ENTRIES) {
  return mount(SecurityAudit, { props: { entries } })
}

beforeEach(() => {
  vi.stubGlobal('confirm', vi.fn(() => true))
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('SecurityAudit 渲染', () => {
  it('渲染全部行：风险徽标/结果图标/决策徽标/毫秒时间', () => {
    const w = mountAudit()
    const rows = w.findAll('tbody tr:not(.empty-row)')
    // 4 条数据行（空态行 v-if 未渲染）
    expect(rows.length).toBe(4)
    expect(w.text()).toContain('显示 4 / 4')
    // 时间列走 formatTime(ts, true)（毫秒形态）
    expect(rows[0]!.find('.cell-time').text()).toMatch(/\d{2}:\d{2}:\d{2}\.\d{3}/)
    // 风险徽标：icon + 小写 class
    expect(rows[0]!.find('.risk-badge.high').text()).toContain('🟠')
    expect(rows[1]!.find('.risk-badge.low').text()).toContain('🟢')
    expect(rows[2]!.find('.risk-badge.medium').text()).toContain('🟡')
    expect(rows[3]!.find('.risk-badge.critical').text()).toContain('🔴')
    // 结果图标：allow ✓ / deny ✗
    expect(rows[0]!.find('.result-icon.allow').text()).toBe('✓')
    expect(rows[1]!.find('.result-icon.deny').text()).toBe('✗')
    // 决策徽标
    expect(rows[0]!.find('.decision-badge.allow').text()).toBe('allow')
    expect(rows[1]!.find('.decision-badge.deny').text()).toBe('deny')
    // 目标列 title
    expect(rows[0]!.find('.cell-target').attributes('title')).toBe('/tmp/a.txt')
    // 详情侧滑未打开
    expect(w.find('.audit-detail').exists()).toBe(false)
  })

  it('空 entries 显示空态行', () => {
    const w = mountAudit([])
    expect(w.text()).toContain('暂无审计事件')
    expect(w.text()).toContain('显示 0 / 0')
  })
})

describe('SecurityAudit 筛选', () => {
  it('风险 chip 点亮/再点取消 → filtered 实时增减', async () => {
    const w = mountAudit()
    const chips = w.findAll('.chip-risk')
    expect(chips.length).toBe(4)
    // 点 HIGH → 只显示 HIGH 条目（chip = 选中要显示的风险级别）
    await chips.find(c => c.text().includes('HIGH'))!.trigger('click')
    expect(w.findAll('.chip-risk.active').length).toBe(1)
    expect(w.text()).toContain('显示 1 / 4')
    expect(w.findAll('tbody .cell-op').map(c => c.text())).toEqual(['file_write'])
    // 再点 HIGH → 取消筛选恢复 4 条（Set delete 分支）
    await w.findAll('.chip-risk').find(c => c.text().includes('HIGH'))!.trigger('click')
    expect(w.findAll('.chip-risk.active').length).toBe(0)
    expect(w.text()).toContain('显示 4 / 4')
  })

  it('多选风险 chip：LOW + CRITICAL 同时点亮', async () => {
    const w = mountAudit()
    await w.findAll('.chip-risk').find(c => c.text().includes('LOW'))!.trigger('click')
    await w.findAll('.chip-risk').find(c => c.text().includes('CRITICAL'))!.trigger('click')
    expect(w.findAll('.chip-risk.active').length).toBe(2)
    expect(w.text()).toContain('显示 2 / 4')
    const ids = w.findAll('tbody tr').map(r => r.find('.cell-op').text())
    expect(ids).toEqual(['process_exec', 'registry_write'])
  })

  it('操作下拉：选项来自 entries 去重排序；选中后过滤', async () => {
    const w = mountAudit()
    const selects = w.findAll('select.form-select')
    expect(selects.length).toBe(2)
    const opOptions = selects[0]!.findAll('option').map(o => o.text())
    expect(opOptions).toEqual(['全部', 'file_write', 'process_exec', 'registry_write'])
    await selects[0]!.setValue('process_exec')
    expect(w.text()).toContain('显示 1 / 4')
    expect(w.findAll('tbody .cell-op').map(c => c.text())).toEqual(['process_exec'])
    // 恢复全部
    await selects[0]!.setValue('all')
    expect(w.text()).toContain('显示 4 / 4')
  })

  it('决策下拉：allow/deny 过滤', async () => {
    const w = mountAudit()
    const selects = w.findAll('select.form-select')
    await selects[1]!.setValue('deny')
    expect(w.text()).toContain('显示 2 / 4')
    expect(w.findAll('tbody .decision-badge').map(b => b.text())).toEqual(['deny', 'deny'])
  })

  it('组合筛选无命中 → 空态行出现', async () => {
    const w = mountAudit()
    const selects = w.findAll('select.form-select')
    await selects[0]!.setValue('process_exec')
    await selects[1]!.setValue('allow')
    expect(w.text()).toContain('暂无审计事件')
    expect(w.text()).toContain('显示 0 / 4')
  })
})

describe('SecurityAudit 详情侧滑', () => {
  it('点行打开详情：全字段 + raw JSON 原样回显；行高亮', async () => {
    const w = mountAudit()
    await w.findAll('tbody tr')[0]!.trigger('click')
    const detail = w.find('.audit-detail')
    expect(detail.exists()).toBe(true)
    expect(w.findAll('tbody tr')[0]!.classes()).toContain('selected')
    expect(detail.text()).toContain('事件详情')
    expect(detail.text()).toContain('alice')
    expect(detail.text()).toContain('pol-1')
    expect(detail.text()).toContain('/tmp/a.txt')
    // raw 存在 → 原样 JSON.stringify(raw)
    expect(detail.text()).toContain('"extra": 1')
    // 关闭按钮清空 selectedId
    await detail.findAll('button').find(b => b.text() === '✕')!.trigger('click')
    expect(w.find('.audit-detail').exists()).toBe(false)
    expect(w.findAll('tbody tr')[0]!.classes()).not.toContain('selected')
  })

  it('raw 缺席 → 详情组装 fallback JSON（event_id/timestamp/...）', async () => {
    const w = mountAudit([ENTRIES[3]!])
    await w.findAll('tbody tr')[0]!.trigger('click')
    const detail = w.find('.audit-detail')
    expect(detail.text()).toContain('"event_id": "evt-4"')
    expect(detail.text()).toContain('"risk_level": "CRITICAL"')
    // user 仍存在（evt-4 未覆盖 user）→ user 行照常渲染
    expect(detail.text()).toContain('alice')
  })

  it('可选字段缺席的行：user/reason/policy 行不出现', async () => {
    const w = mountAudit([ENTRIES[1]!])
    await w.findAll('tbody tr')[0]!.trigger('click')
    const detail = w.find('.audit-detail')
    const labels = detail.findAll('.detail-row span:first-child').map(s => s.text())
    expect(labels).not.toContain('user')
    expect(labels).not.toContain('reason')
    expect(labels).not.toContain('policy')
  })
})

describe('SecurityAudit 导出', () => {
  it('导出：Blob 文本含筛选后条目；download 名 audit-*.txt；URL 释放', async () => {
    const createObjectURLMock = vi.fn((_blob: Blob) => 'blob:mock-url')
    const revokeObjectURLMock = vi.fn()
    vi.stubGlobal('URL', class extends URL {
      static createObjectURL = createObjectURLMock
      static revokeObjectURL = revokeObjectURLMock
    })
    // jsdom Blob 无 .text() —— 用可读回 parts 的替身
    const blobs: string[][] = []
    vi.stubGlobal('Blob', class MockBlob {
      parts: string[]
      type: string
      constructor(parts: string[], opts?: { type?: string }) {
        this.parts = parts
        this.type = opts?.type ?? ''
        blobs.push(parts)
      }
      async text() { return this.parts.join('') }
    })
    const anchorClickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})

    const w = mountAudit()
    // 先筛掉一部分 → 导出只含筛选后条目
    await w.findAll('select.form-select')[1]!.setValue('deny')
    await w.findAll('button').find(b => b.text().includes('导出'))!.trigger('click')

    expect(createObjectURLMock).toHaveBeenCalledTimes(1)
    const blob = createObjectURLMock.mock.calls[0]![0] as unknown as { type: string; parts: string[] }
    expect(blob.type).toBe('text/plain')
    const text = blob.parts.join('')
    // 两条 deny 条目，格式 [ts] [risk] [decision] op: target - reason
    expect(text).toContain('[deny] registry_write: /tmp/a.txt - 测试原因')
    expect(text).toContain('[LOW] [deny] process_exec: /tmp/a.txt - ')
    expect(text.split('\n').length).toBe(2)
    const anchor = anchorClickSpy.mock.instances[0] as unknown as HTMLAnchorElement
    expect(anchor.href).toBe('blob:mock-url')
    expect(anchor.download).toMatch(/^audit-\d+\.txt$/)
    expect(revokeObjectURLMock).toHaveBeenCalledWith('blob:mock-url')

    anchorClickSpy.mockRestore()
  })
})
