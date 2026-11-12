import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 提示词内容可视化（2026-11-13）：构成条弹层的段色块 → 可点按钮，
// 点击新标签页打开独立查看器（prompt-viewer/index.html?session&segment）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn(),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: ref('connected'),
  }
})

import ChatPanel from '../ChatPanel.vue'
import { useSessionStore } from '../../stores/session'

const BREAKDOWN = {
  system: 40,
  tools: 160,
  mcp: 60,
  skill: 30,
  memory: 10,
  summary: 100,
  history: 100,
  free: 500,
}

function installBreakdownResponse() {
  requestMock.mockImplementation((_module: string, cmd: string) => {
    if (cmd === 'context_status')
      return Promise.resolve({
        context: {
          pct: 50,
          pct_trigger: 66,
          window: 1000,
          trigger: 750,
          anchor_tokens: 400,
          delta_tokens: 100,
          anchor_state: 'stale',
          breakdown: { ...BREAKDOWN },
        },
      })
    if (cmd === 'session_usage') return Promise.resolve({ total_cost_usd: 0 })
    return Promise.resolve({})
  })
}

describe('ChatPanel 段按钮（提示词内容可视化）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    requestMock.mockReset()
    requestMock.mockResolvedValue({})
  })

  it('弹层行首渲染 7 个段色按钮：usage-seg-{key} 类 + 查看 title', async () => {
    installBreakdownResponse()
    useSessionStore().currentId = 's1'
    const w = mount(ChatPanel)
    await flushPromises()
    await w.find('.usage-text').trigger('mouseenter')

    const chips = w.findAll('.usage-tip-chip')
    expect(chips.length).toBe(7)
    const keys = ['system', 'tools', 'mcp', 'skill', 'memory', 'summary', 'history']
    chips.forEach((c, i) => {
      expect(c.classes()).toContain(`usage-seg-${keys[i]}`)
      expect(c.attributes('type')).toBe('button')
    })
    // title 与段标签一致（tooltip 提示可点）
    expect(chips[0].attributes('title')).toContain('查看')
    expect(chips[0].attributes('title')).toContain('系统提示')
    w.unmount()
  })

  it('点击段按钮：window.open 携 session+segment 查询参数开新标签页', async () => {
    installBreakdownResponse()
    useSessionStore().currentId = 's1'
    const w = mount(ChatPanel)
    await flushPromises()
    await w.find('.usage-text').trigger('mouseenter')

    const openSpy = vi.spyOn(window, 'open').mockImplementation(() => null)
    const chips = w.findAll('.usage-tip-chip')
    await chips[3].trigger('click') // skill
    expect(openSpy).toHaveBeenCalledTimes(1)
    const [url, target, features] = openSpy.mock.calls[0]
    expect(url).toBe('prompt-viewer/index.html?session=s1&segment=skill')
    expect(target).toBe('_blank')
    expect(features).toBe('noopener')
    openSpy.mockRestore()
    w.unmount()
  })

  it('无会话 id（默认聊天无 currentId）不渲染构成条、无按钮', async () => {
    installBreakdownResponse()
    const w = mount(ChatPanel)
    await flushPromises()
    expect(w.find('.usage-strip').exists()).toBe(false)
    expect(w.findAll('.usage-tip-chip').length).toBe(0)
    w.unmount()
  })
})
