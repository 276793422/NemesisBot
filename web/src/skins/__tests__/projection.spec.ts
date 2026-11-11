import { flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { nextTick, reactive, ref } from 'vue'
import type { Router } from 'vue-router'

// 皮肤数据投影 + 动作白名单装配（projection.ts）单测。
//
// 架构说明：
// - vi.mock 的模块实例在 vi.resetModules() 后**不会**重建（mock 注册表
//   独立持久）——所以共享 mock 状态全部经 vi.hoisted 的 h.* 持有，并在
//   beforeEach 里整体翻新；useSkin 用 getter 导出 skinState/engine，
//   让每次重新求值的 projection 模块绑定到当前实例（旧模块实例的
//   watcher 绑定旧对象，随测试翻新自动失活，不互相开火）。
// - beforeEach vi.resetModules() + 动态 import：每个测试拿全新 projection
//   模块实例（wired/sigLoaded/mdCache/estopTimer 等模块级单例一并复位）。
// - 纯 TS 模块：不 mount 组件，直接操纵 reactive 依赖驱动 watchEffect /
//   watch 分支，动作经 engine.actionHandlers 白名单逐个触发。

const h = vi.hoisted(() => ({
  request: vi.fn(),
  httpGet: vi.fn(),
  bridgeSend: vi.fn(),
  bridgeStop: vi.fn(),
  renderMarkdownHtml: vi.fn((c: string) => `<p>${c}</p>`),
  toggleTheme: vi.fn(),
  logout: vi.fn(),
  sessionCreate: vi.fn(),
  sessionSwitchTo: vi.fn(),
  sessionRemove: vi.fn(),
  sessionFetchList: vi.fn(),
  sessionFetchProjects: vi.fn(),
  toastError: vi.fn(),
  themeRef: null as any,
  fullAccessRef: null as any,
  appState: null as any,
  chatState: null as any,
  sessionState: null as any,
  skinState: null as any,
  engine: null as any,
}))

vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...a: unknown[]) => h.request(...a) }),
}))

vi.mock('../../composables/useToast', () => ({
  useToast: () => ({ toasts: [], error: h.toastError, info: vi.fn(), success: vi.fn(), warn: vi.fn(), remove: vi.fn() }),
}))

vi.mock('../../composables/useTheme', () => ({
  useTheme: () => ({ theme: h.themeRef, toggleTheme: h.toggleTheme, setTheme: vi.fn() }),
}))

vi.mock('../../composables/useEditorMode', () => ({
  // getter：ref 随 beforeEach 翻新，重新求值的模块取当前实例
  get fullAccess() {
    return h.fullAccessRef
  },
}))

vi.mock('../../composables/useWebSocket', () => ({ httpGet: h.httpGet }))

vi.mock('../../stores/app', () => ({ useAppStore: () => h.appState }))

vi.mock('../../stores/auth', () => ({
  useAuthStore: () => ({ logout: h.logout, token: '', authenticated: true }),
}))

vi.mock('../../stores/chat', () => ({ useChatStore: () => h.chatState }))

vi.mock('../../stores/session', () => ({ useSessionStore: () => h.sessionState }))

// skinState / engine 经 getter 导出：projection 每次 resetModules 重新求值
// 时都读到**当前**实例（旧 watcher 绑定旧对象，翻新后自动失活）。
vi.mock('../../composables/useSkin', () => ({
  get skinState() {
    return h.skinState
  },
  skinHasSlot: (name: string) => h.skinState.slots.includes(name),
  getSkinEngine: () => h.engine,
}))

vi.mock('../../utils/markdown', () => ({ renderMarkdownHtml: h.renderMarkdownHtml }))

vi.mock('../chatBridge', () => ({ bridgeSend: h.bridgeSend, bridgeStop: h.bridgeStop }))

import type { SessionEntry } from '../../composables/useChatApi'

type TestRouter = Router & {
  currentRoute: { value: { path: string } }
  push: ReturnType<typeof vi.fn>
}

function makeRouter(initial = '/'): TestRouter {
  return {
    currentRoute: ref<{ path: string }>({ path: initial }),
    push: vi.fn(),
    resolve: vi.fn((p: string) => ({ matched: p === '/cluster' ? [] : [{}] })),
  } as unknown as TestRouter
}

/** 取全新模块实例（wired/sigLoaded/mdCache 等模块级单例一并复位）。 */
async function loadProj() {
  vi.resetModules()
  return await import('../projection')
}

interface SetupOpts {
  slots?: string[]
  path?: string
  meta?: { brand: string; version: string; scenes: string[] } | null
  skinId?: string
  /** 装配之前注入 store 状态（sessions / 路由相关等）。 */
  prep?: () => void
}

