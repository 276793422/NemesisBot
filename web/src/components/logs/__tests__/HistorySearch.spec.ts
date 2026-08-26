import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// M6 补测（quality-hardening goal 2026-08-25）：T6 会话检索组件。
// mock useWSAPI.request —— 后端 logs.history_search/reindex 契约由
// handlers/logs/history_tests.rs 钉住，这里测前端交互层。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import HistorySearch from '../HistorySearch.vue'

function mountSearch() {
  return mount(HistorySearch)
}

beforeEach(() => {
  requestMock.mockReset()
})

describe('HistorySearch 检索', () => {
  it('空查询：按钮禁用且不发请求', async () => {
    const w = mountSearch()
    const btn = w.findAll('button').find(b => b.text().includes('检索'))!
    expect(btn.attributes('disabled')).toBeDefined()
    await w.find('input').setValue('   ')
    expect(btn.attributes('disabled')).toBeDefined()
    expect(requestMock).not.toHaveBeenCalled()
  })

  it('命中渲染：行含会话/角色/片段，请求带 query 与 limit', async () => {
    requestMock.mockResolvedValue({
      hits: [
        { session_key: 'agent:main:session:s1', seq: 3, role: 'user', timestamp: '2026-08-24T10:00:00+08:00', snippet: 'hello 世界' },
        { session_key: 'agent:main:session:s2', seq: 7, role: 'assistant', timestamp: '2026-08-24T11:00:00+08:00', snippet: '好的，文件已读' },
      ],
    })
    const w = mountSearch()
    await w.find('input').setValue('世界')
    await w.findAll('button').find(b => b.text().includes('检索'))!.trigger('click')
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('logs', 'history_search', { query: '世界', limit: 20 })
    const rows = w.findAll('tbody tr')
    expect(rows.length).toBe(2)
    expect(w.text()).toContain('agent:main:session:s1')
    expect(w.text()).toContain('hello 世界')
    // 角色图标：user 👤 / assistant 🤖
    expect(rows[0].text()).toContain('👤')
    expect(rows[1].text()).toContain('🤖')
  })

  it('无命中：empty-state 提示带查询词', async () => {
    requestMock.mockResolvedValue({ hits: [] })
    const w = mountSearch()
    await w.find('input').setValue('不存在的词')
    await w.findAll('button').find(b => b.text().includes('检索'))!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('没有命中「不存在的词」')
    // 未检索前是引导文案，不是这条
    expect(w.text()).not.toContain('输入关键词检索全部会话历史')
  })

  it('检索失败：错误横幅 + hits 清空', async () => {
    requestMock.mockResolvedValueOnce({ hits: [{ session_key: 'ZQZ-marker', seq: 1, role: 'user', timestamp: '', snippet: 'x' }] })
    const w = mountSearch()
    await w.find('input').setValue('a')
    await w.findAll('button').find(b => b.text().includes('检索'))!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('ZQZ-marker')

    requestMock.mockRejectedValue(new Error('检索引擎不可用'))
    await w.findAll('button').find(b => b.text().includes('检索'))!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('检索失败')
    expect(w.text()).toContain('检索引擎不可用')
    expect(w.text()).not.toContain('ZQZ-marker')
    // loading 复位（按钮可再点）
    expect(w.findAll('button').find(b => b.text().includes('检索'))!.attributes('disabled')).toBeUndefined()
  })

  it('检索中：按钮禁用且文案变化', async () => {
    let release!: (v: any) => void
    requestMock.mockReturnValue(new Promise(r => (release = r)))
    const w = mountSearch()
    await w.find('input').setValue('q')
    const btn = w.findAll('button').find(b => b.text().includes('检索'))!
    await btn.trigger('click')
    expect(btn.attributes('disabled')).toBeDefined()
    expect(btn.text()).toContain('检索中')
    release({ hits: [] })
    await flushPromises()
    expect(btn.attributes('disabled')).toBeUndefined()
  })
})

describe('HistorySearch 重建索引', () => {
  it('重建 N 个会话 / 已是最新 两种文案', async () => {
    requestMock.mockResolvedValue({ reindexed_sessions: 3 })
    const w = mountSearch()
    await w.findAll('button').find(b => b.text().includes('重建索引'))!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('logs', 'history_reindex', {})
    expect(w.text()).toContain('已重建 3 个会话的索引')

    requestMock.mockResolvedValue({ reindexed_sessions: 0 })
    await w.findAll('button').find(b => b.text().includes('重建索引'))!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('索引已是最新')
  })

  it('重建失败：错误横幅', async () => {
    requestMock.mockRejectedValue('db locked')
    const w = mountSearch()
    await w.findAll('button').find(b => b.text().includes('重建索引'))!.trigger('click')
    await flushPromises()
    expect(w.text()).toContain('检索失败')
    expect(w.text()).toContain('db locked')
  })
})

describe('HistorySearch 定位', () => {
  it('点击「➤ 会话」emit locate(session_key)', async () => {
    requestMock.mockResolvedValue({
      hits: [{ session_key: 'agent:main:session:hit1', seq: 2, role: 'user', timestamp: '', snippet: 'x' }],
    })
    const w = mountSearch()
    await w.find('input').setValue('x')
    await w.findAll('button').find(b => b.text().includes('检索'))!.trigger('click')
    await flushPromises()
    await w.findAll('button').find(b => b.text().includes('会话'))!.trigger('click')
    expect(w.emitted('locate')).toBeTruthy()
    expect(w.emitted('locate')![0]).toEqual(['agent:main:session:hit1'])
  })
})
