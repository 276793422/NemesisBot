import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'

import IntegrityChain from '../IntegrityChain.vue'
import type { ChainSegment } from '../mockData'

function seg(index: number, valid = true): ChainSegment {
  return {
    index,
    timestamp: 1762000000000,
    hash: 'a'.repeat(64),
    prevHash: 'b'.repeat(64),
    valid,
    breakReason: valid ? undefined : 'prev_hash 不匹配',
    payloadSummary: 'session.append user turn',
  } as unknown as ChainSegment
}

const SEGS = [seg(0), seg(1, false), seg(2)]

describe('IntegrityChain 展示', () => {
  it('空链段显示空态', () => {
    const w = mount(IntegrityChain, { props: { segments: [] } })
    expect(w.text()).toContain('暂无审计链段')
    expect(w.text()).toContain('尚未验证')
  })

  it('渲染链段行：index/hash 前缀/有效标记', () => {
    const w = mount(IntegrityChain, { props: { segments: SEGS } })
    expect(w.text()).toContain('#0')
    expect(w.text()).toContain('aaaaaaaaaaaaaaaa...')
    expect(w.findAll('.chain-row').length).toBe(3)
    expect(w.findAll('.chain-row.broken').length).toBe(1)
  })

  it('未验证/全部有效/有断裂三种状态文案', async () => {
    const w = mount(IntegrityChain, { props: { segments: SEGS } })
    expect(w.text()).toContain('尚未验证')

    await w.setProps({ verifyResult: { valid: true, first_broken_index: null, broken_count: 0, total_segments: 3 } })
    expect(w.text()).toContain('全部有效')

    await w.setProps({ verifyResult: { valid: false, first_broken_index: 1, broken_count: 1, total_segments: 3 } })
    expect(w.text()).toContain('1 处断裂')
    expect(w.text()).toContain('（共 3 条事件）')
  })

  it('验证/刷新按钮 emit', async () => {
    const w = mount(IntegrityChain, { props: { segments: SEGS } })
    await w.findAll('button').find(b => b.text().includes('验证全部完整性'))!.trigger('click')
    expect(w.emitted('verify')).toBeTruthy()
    await w.findAll('button').find(b => b.text().includes('刷新'))!.trigger('click')
    expect(w.emitted('reload')).toBeTruthy()
  })

  it('点击行展开详情；关闭收起；断裂行显示原因', async () => {
    const w = mount(IntegrityChain, { props: { segments: SEGS } })
    await w.findAll('.chain-row')[1]!.trigger('click')
    expect(w.text()).toContain('链段 #1')
    expect(w.text()).toContain('prev_hash 不匹配')
    expect(w.text()).toContain('session.append user turn')
    // 全 hash 展示（截断前 16 位 → 详情完整 64 位）
    expect(w.text()).toContain('a'.repeat(64))
    await w.findAll('.detail-header button').find(b => b.text().includes('✕'))!.trigger('click')
    expect(w.text()).not.toContain('链段 #1')
  })
})
