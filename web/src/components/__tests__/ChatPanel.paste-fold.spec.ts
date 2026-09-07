import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// I4（devtool-upgrade 阶段 4）：超长粘贴折叠 —— >2000 字符纯文本粘贴替换为
// 占位符 `[Pasted ~N lines #p1]`（原文暂存本地映射），chips 点击展开预览，
// 发送时占位符还原为全文（上行与回显都是全量）；短文本不干预。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

const wsSendMock = vi.fn()
vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  return {
    connect: vi.fn(),
    send: (...args: any[]) => wsSendMock(...args),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn(),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: ref('connected'),
  }
})

import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'

beforeEach(() => {
  setActivePinia(createPinia())
  useSessionStore().currentId = 's1'
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  wsSendMock.mockReset()
})

async function mountPanel() {
  const w = mount(ChatPanel)
  await flushPromises()
  return w
}

/** 构造 length 字符、lines 行的文本（行尾有 HEAD/TAIL 标记供全文断言）。 */
function makeText(length: number, lines: number): string {
  const per = Math.max(1, Math.floor(length / lines))
  const body = Array.from({ length: lines }, (_, i) => `L${i}:${'x'.repeat(per - 4)}`)
  let s = body.join('\n')
  s = 'HEAD_' + s.slice(5)
  s = s.slice(0, Math.max(0, length - 5)) + '_TAIL'
  return s
}

function pasteEvent(text: string) {
  return {
    clipboardData: {
      files: [] as File[],
      getData: (type: string) => (type === 'text/plain' ? text : ''),
    },
  }
}

function paste(wrapper: ReturnType<typeof mount>, text: string) {
  return wrapper.find('textarea').trigger('paste', pasteEvent(text))
}

function chipsOf(wrapper: ReturnType<typeof mount>) {
  return wrapper.findAll('.paste-chip')
}

describe('ChatPanel 超长粘贴折叠（I4）', () => {
  it('短文本粘贴不折叠（无占位符、无 chip）', async () => {
    const w = await mountPanel()
    const chat = useChatStore()
    chat.input = 'abc'
    await paste(w, 'short text')
    await flushPromises()
    expect(chat.input).toBe('abc')
    expect(chipsOf(w)).toHaveLength(0)
    w.unmount()
  })

  it('超长粘贴 → 占位符插入光标处，chip 显示行数', async () => {
    const w = await mountPanel()
    const chat = useChatStore()
    const text = makeText(3000, 5)
    chat.input = 'abc'
    await nextFrame(w)
    // 光标移到末尾（jsdom 默认 selectionStart=0）。
    const ta = w.find('textarea').element as HTMLTextAreaElement
    ta.selectionStart = ta.selectionEnd = 3
    await paste(w, text)
    await flushPromises()

    expect(chat.input.startsWith('abc[Pasted ~5 lines #p1]')).toBe(true)
    const chips = chipsOf(w)
    expect(chips).toHaveLength(1)
    expect(chips[0].text()).toContain('~5 lines')
    expect(chips[0].text()).toContain('#p1')
    w.unmount()
  })

  it('单行超长粘贴 → chars 计数占位符', async () => {
    const w = await mountPanel()
    const chat = useChatStore()
    const text = 'z'.repeat(2500)
    await paste(w, text)
    await flushPromises()
    expect(chat.input).toBe('[Pasted ~2500 chars #p1]')
    expect(chipsOf(w)[0].text()).toContain('~2500 chars')
    w.unmount()
  })

  it('chip 点击展开预览原文，再点收起', async () => {
    const w = await mountPanel()
    const text = makeText(3000, 5)
    await paste(w, text)
    await flushPromises()

    expect(w.find('.paste-preview').exists()).toBe(false)
    await chipsOf(w)[0].trigger('click')
    const pre = w.find('.paste-preview')
    expect(pre.exists()).toBe(true)
    expect(pre.text()).toContain('HEAD_')
    expect(pre.text()).toContain('_TAIL')
    expect(pre.text().length).toBe(3000)

    await chipsOf(w)[0].trigger('click')
    expect(w.find('.paste-preview').exists()).toBe(false)
    w.unmount()
  })

  it('多次粘贴 → p1/p2 独立占位与预览', async () => {
    const w = await mountPanel()
    await paste(w, makeText(2500, 4))
    await paste(w, makeText(2600, 2))
    await flushPromises()

    const chips = chipsOf(w)
    expect(chips).toHaveLength(2)
    expect(chips[0].text()).toContain('#p1')
    expect(chips[1].text()).toContain('#p2')
    await chipsOf(w)[1].trigger('click')
    expect(w.find('.paste-preview').text()).toContain('_TAIL')
    w.unmount()
  })

  it('手动删除占位符 → chip 消失；发送时映射随输入清空', async () => {
    const w = await mountPanel()
    const chat = useChatStore()
    await paste(w, makeText(2500, 4))
    await flushPromises()
    expect(chipsOf(w)).toHaveLength(1)

    chat.input = '没有任何占位符了'
    await flushPromises()
    expect(chipsOf(w)).toHaveLength(0)
    w.unmount()
  })

  it('发送时占位符还原为全文（send 首参含头尾标记），回显为全量', async () => {
    const w = await mountPanel()
    const chat = useChatStore()
    const text = makeText(3000, 5)
    await paste(w, text)
    await flushPromises()
    expect(chat.input).toContain('#p1')

    const sendBtn = w.findAll('button').find(b => b.text() === '发送')
    await sendBtn!.trigger('click')
    await flushPromises()

    expect(wsSendMock).toHaveBeenCalledTimes(1)
    const sent = wsSendMock.mock.calls[0][0] as string
    expect(sent.startsWith('HEAD_')).toBe(true)
    expect(sent.endsWith('_TAIL')).toBe(true)
    expect(sent.length).toBe(3000)
    // 本地回显 = 全量原文，不再是占位符。
    const lastUser = [...chat.messages].reverse().find(m => m.role === 'user')
    expect(lastUser?.content).toBe(text)
    // 输入区与 chips 清空。
    expect(chat.input).toBe('')
    expect(chipsOf(w)).toHaveLength(0)
    w.unmount()
  })

  it('混合文本+占位符一起还原（占位符夹在文字中间）', async () => {
    const w = await mountPanel()
    const chat = useChatStore()
    const text = 'y'.repeat(2500)
    await paste(w, text)
    await flushPromises()
    chat.input = `前文 ${chat.input} 后文`
    await flushPromises()

    const sendBtn = w.findAll('button').find(b => b.text() === '发送')
    await sendBtn!.trigger('click')
    await flushPromises()

    const sent = wsSendMock.mock.calls[0][0] as string
    expect(sent).toBe(`前文 ${text} 后文`)
    w.unmount()
  })

  it('手打的同形占位符（不在映射里）发送时原样保留', async () => {
    const w = await mountPanel()
    const chat = useChatStore()
    chat.input = '[Pasted ~3 lines #p9] hello'
    await flushPromises()

    const sendBtn = w.findAll('button').find(b => b.text() === '发送')
    await sendBtn!.trigger('click')
    await flushPromises()

    expect(wsSendMock.mock.calls[0][0]).toBe('[Pasted ~3 lines #p9] hello')
    w.unmount()
  })
})

async function nextFrame(w: ReturnType<typeof mount>) {
  await w.vm.$nextTick()
}
