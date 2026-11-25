import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { inject } from 'vue'
import { mount, flushPromises, type VueWrapper } from '@vue/test-utils'

// ChatView 资源看板壳层交互（2026-11-19 改造回归锁）：
// - 滞后带双阈值：≥1284 判宽 / <1180 判窄 / 带间保持原值（拖动防抖）；
//   首屏例外（2026-11-25 挤压修复）：无历史态可「保持」，带间宽度按
//   OPEN_AT 硬判——否则宽判定初值 true 把 1180~1283 误判成宽屏走并排，
//   行内无缩窄保护的点名面板列被挤成 0 宽（1440 视口实录）；
// - 边缘触发自动开：窄→宽穿越那一刻开一次；用户手动收起后不再自动开
//   （意志优先；复位资格 = 手动开，但无自动关场景下不可观测，不单测）；
// - 首屏窄窗口静默收起（不写偏好、不算手动关）——抽屉不盖首屏对话列；
// - overlay 抽屉形态：窄窗口打开 = 悬浮 + 遮罩，对话列不缩（无
//   chat-main-narrow）；遮罩点击 / 抽屉 ✕ 均可收起；
// - 提案 C 注入接线：ChatPanel 经 RESOURCES_PANEL_CTL 注入的开关按钮与
//   ChatView 状态机同源（toggle 驱动同一份开合 + localStorage 同步）；
//   E1（2026-11-19 #6）rail 竖条入口已删——收起后重开走注入开关
//   （任何宽度可用），本 spec 的手动开全部走该入口。

// jsdom 无 ResizeObserver：可注入假件捕获回调，驱动宽度穿越。
const roInstances: FakeRO[] = []
class FakeRO {
  cb: ResizeObserverCallback
  constructor(cb: ResizeObserverCallback) {
    this.cb = cb
    roInstances.push(this)
  }
  observe(el: HTMLElement) {
    FakeRO.observed.push(el)
    // 首帧宽度注入队列（2026-11-25 挤压修复集成锁）：jsdom clientWidth 恒 0，
    // 挂载直调的首屏判宽只能见到 0（恒窄）；生产首屏读到的是真实布局宽。
    // 预置宽度 = observe 时写入并同步触发首帧回调（真实 RO observe 后也会
    // 派发首帧），让「首屏即带间宽度」这条修复路径可测。
    const w = FakeRO.firstWidths.pop()
    if (w !== undefined) {
      Object.defineProperty(el, 'clientWidth', { value: w, configurable: true })
      this.cb([] as any, this as any)
    }
  }
  disconnect() {}
  static observed: HTMLElement[] = []
  static firstWidths: number[] = []
}

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

vi.mock('../../composables/useSSE', () => ({
  on: vi.fn(),
  off: vi.fn(),
}))

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn() }),
}))

import ChatView from '../ChatView.vue'
import { RESOURCES_PANEL_CTL, resolveResourcesWide } from '../../composables/useResourcesPanel'
import { useSessionStore } from '../../stores/session'

// 注入消费桩：模拟 ChatPanel 的资源窗开关按钮（提案 C 接线验证对象，
// 也是 rail 删除后唯一的手动开入口）——import 同一个 Symbol key 注入，
// 非顶层 ref 在模板里显式 .value。
const InjectConsumer = {
  setup() {
    const ctl = inject(RESOURCES_PANEL_CTL, null)
    return { ctl }
  },
  template: `<button v-if="ctl" class="injected-toggle" @click="ctl.toggle()">{{ ctl.open.value ? '收起资源窗' : '打开资源窗' }}</button>`,
}

/** 设定布局宽度并触发 ResizeObserver 回调（jsdom clientWidth 恒 0 的补位）。 */
function setLayoutWidth(w: number): void {
  const el = FakeRO.observed[FakeRO.observed.length - 1]
  Object.defineProperty(el, 'clientWidth', { value: w, configurable: true })
  roInstances[roInstances.length - 1].cb([] as any, roInstances[roInstances.length - 1] as any)
}

function mountView(chatPanelStub?: Record<string, unknown>): VueWrapper<any> {
  return mount(ChatView, {
    global: {
      stubs: {
        // 重子组件桩掉（侧栏/文件树各自的重挂载面与本 spec 无关）；
        // ChatPanel 缺省壳桩 / SessionResourcesPanel 保真（overlay 形态断言对象）。
        ChatPanel: chatPanelStub ?? { template: '<div class="page-chat" />' },
        SessionSidebar: true,
        FileTreePanel: true,
        BrowserPickPanel: true,
      },
    },
  })
}