async function setupProjection(opts: SetupOpts = {}) {
  const mod = await loadProj()
  opts.prep?.()
  const router = makeRouter(opts.path ?? '/')
  if (opts.slots) h.skinState.slots = [...opts.slots]
  if (opts.meta !== undefined) h.skinState.meta = opts.meta
  if (opts.skinId) h.skinState.id = opts.skinId
  mod.setupSkinProjection(router)
  await flushPromises()
  return { mod, router }
}

function sess(p: Partial<SessionEntry> & { id: string }): SessionEntry {
  return { title: '', firstMessage: '', lastTime: '', ...p } as SessionEntry
}

const DAY = 86400000

function startOfToday(): number {
  const d = new Date()
  d.setHours(0, 0, 0, 0)
  return d.getTime()
}

/** 与 projection.ts relTime 同口径的期望值（渲染后同一时刻求值）。 */
function expectedRel(t: string): string {
  if (!t) return ''
  const ms = Date.parse(t)
  if (Number.isNaN(ms)) return ''
  const diff = Date.now() - ms
  if (diff < 60000) return '刚刚'
  if (diff < 3600000) return `${Math.floor(diff / 60000)} 分钟前`
  if (diff < 86400000) return `${Math.floor(diff / 3600000)} 小时前`
  return `${Math.floor(diff / DAY)} 天前`
}

beforeEach(() => {
  vi.resetModules()
  // 共享 mock 状态整体翻新（mock 工厂不重跑——注册表持久，getter 取当前值）
  h.themeRef = ref<'dark' | 'light'>('dark')
  h.fullAccessRef = ref(false)
  h.appState = reactive({ sidebarCollapsed: false, focusMode: false, showMobileSidebar: false, connected: false })
  h.chatState = reactive({
    streaming: false,
    input: '',
    agentMode: 'build',
    messages: [] as unknown[],
    focusInputNonce: 0,
  })
  h.sessionState = reactive({
    sessions: [] as SessionEntry[],
    currentId: null as string | null,
    create: h.sessionCreate,
    switchTo: h.sessionSwitchTo,
    remove: h.sessionRemove,
    fetchList: h.sessionFetchList,
    fetchProjects: h.sessionFetchProjects,
  })
  h.skinState = reactive({ id: '', meta: null, slots: [] as string[], structRev: 0, scriptPending: '' })
  h.engine = {
    projection: null,
    actionHandlers: new Map(),
    fieldSinks: new Map(),
    local: reactive<Record<string, boolean>>({}),
  }
  h.request.mockReset()
  h.httpGet.mockReset()
  h.httpGet.mockResolvedValue({})
  h.bridgeSend.mockReset()
  h.bridgeStop.mockReset()
  h.renderMarkdownHtml.mockClear()
  h.toggleTheme.mockReset()
  h.logout.mockReset()
  h.sessionCreate.mockReset()
  h.sessionSwitchTo.mockReset()
  h.sessionRemove.mockReset()
  h.sessionFetchList.mockReset()
  h.sessionFetchProjects.mockReset()
  h.toastError.mockReset()
  localStorage.removeItem('nemesisbot_auth_token')
})

