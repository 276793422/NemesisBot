import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// 2026-11-12 会话分叉入口（PLAN 234 批次 5）：message-time 回退按钮组收编为
// 「⋯」三点菜单 + assistant 行「⑂ 从此分叉」直达 fork。本 spec 钉：
// - 菜单项构成：rewind 两项原样保留 + 分叉项只在 assistant 行；
// - 分叉轮次换算：forkTurnIndexOf（非 [steer] user 行计数 +1，与后端 turn
//   表同构）→ fork(sid, N) 参数正确；
// - 分叉后链：fetchList(true) + switchTo(新会话) + toast；
// - 与全局右键菜单互斥单开（双向：右键压掉「⋯」；开「⋯」关右键）；
// - 点外/Esc 关闭；非 chat 模块无触发钮。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

const forkMock = vi.fn()
vi.mock('../../composables/useChatApi', () => ({
  useChatApi: () => ({ fork: (...args: any[]) => forkMock(...args) }),
}))

/** 捕获槽（vi.hoisted——mock 工厂在 import 期执行，普通顶层 let 会 TDZ）。 */
const h = vi.hoisted(() => ({
  wsHandler: null as ((data: any) => void) | null,
  wsStatus: null as any,
}))

vi.mock('../../composables/useWebSocket', async () => {
  const { ref } = await import('vue')
  h.wsStatus = ref<'connected' | 'disconnected'>('connected')
  return {
    connect: vi.fn(),
    send: vi.fn(),
    sendHistoryRequest: vi.fn(),
    onMessage: vi.fn((fn: (data: any) => void) => { h.wsHandler = fn }),
    addMessageHandler: vi.fn(),
    removeMessageHandler: vi.fn(),
    wsStatus: h.wsStatus,
  }
})

import ChatPanel from '../ChatPanel.vue'
import ContextMenu from '../ContextMenu.vue'
import { sendHistoryRequest } from '../../composables/useWebSocket'
import { useSessionStore } from '../../stores/session'
import { useToast } from '../../composables/useToast'
import { installContextMenu, useContextMenu } from '../../composables/useContextMenu'

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  requestMock.mockResolvedValue({})
  forkMock.mockReset()
  forkMock.mockResolvedValue({ session_id: 's-fork', kept_messages: 1 })
  h.wsHandler = null
  h.wsStatus.value = 'connected'
  useContextMenu().close()
  document.querySelectorAll('.msg-menu-overlay, .msg-menu, .ctx-menu, .ctx-overlay')
    .forEach((el) => el.remove())
})

function prepareSessions() {
  const s = useSessionStore()
  s.sessions = [
    { id: 's1', channel: 'web', startTime: '', lastTime: '', messageCount: 0, firstMessage: 'A', model: '' },
  ] as any
  s.currentId = 's1'
}

function feedHistory(requestId: string, messages: any[]) {
  h.wsHandler!({
    type: 'message',
    module: 'chat',
    cmd: 'history_response',
    data: { request_id: requestId, messages, has_more: false, oldest_index: 0 },
  })
}

function lastHistoryRequestId(): string {
  const calls = vi.mocked(sendHistoryRequest).mock.calls
  return calls[calls.length - 1][0] as string
}

/** 点开目标行「⋯」并返回菜单项文本列表（菜单本体 Teleport to body →
 *  wrapper.find 找不到，须经 document.body）。 */
async function openMsgMenu(w: any, role: 'user' | 'assistant'): Promise<string[]> {
  const row = w.findAll('.message.' + role).at(-1)
  expect(row, role + ' 消息行应存在').toBeTruthy()
  await row!.find('.msg-menu-trigger').trigger('click')
  await flushPromises()
  return Array.from(document.querySelectorAll('.msg-menu-item')).map((b) =>
    (b.textContent ?? '').trim(),
  )
}

async function mountWithHistory(messages: any[], attach = false) {
  prepareSessions()
  installContextMenu()
  // attach：右键贡献项用例要挂在真文档上——脱离文档树的事件不冒泡到
  // window，全局 contextmenu listener 收不到。
  const w = mount(ChatPanel, attach ? { attachTo: document.body } : {})
  await flushPromises()
  feedHistory(lastHistoryRequestId(), messages)
  await flushPromises()
  return w
}