beforeEach(() => {
  setActivePinia(createPinia())
  localStorage.clear()
  requestMock.mockReset().mockResolvedValue({})
  void useSessionStore().fetchList?.()
  roInstances.length = 0
  FakeRO.observed.length = 0
  FakeRO.firstWidths.length = 0
  vi.stubGlobal('ResizeObserver', FakeRO)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('ChatView 资源看板 · 滞后带与自动开', () => {
  it('首屏窄窗口 + 默认展开态 → 静默收起（不写偏好；rail 已删、注入开关在场可重开）', async () => {
    const w = mountView(InjectConsumer)
    await flushPromises()
    setLayoutWidth(1000)
    await flushPromises()
    expect(w.find('.chat-resources-panel').exists()).toBe(false)
    // E1（#6）rail 竖条入口已删：DOM 中不存在（回归锁）。
    expect(w.find('.chat-resources-rail').exists()).toBe(false)
    // 重开入口 = ChatPanel 注入的开关按钮（任何宽度可用）。
    expect(w.find('.injected-toggle').exists()).toBe(true)
    expect(w.find('.injected-toggle').text()).toContain('打开资源窗')
    // 不写偏好：localStorage 保持无记录。
    expect(localStorage.getItem('nb_resources_panel')).toBe(null)
    w.unmount()
  })

  it('窄→宽穿越 → 边缘触发自动开（偏好写 1）；带间移动不翻转', async () => {
    const w = mountView()
    await flushPromises()
    setLayoutWidth(1000) // 窄：首屏静默收起
    await flushPromises()
    setLayoutWidth(1200) // 滞后带内（1180~1284）：保持窄判定，不开
    await flushPromises()
    expect(w.find('.chat-resources-panel').exists()).toBe(false)
    setLayoutWidth(1300) // 穿越 OPEN_AT：自动开
    await flushPromises()
    expect(w.find('.chat-resources-panel').exists()).toBe(true)
    expect(localStorage.getItem('nb_resources_panel')).toBe('1')
    // 带间回落：1300→1200（带内保持宽）→1180 以下才翻窄。
    setLayoutWidth(1200)
    await flushPromises()
    expect(w.find('.chat-resources-panel').exists()).toBe(true)
    setLayoutWidth(1100) // 翻窄：不强制收（overlay 形态接管）
    await flushPromises()
    expect(w.find('.chat-resources-panel').exists()).toBe(true)
    w.unmount()
  })

  it('手动收起后宽穿越不再自动开（意志优先）', async () => {
    const w = mountView(InjectConsumer)
    await flushPromises()
    setLayoutWidth(1000)
    await flushPromises()
    // 手动开（注入开关）→手动收（写偏好 0 + 置意志旗）。
    await w.find('.injected-toggle').trigger('click')
    await flushPromises()
    await w.find('.chat-resources-backdrop').trigger('click')
    await flushPromises()
    expect(localStorage.getItem('nb_resources_panel')).toBe('0')
    setLayoutWidth(1300) // 宽穿越：意志旗在场 → 不自动开
    await flushPromises()
    expect(w.find('.chat-resources-panel').exists()).toBe(false)
    // rail 不在场（E1 已删）；重开走注入开关。
    expect(w.find('.chat-resources-rail').exists()).toBe(false)
    expect(w.find('.injected-toggle').text()).toContain('打开资源窗')
    w.unmount()
  })

  it('手动开后窄→宽往返：面板保持（overlay 接管），收起态宽度穿越不再自动开', async () => {
    const w = mountView(InjectConsumer)
    await flushPromises()
    setLayoutWidth(1000)
    await flushPromises()
    await w.find('.injected-toggle').trigger('click')
    await flushPromises()
    // 窄窗口开着 → 收窄方向不强制收（overlay 接管，面板保持）。
    setLayoutWidth(1100)
    await flushPromises()
    expect(w.find('.chat-resources-panel').exists()).toBe(true)
    // 宽方向穿越：已开，无自动开语义；并排形态接管。
    setLayoutWidth(1300)
    await flushPromises()
    expect(w.find('.chat-resources-panel').exists()).toBe(true)
    expect(w.find('.chat-resources-panel').classes()).not.toContain('chat-resources-overlay')
    w.unmount()
  })
})

describe('ChatView 资源看板 · 首屏带间硬判（2026-11-25 挤压修复回归锁）', () => {
  it('resolveResourcesWide：首屏带间宽度按并排下限硬判，不保持初值 true', () => {
    // 1200 落带间 [1180, 1284)（1440 视口可用宽实录）：首屏 → 窄，
    // 走 overlay 不并排——并排挤压无保护列（点名面板 0 宽）的根修点。
    expect(resolveResourcesWide(1200, true, true)).toBe(false)
    expect(resolveResourcesWide(1180, true, true)).toBe(false)
    expect(resolveResourcesWide(1283, true, true)).toBe(false)
    // 阈值两侧语义不变：<CLOSE_AT 窄 / ≥OPEN_AT 宽。
    expect(resolveResourcesWide(1000, true, true)).toBe(false)
    expect(resolveResourcesWide(1300, true, true)).toBe(true)
  })

  it('resolveResourcesWide：非首屏滞后带行为不变（≥1284 宽 / <1180 窄 / 带间保持）', () => {
    expect(resolveResourcesWide(1300, false, false)).toBe(true)
    expect(resolveResourcesWide(1100, false, false)).toBe(false)
    expect(resolveResourcesWide(1200, true, false)).toBe(true)
    expect(resolveResourcesWide(1200, false, false)).toBe(false)
  })

  it('首屏即带间宽度（1440 视口实录形态）：默认展开态被静默收起，拖宽穿越照常自动开', async () => {
    FakeRO.firstWidths.push(1200) // observe 首帧即带间宽（生产 = 真实布局宽）
    const w = mountView(InjectConsumer)
    await flushPromises()
    // 首屏硬判窄 → 默认展开态静默收起（不写偏好、不算手动关）。
    expect(w.find('.chat-resources-panel').exists()).toBe(false)
    expect(localStorage.getItem('nb_resources_panel')).toBe(null)
    // 之后拖宽穿越 OPEN_AT → 边缘触发自动开照常（滞后带语义未伤）。
    setLayoutWidth(1300)
    await flushPromises()
    expect(w.find('.chat-resources-panel').exists()).toBe(true)
    expect(localStorage.getItem('nb_resources_panel')).toBe('1')
    w.unmount()
  })
})

describe('ChatView 资源看板 · overlay 抽屉形态', () => {
  it('窄窗口打开 = overlay 类 + 遮罩 + 对话列不缩（无 chat-main-narrow）', async () => {
    const w = mountView(InjectConsumer)
    await flushPromises()
    setLayoutWidth(1000)
    await flushPromises()
    await w.find('.injected-toggle').trigger('click')
    await flushPromises()
    const panel = w.find('.chat-resources-panel')
    expect(panel.classes()).toContain('chat-resources-overlay')
    expect(w.find('.chat-resources-backdrop').exists()).toBe(true)
    expect(w.find('.chat-main-narrow').exists()).toBe(false)
    w.unmount()
  })

  it('宽窗口打开 = 并排形态（对话列 944 缩窄、无遮罩、无 overlay 类）', async () => {
    const w = mountView()
    await flushPromises()
    setLayoutWidth(1300)
    await flushPromises()
    // 宽窗口首屏自动/默认态：面板已在场（默认展开）。
    const panel = w.find('.chat-resources-panel')
    expect(panel.exists()).toBe(true)
    expect(panel.classes()).not.toContain('chat-resources-overlay')
    expect(w.find('.chat-resources-backdrop').exists()).toBe(false)
    expect(w.find('.chat-main-narrow').exists()).toBe(true)
    w.unmount()
  })

  it('遮罩点击与抽屉 ✕ 均收起（偏好写 0）', async () => {
    const w = mountView(InjectConsumer)
    await flushPromises()
    setLayoutWidth(1000)
    await flushPromises()
    await w.find('.injected-toggle').trigger('click')
    await flushPromises()
    await w.find('.chat-resources-backdrop').trigger('click')
    await flushPromises()
    expect(w.find('.chat-resources-panel').exists()).toBe(false)
    expect(localStorage.getItem('nb_resources_panel')).toBe('0')
    // 再开，走抽屉头部 ✕（SessionResourcesPanel @close 上抛）。
    await w.find('.injected-toggle').trigger('click')
    await flushPromises()
    await w.find('.srp-tools-close').trigger('click')
    await flushPromises()
    expect(w.find('.chat-resources-panel').exists()).toBe(false)
    w.unmount()
  })
})

describe('ChatView 资源看板 · 提案 C 注入接线（2026-11-19）', () => {
  it('ChatPanel 注入的开关按钮 ↔ 状态机同源：toggle 开合 + localStorage 同步', async () => {
    const w = mountView(InjectConsumer)
    await flushPromises()
    setLayoutWidth(1000) // 窄：首屏静默收起 → 注入按钮显示「打开资源窗」
    await flushPromises()
    const btn = () => w.find('.injected-toggle')
    expect(btn().exists()).toBe(true)
    expect(btn().text()).toContain('打开资源窗')
    // 点开（窄窗口 → overlay 形态在场的就是状态机开合的直接证据）。
    await btn().trigger('click')
    await flushPromises()
    expect(w.find('.chat-resources-panel').exists()).toBe(true)
    expect(w.find('.chat-resources-panel').classes()).toContain('chat-resources-overlay')
    expect(localStorage.getItem('nb_resources_panel')).toBe('1')
    expect(btn().text()).toContain('收起资源窗')
    // 再点收起：状态机 + 偏好同步归 0。
    await btn().trigger('click')
    await flushPromises()
    expect(w.find('.chat-resources-panel').exists()).toBe(false)
    expect(localStorage.getItem('nb_resources_panel')).toBe('0')
    w.unmount()
  })
})