afterEach(() => {
  vi.unstubAllEnvs()
  vi.unstubAllGlobals()
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('skinProjection 模块单例缺省值', () => {
  it('导入即有完整缺省投影（引擎消费前不炸）', async () => {
    const { skinProjection } = await loadProj()
    expect(skinProjection.brand).toBe('')
    expect(skinProjection.connectionText).toBe('未连接')
    expect(skinProjection.readyText).toBe('离线')
    expect(skinProjection.estopLabel).toBe('急停')
    expect(skinProjection.estopBusy).toBe(false)
    expect(skinProjection.signature).toEqual({ visible: false, state: 'unsigned', label: '', title: '' })
    expect(skinProjection.pages).toEqual({})
    expect(skinProjection.mode).toBe('build')
    expect(skinProjection.hasSessions).toBe(false)
    expect(skinProjection.themeToggleLabel).toBe('')
    expect(skinProjection.messages).toEqual([])
  })
})

describe('setupSkinProjection 装配', () => {
  it('引擎挂投影 + 10 动作 / chat.input sink 全注册；meta 投影；重复调用幂等 no-op', async () => {
    const { mod } = await setupProjection({
      meta: { brand: 'WB 品牌', version: '2.0.0', scenes: ['写代码', '查资料'] },
    })
    expect(h.engine.projection).toBe(mod.skinProjection)
    expect([...h.engine.actionHandlers.keys()].sort()).toEqual(
      [
        'chat-send',
        'chat-stop',
        'estop-toggle',
        'fill-input',
        'logout',
        'new-chat',
        'remove-session',
        'route',
        'switch-session',
        'toggle-theme',
      ].sort()
    )
    expect([...h.engine.fieldSinks.keys()]).toEqual(['chat.input'])
    expect(mod.skinProjection.brand).toBe('WB 品牌')
    expect(mod.skinProjection.version).toBe('2.0.0')
    expect(mod.skinProjection.scenes).toEqual(['写代码', '查资料'])
    // 主导航 4 项（active 待路由）
    expect(mod.skinProjection.navPrimary.map(n => n.path)).toEqual(['/persona', '/coding', '/skills', '/tasks'])
    expect(mod.skinProjection.navPrimary.every(n => n.active === false)).toBe(true)
    expect(mod.skinProjection.navPrimary[0]!.icon).toContain('M12')
    // 无会话 / 未连接缺省
    expect(mod.skinProjection.hasSessions).toBe(false)
    expect(mod.skinProjection.currentSessionId).toBe('')
    expect(mod.skinProjection.connectionText).toBe('未连接')
    // 幂等：二次装配 no-op（navMore 引用不变、注册表不重复扩充）
    const navMoreBefore = mod.skinProjection.navMore
    const sizeBefore = h.engine.actionHandlers.size
    mod.setupSkinProjection(makeRouter())
    expect(h.engine.actionHandlers.size).toBe(sizeBefore)
    expect(mod.skinProjection.navMore).toBe(navMoreBefore)
  })

  it('meta 缺省回落 skinState.id；connected/抽屉/fullAccess/theme 全部跟随（双分支）', async () => {
    const { mod } = await setupProjection({ skinId: 'fallback-id' })
    expect(mod.skinProjection.brand).toBe('fallback-id')
    expect(mod.skinProjection.version).toBe('')
    expect(mod.skinProjection.scenes).toEqual([])

    h.appState.connected = true
    h.appState.showMobileSidebar = true
    await nextTick()
    expect(mod.skinProjection.connected).toBe(true)
    expect(mod.skinProjection.connectionText).toBe('已连接')
    expect(mod.skinProjection.readyText).toBe('就绪')
    expect(mod.skinProjection.mobileOpen).toBe(true)

    h.fullAccessRef.value = true
    h.themeRef.value = 'light'
    await nextTick()
    expect(mod.skinProjection.fullAccess).toBe(true)
    expect(mod.skinProjection.theme).toBe('light')
    expect(mod.skinProjection.themeToggleLabel).toBe('切换到暗色模式')
    h.themeRef.value = 'dark'
    await nextTick()
    expect(mod.skinProjection.themeToggleLabel).toBe('切换到亮色模式')
  })

  it('navPrimary active 跟随路由 path', async () => {
    const { mod, router } = await setupProjection()
    router.currentRoute.value = { path: '/coding' } as any
    await nextTick()
    expect(mod.skinProjection.navPrimary.find(n => n.path === '/coding')!.active).toBe(true)
    expect(mod.skinProjection.navPrimary.filter(n => n.active)).toHaveLength(1)
  })

  it('navMore：feature=false 裁剪 + 未注册路由剔除 + feature 非 false 保留', async () => {
    vi.stubEnv('VITE_FEATURE_USAGE', 'false')
    vi.stubEnv('VITE_FEATURE_FORGE', 'true')
    const { mod } = await setupProjection()
    const paths = mod.skinProjection.navMore.map(n => n.path)
    // 全量 26 项 − usage（feature=false）− cluster（未注册路由）
    expect(paths).toHaveLength(24)
    expect(paths).not.toContain('/usage')
    expect(paths).not.toContain('/cluster')
    expect(paths).toContain('/forge') // feature=true 放行
    expect(paths).toContain('/overview') // 无 feature 门控
    expect(paths).toContain('/license')
  })

  it('会话分组：今天/昨天/7天内/更早 + relTime 四档 + 标题三级回退 + active 随路由', async () => {
    const today = startOfToday()
    const tJust = Date.now() - 30000
    const tYesterday = today - 3600000
    const tWeek = today - 3 * DAY
    const tOld = today - 40 * DAY
    const iso = (t: number) => new Date(t).toISOString()
    const { mod, router } = await setupProjection({
      path: '/',
      prep: () => {
        h.sessionState.sessions = [
          sess({ id: 'aaaaaaaa-1', title: 'T1', lastTime: iso(tJust) }),
          sess({ id: 'bbbbbbbb-2', title: '', firstMessage: 'FM2', lastTime: iso(tYesterday) }),
          sess({ id: 'cccccccc-3', firstMessage: 'FM3', lastTime: iso(tWeek) }),
          sess({ id: 'dddddddd-4', lastTime: iso(tOld) }),
          sess({ id: 'eeeeeeee-5', title: '无时间' }),
          sess({ id: 'ffffffff-6', title: '', firstMessage: '', lastTime: 'not-a-date' }),
          sess({ id: 'gggggggg-rest', title: '', firstMessage: '' }),
        ]
        h.sessionState.currentId = 'aaaaaaaa-1'
      },
    })

    const groups = mod.skinProjection.sessionGroups
    const byLabel = Object.fromEntries(groups.map(g => [g.label, g.sessions]))
    expect(groups.map(g => g.label)).toEqual(['今天', '昨天', '7 天内', '更早'])
    // 今天 + relTime 刚刚
    const s1 = byLabel['今天'].find(s => s.id === 'aaaaaaaa-1')!
    expect(s1.title).toBe('T1')
    expect(s1.relTime).toBe('刚刚')
    expect(s1.active).toBe(true)
    expect(s1.pinned).toBe(false)
    // 昨天：标题回退 firstMessage；relTime（1h~25h 区间，按渲染口径动态断言）
    const s2 = byLabel['昨天'].find(s => s.id === 'bbbbbbbb-2')!
    expect(s2.title).toBe('FM2')
    expect(s2.relTime).toBe(expectedRel(iso(tYesterday)))
    // 7 天内 / 更早：分组锚定 startOfToday → 天数确定（3 / 40）
    const s3 = byLabel['7 天内'].find(s => s.id === 'cccccccc-3')!
    expect(s3.relTime).toBe(expectedRel(iso(tWeek)))
    const earlier = byLabel['更早']
    expect(earlier.map(s => s.id)).toEqual(['dddddddd-4', 'eeeeeeee-5', 'ffffffff-6', 'gggggggg-rest'])
    expect(earlier.find(s => s.id === 'dddddddd-4')!.relTime).toBe(expectedRel(iso(tOld)))
    expect(earlier.find(s => s.id === 'eeeeeeee-5')!.relTime).toBe('')
    expect(earlier.find(s => s.id === 'ffffffff-6')!.relTime).toBe('')
    // 标题最后回退 id 前 8 位
    expect(earlier.find(s => s.id === 'gggggggg-rest')!.title).toBe('gggggggg')
    expect(mod.skinProjection.hasSessions).toBe(true)
    expect(mod.skinProjection.currentSessionId).toBe('aaaaaaaa-1')

    // 路由离开 / → active 全灭
    router.currentRoute.value = { path: '/settings' } as any
    await nextTick()
    expect(mod.skinProjection.sessionGroups[0]!.sessions[0]!.active).toBe(false)
  })

  it('chat 切片：无 chat 槽置空；有槽映射（assistant markdown / 角色 / 缺省字段）+ busy/input/mode 跟随', async () => {
    const { mod } = await setupProjection()
    h.chatState.messages = [
      { role: 'user', content: 'u1', timestamp: 't0', rowIndex: 7 },
      { role: 'assistant', content: 'a1', timestamp: 't1' },
      { role: 'error', content: 'e1', timestamp: 't2', model: 'prov/mdl', sourceNode: 'node-x', imageCount: 2 },
    ]
    h.chatState.streaming = true
    h.chatState.input = '在打字'
    h.chatState.agentMode = 'plan'
    await nextTick()
    // 无槽：置空（渲染成本不花）
    expect(mod.skinProjection.messages).toEqual([])
    expect(mod.skinProjection.chatBusy).toBe(true)
    expect(mod.skinProjection.inputText).toBe('在打字')
    expect(mod.skinProjection.mode).toBe('plan')

    h.skinState.slots = ['chat']
    await nextTick()
    const msgs = mod.skinProjection.messages
    expect(msgs).toHaveLength(3)
    expect(msgs[0]).toMatchObject({ id: 7, role: 'user', isUser: true, isError: false, content: 'u1', contentHtml: '' })
    expect(msgs[1]).toMatchObject({ id: 1, role: 'assistant', isUser: false, isError: false, contentHtml: '<p>a1</p>' })
    expect(msgs[2]).toMatchObject({
      id: 2,
      role: 'error',
      isUser: false,
      isError: true,
      contentHtml: '',
      model: 'prov/mdl',
      sourceNode: 'node-x',
      imageCount: 2,
    })
    expect(h.renderMarkdownHtml).toHaveBeenCalledTimes(1)
    expect(h.renderMarkdownHtml).toHaveBeenCalledWith('a1', { breaks: true })

    // 同内容再来一条 → markdown 缓存命中（不再调 renderMarkdownHtml）
    h.chatState.messages = [...h.chatState.messages, { role: 'assistant', content: 'a1', timestamp: 't3' }]
    await nextTick()
    expect(mod.skinProjection.messages).toHaveLength(4)
    expect(h.renderMarkdownHtml).toHaveBeenCalledTimes(1)
    expect(mod.skinProjection.messages[3]!.contentHtml).toBe('<p>a1</p>')

    h.chatState.streaming = false
    h.chatState.input = ''
    h.chatState.agentMode = 'build'
    await nextTick()
    expect(mod.skinProjection.chatBusy).toBe(false)
    expect(mod.skinProjection.inputText).toBe('')
    expect(mod.skinProjection.mode).toBe('build')
  })

  it('chat markdown 缓存超 500 条整体清空重渲', async () => {
    const { mod } = await setupProjection({ slots: ['chat'] })
    const first = Array.from({ length: 501 }, (_, i) => ({ role: 'assistant', content: `m${i}`, timestamp: 't' }))
    h.chatState.messages = first
    await nextTick()
    expect(h.renderMarkdownHtml).toHaveBeenCalledTimes(501)
    // 追加 1 条触发重跑：cache 501 > 500 → 整体清空 → 502 条全量重渲
    h.chatState.messages = [...first, { role: 'assistant', content: 'm501', timestamp: 't' }]
    await nextTick()
    expect(mod.skinProjection.messages).toHaveLength(502)
    expect(h.renderMarkdownHtml).toHaveBeenCalledTimes(501 + 502)
  })

  it('chat.input 字段 sink 是唯一写通道', async () => {
    await setupProjection()
    const sink = h.engine.fieldSinks.get('chat.input')!
    sink('hello')
    expect(h.chatState.input).toBe('hello')
    sink('')
    expect(h.chatState.input).toBe('')
  })
})

describe('动作白名单', () => {
  async function actionSetup() {
    return await setupProjection({ slots: ['sidebar'] })
  }

  it('route：无参 no-op；有参关 flyout/抽屉并 push', async () => {
    const { router } = await actionSetup()
    const route = h.engine.actionHandlers.get('route')!
    route(undefined)
    expect(router.push).not.toHaveBeenCalled()
    h.engine.local.moreOpen = true
    h.appState.showMobileSidebar = true
    route('/settings')
    expect(h.engine.local.moreOpen).toBe(false)
    expect(h.appState.showMobileSidebar).toBe(false)
    expect(router.push).toHaveBeenCalledWith('/settings')
  })

  it('new-chat：成功 push /；失败 toast（sid 为空）', async () => {
    const { router } = await actionSetup()
    const newChat = h.engine.actionHandlers.get('new-chat')!
    h.sessionCreate.mockResolvedValue('sid-1')
    newChat(undefined)
    await flushPromises()
    expect(h.sessionCreate).toHaveBeenCalled()
    expect(router.push).toHaveBeenCalledWith('/')

    h.sessionCreate.mockResolvedValue(null)
    newChat(undefined)
    await flushPromises()
    expect(h.toastError).toHaveBeenCalledWith('新建会话失败')
  })

  it('switch-session：无参 no-op；有参 switchTo + 关抽屉 + push /', async () => {
    const { router } = await actionSetup()
    const sw = h.engine.actionHandlers.get('switch-session')!
    sw(undefined)
    expect(h.sessionSwitchTo).not.toHaveBeenCalled()
    h.appState.showMobileSidebar = true
    sw('sid-2')
    expect(h.sessionSwitchTo).toHaveBeenCalledWith('sid-2')
    expect(h.appState.showMobileSidebar).toBe(false)
    expect(router.push).toHaveBeenCalledWith('/')
  })

  it('remove-session：找不到 no-op；confirm 取消不删；确认后 remove', async () => {
    await actionSetup()
    const removeSession = h.engine.actionHandlers.get('remove-session')!
    h.sessionState.sessions = [sess({ id: 's1', title: '标题A', lastTime: new Date().toISOString() })]
    const confirmSpy = vi.fn(() => true)
    vi.stubGlobal('confirm', confirmSpy)

    removeSession('missing')
    expect(confirmSpy).not.toHaveBeenCalled()
    expect(h.sessionRemove).not.toHaveBeenCalled()

    confirmSpy.mockReturnValueOnce(false)
    removeSession('s1')
    expect(confirmSpy).toHaveBeenCalledWith('删除会话「标题A」？此操作不可恢复。')
    expect(h.sessionRemove).not.toHaveBeenCalled()

    removeSession('s1')
    expect(h.sessionRemove).toHaveBeenCalledWith('s1')
  })

  it('fill-input：arg 加「：」后缀；空 arg 清空；nonce 递增', async () => {
    await actionSetup()
    const fill = h.engine.actionHandlers.get('fill-input')!
    fill('帮我写')
    expect(h.chatState.input).toBe('帮我写：')
    expect(h.chatState.focusInputNonce).toBe(1)
    fill('')
    expect(h.chatState.input).toBe('')
    expect(h.chatState.focusInputNonce).toBe(2)
  })

  it('chat-send / chat-stop 走桥；toggle-theme / logout 走对应回调', async () => {
    await actionSetup()
    const actions = h.engine.actionHandlers
    actions.get('chat-send')!('')
    actions.get('chat-stop')!('')
    expect(h.bridgeSend).toHaveBeenCalledTimes(1)
    expect(h.bridgeStop).toHaveBeenCalledTimes(1)
    actions.get('toggle-theme')!('')
    expect(h.toggleTheme).toHaveBeenCalledTimes(1)
    actions.get('logout')!('')
    expect(h.logout).toHaveBeenCalledTimes(1)
  })
})

describe('pages 装载器（v3 page 槽）', () => {
  it('路由激活且槽在场 → WSAPI 响应原文落盘；失败 / 无槽 / 非注册路由不拉', async () => {
    const { mod, router } = await setupProjection({ slots: ['page:/models'] })
    // '/' 无装载器 → 不拉
    expect(h.request).not.toHaveBeenCalled()
    h.request.mockResolvedValue({ profiles: [1] })
    router.currentRoute.value = { path: '/models' } as any
    await flushPromises()
    expect(h.request).toHaveBeenCalledWith('models', 'list', {}, 8000)
    expect(mod.skinProjection.pages['/models']).toEqual({ profiles: [1] })

    // 响应 null → 回落 {}
    h.skinState.slots = ['page:/persona']
    h.request.mockResolvedValue(null)
    router.currentRoute.value = { path: '/persona' } as any
    await flushPromises()
    expect(h.request).toHaveBeenCalledWith('persona', 'list', {}, 8000)
    expect(mod.skinProjection.pages['/persona']).toEqual({})

    // 槽不在场：不拉
    h.request.mockClear()
    h.skinState.slots = []
    router.currentRoute.value = { path: '/models' } as any
    await flushPromises()
    expect(h.request).not.toHaveBeenCalled()

    // 非注册路由：不拉
    router.currentRoute.value = { path: '/settings' } as any
    await flushPromises()
    expect(h.request).not.toHaveBeenCalled()
  })

  it('竞态守卫：切走丢旧响应；seq 不匹配丢旧；慢响应失败静默保留旧数据', async () => {
    const { mod, router } = await setupProjection({ slots: ['page:/models', 'page:/skills'] })
    const defs: Array<{ resolve: (v: unknown) => void; reject: (e: unknown) => void }> = []
    let seq = 0
    h.request.mockImplementation(() => {
      let resolve!: (v: unknown) => void
      let reject!: (e: unknown) => void
      const p = new Promise<unknown>((res, rej) => {
        resolve = res
        reject = rej
      })
      void p.catch(() => {}) // 防未处理拒绝噪音
      defs[seq++] = { resolve, reject }
      return p
    })

    // load1（/models）在飞 → 切到 /skills（load2）→ load1 到达时路由已走 → 丢弃
    router.currentRoute.value = { path: '/models' } as any
    await nextTick()
    router.currentRoute.value = { path: '/skills' } as any
    await nextTick()
    expect(defs).toHaveLength(2)
    defs[0]!.resolve({ a: 1 })
    await flushPromises()
    expect(mod.skinProjection.pages['/models']).toBeUndefined()
    defs[1]!.resolve({ b: 2 })
    await flushPromises()
    expect(mod.skinProjection.pages['/skills']).toEqual({ b: 2 })

    // seq 守卫：/models 走一轮 → 切走再切回（seq++）→ 旧响应到达被丢
    defs.length = 0
    seq = 0
    router.currentRoute.value = { path: '/models' } as any
    await nextTick()
    router.currentRoute.value = { path: '/x' } as any
    await nextTick()
    router.currentRoute.value = { path: '/models' } as any
    await nextTick()
    expect(defs).toHaveLength(2)
    defs[0]!.resolve({ stale: true })
    await flushPromises()
    expect(mod.skinProjection.pages['/models']).toBeUndefined() // 旧一轮被丢
    defs[1]!.resolve({ fresh: true })
    await flushPromises()
    expect(mod.skinProjection.pages['/models']).toEqual({ fresh: true })

    // 失败静默：pages 保留旧数据
    defs.length = 0
    seq = 0
    router.currentRoute.value = { path: '/skills' } as any
    await nextTick()
    defs[0]!.reject(new Error('ws down'))
    await flushPromises()
    expect(mod.skinProjection.pages['/skills']).toEqual({ b: 2 })
  })
})

describe('E-Stop + 签名（sidebar 槽轮询）', () => {
  it('sidebar 在场启动 10s 轮询；estop.status 成功投影；离场清定时器 + 重新入场再拉', async () => {
    const intervalSpy = vi.spyOn(window, 'setInterval')
    const clearIntervalSpy = vi.spyOn(window, 'clearInterval')
    h.request.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve({ engaged: true })
      return Promise.reject(new Error('sig down'))
    })
    const { mod } = await setupProjection({ slots: ['sidebar'] })
    expect(h.sessionFetchList).toHaveBeenCalled()
    expect(h.sessionFetchProjects).toHaveBeenCalled()
    expect(intervalSpy).toHaveBeenCalledWith(expect.any(Function), 10000)
    expect(mod.skinProjection.estopEngaged).toBe(true)
    expect(mod.skinProjection.estopLabel).toBe('释放急停')
    // 签名请求失败（WS 未就绪）→ 保持未加载 → 徽标不显示
    expect(mod.skinProjection.signature.visible).toBe(false)

    // 离场：清定时器 + projectSignature（未加载 → unsigned）
    h.skinState.slots = []
    await nextTick()
    expect(clearIntervalSpy).toHaveBeenCalled()
    expect(mod.skinProjection.signature).toEqual({ visible: false, state: 'unsigned', label: '', title: '' })

    // 重新入场：再次拉取（engaged=false → 急停）
    h.request.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve({ engaged: false })
      return Promise.reject(new Error('sig down'))
    })
    h.skinState.slots = ['sidebar']
    await flushPromises()
    expect(mod.skinProjection.estopEngaged).toBe(false)
    expect(mod.skinProjection.estopLabel).toBe('急停')
  })

  it('10s 轮询周期触发 refreshEstop + refreshSigVerify（手动驱动 interval 回调）', async () => {
    const intervalSpy = vi.spyOn(window, 'setInterval')
    h.request.mockResolvedValue({ engaged: false })
    await setupProjection({ slots: ['sidebar'] })
    const estopCalls = () => h.request.mock.calls.filter(c => c[0] === 'estop').length
    const sigCalls = () => h.request.mock.calls.filter(c => c[0] === 'security').length
    expect(estopCalls()).toBe(1)
    expect(sigCalls()).toBe(1)
    const tick = intervalSpy.mock.calls[0]![0] as () => void
    tick()
    await flushPromises()
    expect(estopCalls()).toBe(2)
    expect(sigCalls()).toBe(2)
    // estop.status 拒绝 → 静默保持原状态，下个轮询周期再试
    h.request.mockRejectedValueOnce(new Error('ws down'))
    tick()
    await flushPromises()
    expect(estopCalls()).toBe(3)
    expect(sigCalls()).toBe(3)
  })

  it('estop-toggle：busy 守卫早退；trigger→release 按 engaged 切换；失败 console.error + busy 复位', async () => {
    const errSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    h.request.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve({ engaged: false })
      if (cmd === 'trigger') return Promise.resolve({ engaged: true })
      if (cmd === 'release') return Promise.resolve({ engaged: false })
      return Promise.reject(new Error('sig down'))
    })
    const { mod } = await setupProjection({ slots: ['sidebar'] })
    const toggle = h.engine.actionHandlers.get('estop-toggle')!

    // busy 守卫
    mod.skinProjection.estopBusy = true
    toggle('')
    await flushPromises()
    expect(h.request.mock.calls.filter(c => c[0] === 'estop' && c[1] !== 'status')).toHaveLength(0)
    mod.skinProjection.estopBusy = false

    // engaged=false → trigger
    toggle('')
    await flushPromises()
    expect(h.request).toHaveBeenCalledWith('estop', 'trigger', {}, 5000)
    expect(mod.skinProjection.estopEngaged).toBe(true)
    expect(mod.skinProjection.estopBusy).toBe(false)
    expect(mod.skinProjection.estopLabel).toBe('释放急停')

    // engaged=true → release
    toggle('')
    await flushPromises()
    expect(h.request).toHaveBeenCalledWith('estop', 'release', {}, 5000)
    expect(mod.skinProjection.estopEngaged).toBe(false)
    expect(mod.skinProjection.estopLabel).toBe('急停')

    // 失败：console.error + busy 复位
    h.request.mockImplementation((_m: string, cmd: string) =>
      cmd === 'status' ? Promise.resolve({ engaged: false }) : Promise.reject(new Error('boom'))
    )
    toggle('')
    await flushPromises()
    expect(errSpy).toHaveBeenCalledWith('[E-Stop] toggle failed:', expect.any(Error))
    expect(mod.skinProjection.estopBusy).toBe(false)
  })

  it('签名徽标：未加载/未注入不显示；完整加载 verified（锁定文案）+ parts 拼接；已加载再轮询早退', async () => {
    h.request.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve({ engaged: false })
      return Promise.reject(new Error('sig down'))
    })
    const { mod } = await setupProjection({ slots: ['sidebar'] })
    expect(mod.skinProjection.signature.visible).toBe(false)

    // 未注入：不置 loaded（visible 仍 false）
    h.request.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve({ engaged: false })
      return Promise.resolve({ injected: false })
    })
    h.skinState.slots = []
    await nextTick()
    h.skinState.slots = ['sidebar']
    await flushPromises()
    expect(mod.skinProjection.signature.visible).toBe(false)

    // 完整加载：Valid + 锁定 + keyFp + detail
    h.request.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve({ engaged: false })
      return Promise.resolve({
        injected: true,
        mode: 'enforce',
        locked: true,
        last_result: 'Valid',
        key_fp: 'ABCD1234',
        detail: '详情行',
      })
    })
    h.skinState.slots = []
    await nextTick()
    h.skinState.slots = ['sidebar']
    await flushPromises()
    expect(mod.skinProjection.signature).toEqual({
      visible: true,
      state: 'verified',
      label: '🔒 签名已验证',
      title: '模式: enforce（锁定版，config 不可关）\n启动自验: Valid\n签名者: ABCD1234\n详情行',
    })

    // 已加载：再轮询早退（不再发签名请求，徽标保持）
    h.skinState.slots = []
    await nextTick()
    h.skinState.slots = ['sidebar']
    await flushPromises()
    const sigCalls = h.request.mock.calls.filter(c => c[0] === 'security').length
    h.skinState.slots = []
    await nextTick()
    h.skinState.slots = ['sidebar']
    await flushPromises()
    expect(h.request.mock.calls.filter(c => c[0] === 'security').length).toBe(sigCalls)
    // 离场 projectSignature：已加载 → 仍 verified
    expect(mod.skinProjection.signature.state).toBe('verified')
  })

  it('签名三色其余分支：Tampered / mode=off / 异常形态（各自独立模块实例）', async () => {
    // Tampered → invalid（解锁：label 无锁 emoji；无 keyFp/detail → parts 最小）
    h.request.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve({ engaged: false })
      return Promise.resolve({ injected: true, mode: 'audit', locked: false, last_result: 'Tampered' })
    })
    const a = await setupProjection({ slots: ['sidebar'] })
    expect(a.mod.skinProjection.signature).toEqual({
      visible: true,
      state: 'invalid',
      label: '签名已被篡改',
      title: '模式: audit\n启动自验: Tampered',
    })

    // mode=off + 无结果 → unverified「签名验证 关」
    h.request.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve({ engaged: false })
      return Promise.resolve({ injected: true, mode: 'off', locked: false, last_result: null })
    })
    const b = await setupProjection({ slots: ['sidebar'] })
    expect(b.mod.skinProjection.signature).toEqual({
      visible: true,
      state: 'unverified',
      label: '签名验证 关',
      title: '模式: off',
    })

    // 其余（如 audit + Broken）→ 「签名验证异常」
    h.request.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve({ engaged: false })
      return Promise.resolve({ injected: true, mode: 'audit', locked: false, last_result: 'Broken' })
    })
    const c = await setupProjection({ slots: ['sidebar'] })
    expect(c.mod.skinProjection.signature).toEqual({
      visible: true,
      state: 'unverified',
      label: '签名验证异常',
      title: '模式: audit\n启动自验: Broken',
    })
  })
})