describe('ChatPanel「⋯」消息菜单与会话分叉入口', () => {
  it('菜单项构成：user 行 = rewind 两项；assistant 行 = rewind 两项 + 从此分叉', async () => {
    const w = await mountWithHistory([
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])

    const userItems = await openMsgMenu(w, 'user')
    expect(userItems.some((t) => t.includes('重新生成'))).toBe(true)
    expect(userItems.some((t) => t.includes('撤销此问'))).toBe(true)
    expect(userItems.some((t) => t.includes('从此分叉'))).toBe(false)

    const asstItems = await openMsgMenu(w, 'assistant')
    expect(asstItems.some((t) => t.includes('删除整轮'))).toBe(true)
    expect(asstItems.some((t) => t.includes('从此分叉'))).toBe(true)
    w.unmount()
  })

  it('分叉直达：assistant 行点击 → fork(sid, 轮号) + fetchList + switchTo + toast', async () => {
    const w = await mountWithHistory([
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    const items = await openMsgMenu(w, 'assistant')

    const forkItem = Array.from(document.querySelectorAll('.msg-menu-item')).find((b) =>
      b.textContent?.includes('从此分叉'),
    )
    ;(forkItem as HTMLElement).dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await flushPromises()

    // 单轮历史 → a1 所在轮 = 第 1 轮 → fork('s1', 1)（保留前 1 轮）。
    expect(forkMock).toHaveBeenCalledTimes(1)
    expect(forkMock).toHaveBeenCalledWith('s1', 1)
    expect(useSessionStore().currentId, '分叉后应切入新会话').toBe('s-fork')
    const toasts = useToast().toasts
    expect(toasts.some((t) => t.message.includes('已分叉出新会话'))).toBe(true)
    // 菜单已随点击关闭。
    expect(document.querySelector('.msg-menu-overlay')).toBeNull()
    expect(items.length).toBeGreaterThan(0)
    w.unmount()
  })

  it('分叉轮次换算：steer 插话不占轮号（q1,a1,[steer],a2 → a2 在第 2 轮）', async () => {
    const w = await mountWithHistory([
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
      { role: 'user', content: '[steer] 插话内容' },
      { role: 'assistant', content: 'a2' },
    ])
    await openMsgMenu(w, 'assistant')

    const forkItem = Array.from(document.querySelectorAll('.msg-menu-item')).find((b) =>
      b.textContent?.includes('从此分叉'),
    )
    ;(forkItem as HTMLElement).dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await flushPromises()

    expect(forkMock).toHaveBeenCalledWith('s1', 2)
    w.unmount()
  })

  it('互斥单开（右键压「⋯」）：⋯ 菜单开着 → 任意处右键 → ⋯ 菜单关', async () => {
    const w = await mountWithHistory([
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    await openMsgMenu(w, 'assistant')
    expect(document.querySelector('.msg-menu-overlay')).toBeTruthy()

    document.body.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
    )
    await flushPromises()

    expect(document.querySelector('.msg-menu-overlay'), '右键应关掉「⋯」菜单').toBeNull()
    w.unmount()
  })

  it('互斥单开（「⋯」压右键）：右键菜单开着 → 点触发钮 → 右键菜单关', async () => {
    const w = await mountWithHistory([
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    // 右键菜单开在 body（无区域命中也无妨——只要有状态；本 spec 不挂
    // ContextMenu.vue 渲染体，断言走 composable 状态而非 DOM）。
    const ctx = useContextMenu()
    ctx.openAt(10, 10, [{ label: '复制', run: () => {} }])
    expect(ctx.state.value).not.toBeNull()

    await openMsgMenu(w, 'assistant')
    await flushPromises()

    expect(ctx.state.value, '开「⋯」应关掉右键菜单').toBeNull()
    w.unmount()
  })

  it('点外关闭：overlay click → 菜单关', async () => {
    const w = await mountWithHistory([
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    await openMsgMenu(w, 'assistant')
    const overlay = document.querySelector('.msg-menu-overlay') as HTMLElement
    expect(overlay).toBeTruthy()
    overlay.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await flushPromises()
    expect(document.querySelector('.msg-menu-overlay')).toBeNull()
    w.unmount()
  })

  it('Esc 关闭：菜单开着 → window keydown Escape → 菜单关', async () => {
    const w = await mountWithHistory([
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    await openMsgMenu(w, 'assistant')
    expect(document.querySelector('.msg-menu-overlay')).toBeTruthy()

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))
    await flushPromises()
    expect(document.querySelector('.msg-menu-overlay')).toBeNull()
    w.unmount()
  })

  it('非 chat 模块（workflow_chat）：无「⋯」触发钮（分叉/回退入口都不出）', async () => {
    prepareSessions()
    const w = mount(ChatPanel, { props: { module: 'workflow_chat' } })
    await flushPromises()
    feedHistory(lastHistoryRequestId(), [
      { role: 'user', content: 'q1' },
      { role: 'assistant', content: 'a1' },
    ])
    await flushPromises()
    expect(w.findAll('.msg-menu-trigger')).toHaveLength(0)
    w.unmount()
  })
})

// 2026-11-13 右键菜单贡献项：文件链接（linkify a.file-link）右键追加
// 「复制路径 / 打开所在目录」（底部两项）。ChatPanel 贡献函数带会话上下
// 文（effectiveSid）与本面板消息区 containment——集成面在此钉住。
describe('ChatPanel 文件链接右键贡献项', () => {
  it('右键 .file-link → 菜单四项（两项贡献项在底部）；点「打开所在目录」→ fs.reveal 带 session_id', async () => {
    const w = await mountWithHistory(
      [{ role: 'assistant', content: '报告已生成：C:/ws/reports/a.md 请查收' }],
      true,
    )
    const cm = mount(ContextMenu, { attachTo: document.body })

    const link = w.find('a.file-link')
    expect(link.exists(), '文件路径应被 linkify 成链接').toBe(true)

    link.element.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
    )
    await flushPromises()

    const texts = Array.from(document.querySelectorAll('.ctx-item')).map((b) =>
      (b.textContent ?? '').trim(),
    )
    expect(texts).toEqual(['复制', '全选', '复制路径', '打开所在目录'])

    const open = Array.from(document.querySelectorAll('.ctx-item')).find((b) =>
      b.textContent?.includes('打开所在目录'),
    ) as HTMLElement
    open.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('fs', 'reveal', {
      path: 'C:/ws/reports/a.md',
      session_id: 's1',
    })
    cm.unmount()
    w.unmount()
  })

  it('右键非链接消息区 → 仅默认两项（贡献函数空数组不出项）', async () => {
    const w = await mountWithHistory([{ role: 'assistant', content: '普通回复，没有路径' }], true)
    const cm = mount(ContextMenu, { attachTo: document.body })

    const bubble = w.find('.message-content')
    expect(bubble.exists()).toBe(true)
    bubble.element.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
    )
    await flushPromises()

    const texts = Array.from(document.querySelectorAll('.ctx-item')).map((b) =>
      (b.textContent ?? '').trim(),
    )
    expect(texts).toEqual(['复制', '全选'])
    cm.unmount()
    w.unmount()
  })

  // 2026-11-14 复制路径解析化（B3）：复制的是 fs.resolve 解析后的绝对
  // 路径（与打开/定位同一解析阶梯），不是 data-path 原文。
  it('点「复制路径」→ fs.resolve 带 session_id + 剪贴板收解析后绝对路径', async () => {
    const w = await mountWithHistory(
      [{ role: 'assistant', content: '报告已生成：C:/ws/reports/a.md 请查收' }],
      true,
    )
    const cm = mount(ContextMenu, { attachTo: document.body })
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    requestMock.mockResolvedValue({ path: 'C:/resolved/reports/a.md', kind: 'file', exists: true })

    const link = w.find('a.file-link')
    link.element.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
    await flushPromises()
    const copy = Array.from(document.querySelectorAll('.ctx-item')).find((b) =>
      b.textContent?.includes('复制路径'),
    ) as HTMLElement
    copy.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('fs', 'resolve', {
      path: 'C:/ws/reports/a.md',
      session_id: 's1',
    })
    expect(writeText).toHaveBeenCalledWith('C:/resolved/reports/a.md')
    cm.unmount()
    w.unmount()
  })

  it('exists=false（祖先回退）→ 复制所在目录路径 + warn 注记', async () => {
    const w = await mountWithHistory(
      [{ role: 'assistant', content: '报告已生成：C:/ws/reports/a.md 请查收' }],
      true,
    )
    const cm = mount(ContextMenu, { attachTo: document.body })
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    requestMock.mockResolvedValue({ path: 'C:/resolved', kind: 'dir', exists: false })

    const link = w.find('a.file-link')
    link.element.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
    await flushPromises()
    const copy = Array.from(document.querySelectorAll('.ctx-item')).find((b) =>
      b.textContent?.includes('复制路径'),
    ) as HTMLElement
    copy.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await flushPromises()

    expect(writeText).toHaveBeenCalledWith('C:/resolved')
    const toasts = useToast().toasts
    const last = toasts[toasts.length - 1]
    expect(last.type, JSON.stringify(toasts)).toBe('warn')
    expect(last.message).toContain('C:/resolved')
    cm.unmount()
    w.unmount()
  })

  // B4：LAN HTTP 部署 navigator.clipboard 缺席/被拒 → execCommand 隐藏
  // textarea 回退通路（jsdom 无 execCommand 实现即抛错 → 诚实「复制失败」，
  // 不炸不假成功）。
  it('clipboard 缺席 → 回退通路失败面诚实报错（不再恒败无提示）', async () => {
    const w = await mountWithHistory(
      [{ role: 'assistant', content: '报告已生成：C:/ws/reports/a.md 请查收' }],
      true,
    )
    const cm = mount(ContextMenu, { attachTo: document.body })
    Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true })
    requestMock.mockResolvedValue({ path: 'C:/resolved/reports/a.md', kind: 'file', exists: true })

    const link = w.find('a.file-link')
    link.element.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
    await flushPromises()
    const copy = Array.from(document.querySelectorAll('.ctx-item')).find((b) =>
      b.textContent?.includes('复制路径'),
    ) as HTMLElement
    copy.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await flushPromises()

    const toasts = useToast().toasts
    const last = toasts[toasts.length - 1]
    expect(last.type, JSON.stringify(toasts)).toBe('error')
    cm.unmount()
    w.unmount()
  })
})
