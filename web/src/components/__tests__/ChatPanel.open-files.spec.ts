import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// I5 前端接线（2026-10-17）：文件树 📌 上下文文件随 chat.send 上报——
// A) 设定 📌 后发送 → data.open_files = [路径]；B) 未设定 → 不带字段
// （老协议零影响）；C) 清除后 → 不再携带。

const { requestMock, sendMock, messageHandlers } = vi.hoisted(() => ({
  requestMock: vi.fn(),
  sendMock: vi.fn(),
  messageHandlers: [] as ((msg: any) => void)[],
}))

vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  return {
    connect: vi.fn(),
    send: sendMock,
    sendHistoryRequest: vi.fn(),
    onMessage: (cb: (m: any) => void) => { messageHandlers.push(cb) },
    addMessageHandler: (cb: (m: any) => void) => { messageHandlers.push(cb) },
    removeMessageHandler: (cb: (m: any) => void) => {
      const i = messageHandlers.indexOf(cb)
      if (i >= 0) messageHandlers.splice(i, 1)
    },
    wsStatus: ref('connected'),
    httpGet: vi.fn().mockResolvedValue({}),
  }
})

import ChatPanel from '../ChatPanel.vue'
import { useChatStore } from '../../stores/chat'
import { useSessionStore } from '../../stores/session'
import { useFileTreePanel } from '../../composables/useFileTreePanel'

beforeEach(() => {
  setActivePinia(createPinia())
  messageHandlers.length = 0
  sendMock.mockReset()
  requestMock.mockReset()
  requestMock.mockImplementation(async (module: string, cmd: string) => {
    if (module === 'sessions' && cmd === 'list') return { sessions: [] }
    if (module === 'projects') return { projects: [] }
    return {}
  })
  useFileTreePanel().setCurrentFile(null)
})

async function mountPanel() {
  const w = mount(ChatPanel)
  await flushPromises()
  // mock 的 sendHistoryRequest 永不回落 → 手工落定（生产由历史加载完成置 false）
  useChatStore().historyLoading = false
  await flushPromises()
  return w
}

async function typeAndSend(w: ReturnType<typeof mount>, text: string) {
  await w.find('.chat-input-area textarea').setValue(text)
  await w.find('.chat-input-area button.btn-primary').trigger('click')
  await flushPromises()
}

describe('ChatPanel I5 上下文文件上报（2026-10-17）', () => {
  it('A: 📌 设定后发送 → chat.send moduleData.open_files = [路径]', async () => {
    useFileTreePanel().setCurrentFile('src/main.rs')
    // 直设会话锚（发送链带 session_id 的同一前提），省去 sessions.create mock 分支。
    useSessionStore().currentId = 's1'
    const w = await mountPanel()
    await typeAndSend(w, '这个文件为什么崩')

    const chatSend = sendMock.mock.calls.find((c: any[]) => c[2]?.moduleData?.session_id !== undefined)
    expect(chatSend).toBeTruthy()
    expect(chatSend![2].moduleData.open_files).toEqual(['src/main.rs'])
    w.unmount()
  })

  it('B: 未设定 → 不带 open_files 字段（老协议零影响）', async () => {
    useSessionStore().currentId = 's1'
    const w = await mountPanel()
    await typeAndSend(w, '普通消息')

    const chatSend = sendMock.mock.calls.find((c: any[]) => c[2]?.moduleData?.session_id !== undefined)
    expect(chatSend).toBeTruthy()
    expect(chatSend![2].moduleData.open_files).toBeUndefined()
    w.unmount()
  })

  it('C: 清除后不再携带', async () => {
    useSessionStore().currentId = 's1'
    const tree = useFileTreePanel()
    tree.setCurrentFile('README.md')
    const w = await mountPanel()
    await typeAndSend(w, '第一条')
    // 首条发送后 busy=true 会让发送按钮退场/禁用（v-if="!streaming ||
    // canQueueWhileBusy"），mock 下无回复帧收尾——手工落 busy 并 flush
    // （等 :disabled 重渲染）再发第二条。
    useChatStore().setBusy('s1', false)
    await flushPromises()
    tree.setCurrentFile(null)
    await typeAndSend(w, '第二条')

    const sends = sendMock.mock.calls.filter((c: any[]) => c[2]?.moduleData?.session_id !== undefined)
    expect(sends.length).toBe(2)
    expect(sends[0][2].moduleData.open_files).toEqual(['README.md'])
    expect(sends[1][2].moduleData.open_files).toBeUndefined()
    w.unmount()
  })
})