describe('状态栏取数（statusbar 槽）', () => {
  it('无 token 直取；version/model 投影（分隔符非断行空格）；shortModel 去 provider 段；失败静默', async () => {
    const { mod } = await setupProjection({ slots: ['statusbar'] })
    expect(h.httpGet).toHaveBeenCalledWith(expect.stringMatching(/\/api\/status$/))
    h.httpGet.mockResolvedValue({ version: '1.2.3', model: 'prov/gpt-x' })
    h.skinState.slots = []
    await nextTick()
    h.skinState.slots = ['statusbar']
    await flushPromises()
    // 分隔符为源文件里的非断行空格「\u00a0·\u00a0」（宿主拼好对齐原版间距）
    const SEP = '\u00a0·\u00a0'
    expect(mod.skinProjection.statusVersion).toBe(SEP + 'v1.2.3')
    expect(mod.skinProjection.model).toBe(SEP + 'gpt-x')

    // model 无斜杠：原样
    h.httpGet.mockResolvedValue({ version: '', model: 'plain-model' })
    h.skinState.slots = []
    await nextTick()
    h.skinState.slots = ['statusbar']
    await flushPromises()
    expect(mod.skinProjection.statusVersion).toBe('')
    expect(mod.skinProjection.model).toBe(SEP + 'plain-model')

    // 全空 → 两段清空
    h.httpGet.mockResolvedValue({})
    h.skinState.slots = []
    await nextTick()
    h.skinState.slots = ['statusbar']
    await flushPromises()
    expect(mod.skinProjection.statusVersion).toBe('')
    expect(mod.skinProjection.model).toBe('')

    // 失败静默：状态栏保持原值不炸
    h.httpGet.mockRejectedValue(new Error('net down'))
    h.skinState.slots = []
    await nextTick()
    h.skinState.slots = ['statusbar']
    await flushPromises()
    expect(mod.skinProjection.statusVersion).toBe('')
  })

  it('有 token：url 带 ?token=（encodeURIComponent）', async () => {
    localStorage.setItem('nemesisbot_auth_token', 'tk&x=1')
    h.httpGet.mockResolvedValue({})
    await setupProjection({ slots: ['statusbar'] })
    expect(h.httpGet).toHaveBeenCalledWith(expect.stringContaining('/api/status?token=tk%26x%3D1'))
  })
})
