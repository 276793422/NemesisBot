/**
 * 皮肤数据投影 + 动作白名单装配（v2 结构引擎数据面）。
 *
 * 投影（skinProjection）是宿主 → 皮肤的**唯一数据通道**：全部字段宿主
 * 算好（时间分组 / relTime / active / 连接文案 / 版本·模型分隔符……），
 * 引擎只声明式渲染（for/bind/if），皮肤包不做任何数据加工、不持逻辑。
 * 装配点 = AppLayout setup 调 `setupSkinProjection(router)`（幂等；模块级
 * effectScope——AppLayout 卸载不拆投影，皮肤热切 / 组件重挂零重建）。
 *
 * 数据源（全部现成单例，零新协议）：
 * - brand/version/scenes ← skinState.meta（useSkin 的 skins.detail）
 * - connected/connectionText/readyText/mobileOpen ← appStore
 * - statusVersion/model ← GET /api/status（REST 鉴权闸 ?token= 惯例；
 *   自 AppLayout fetchStatusbar 迁入；分隔符「 · 」宿主拼好）
 * - navPrimary/navMore ← 静态导航表（SkinSidebar 迁入 + forge/license
 *   补齐原生覆盖；featureOn + router.resolve matched 双过滤防死链）
 * - sessionGroups/hasSessions ← sessionStore（SkinSidebar 时间分组迁移）
 * - estop 三态 ← WSAPI estop.status 10s 轮询（sidebar 槽在场才启停）
 * - signature ← WSAPI security.signature_verify_status（进程内不变，
 *   取到一次即止；三色口径照 Sidebar.vue sigBadge）
 * - fullAccess ← useEditorMode；theme/themeToggleLabel ← useTheme
 * - v3 chat 切片：messages（含同源 marked contentHtml）/chatBusy/inputText
 *   ← chatStore；mode ← useEditorMode
 * - v3 pages 切片：按路由注册的 WSAPI 装载器（独立于内置视图——page 槽
 *   替换视图后视图不挂载，数据必须自装载；首批 models/persona/skills）
 *
 * 动作白名单是**唯一交互出口**：route / new-chat / switch-session /
 * remove-session / fill-input / estop-toggle / toggle-theme / logout /
 * chat-send / chat-stop（+ 引擎内建 local:toggle）。confirm 在
 * remove-session 处理器内。输入 sink 是唯一写通道（chat.input）。
 */

import { effectScope, computed, reactive, watch, watchEffect } from 'vue'
import type { Router } from 'vue-router'
import { useAppStore } from '../stores/app'
import { useAuthStore } from '../stores/auth'
import { useChatStore } from '../stores/chat'
import { useSessionStore } from '../stores/session'
import type { SessionEntry } from '../composables/useChatApi'
import { httpGet } from '../composables/useWebSocket'
import { apiUrl } from '../lib/appBase'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import { useTheme } from '../composables/useTheme'
import { fullAccess } from '../composables/useEditorMode'
import { renderMarkdownHtml } from '../utils/markdown'
import { skinState, skinHasSlot, getSkinEngine } from '../composables/useSkin'
import { registerSkinActions, registerSkinFields } from './actions'
import { bridgeSend, bridgeStop } from './chatBridge'
import type { SkinNavItem, SkinProjection } from './types'

/** 投影单例（reactive；引擎只读消费）。 */
export const skinProjection = reactive<SkinProjection>({
  brand: '',
  version: '',
  scenes: [],
  connected: false,
  connectionText: '未连接',
  readyText: '离线',
  statusVersion: '',
  model: '',
  mobileOpen: false,
  navPrimary: [],
  navMore: [],
  sessionGroups: [],
  hasSessions: false,
  currentSessionId: '',
  estopEngaged: false,
  estopBusy: false,
  estopLabel: '急停',
  signature: { visible: false, state: 'unsigned', label: '', title: '' },
  fullAccess: false,
  theme: 'dark',
  themeToggleLabel: '',
  messages: [],
  chatBusy: false,
  inputText: '',
  mode: 'build',
  pages: {},
})

// ---- 导航静态表（SkinSidebar 迁移 + forge/license 补齐原生覆盖）----

/** 主导航四项（WB 视觉形态映射真实页面；16×16 viewBox 24 描边图标）。 */
const NAV_PRIMARY: Omit<SkinNavItem, 'active'>[] = [
  {
    label: '人格',
    path: '/persona',
    icon: 'M12 12c2.21 0 4-1.79 4-4s-1.79-4-4-4-4 1.79-4 4 1.79 4 4 4zm0 2c-2.67 0-8 1.34-8 4v2h16v-2c0-2.66-5.33-4-8-4z',
  },
  { label: '代码开发', path: '/coding', icon: 'M16 18l6-6-6-6M8 6l-6 6 6 6' },
  {
    label: 'Skills',
    path: '/skills',
    icon: 'M12 2l2.4 4.86 5.36.78-3.88 3.78.92 5.34L12 14.24l-4.8 2.52.92-5.34L4.24 7.64l5.36-.78L12 2z',
  },
  {
    label: '定时任务',
    path: '/tasks',
    icon: 'M9 11l3 3L22 4 M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11',
  },
]

/** 「更多」flyout：其余全部管理页（含 forge/license，功能可达性不缩水）。 */
const NAV_MORE: { id: string; label: string; path: string }[] = [
  { id: 'overview', label: '概览', path: '/overview' },
  { id: 'usage', label: '使用统计', path: '/usage' },
  { id: 'logs', label: '日志', path: '/logs' },
  { id: 'models', label: '模型', path: '/models' },
  { id: 'memory', label: '记忆', path: '/memory' },
  { id: 'persona-shop', label: '人格超市', path: '/persona-shop' },
  { id: 'mcp', label: 'MCP', path: '/mcp' },
  { id: 'hooks', label: 'HOOK', path: '/hooks' },
  { id: 'commands', label: '命令', path: '/commands' },
  { id: 'plugins', label: '插件', path: '/plugins' },
  { id: 'subagents', label: '子Agent', path: '/subagents' },
  { id: 'channels', label: '通道', path: '/channels' },
  { id: 'workflows', label: '工作流', path: '/workflows' },
  { id: 'forge', label: 'Forge', path: '/forge' },
  { id: 'board', label: '看板', path: '/board' },
  { id: 'cluster', label: '集群', path: '/cluster' },
  { id: 'security', label: '安全', path: '/security' },
  { id: 'scanner', label: '扫描器', path: '/scanner' },
  { id: 'sandbox', label: '沙盒', path: '/sandbox' },
  { id: 'terminal', label: '终端', path: '/terminal' },
  { id: 'local-models', label: '本地模型', path: '/local-models' },
  { id: 'tools', label: 'Tools', path: '/tools' },
  { id: 'sdk', label: '二次开发', path: '/sdk' },
  { id: 'proxy-settings', label: '代理设置', path: '/proxy-settings' },
  { id: 'settings', label: '设置', path: '/settings' },
  { id: 'about', label: '关于', path: '/about' },
  { id: 'license', label: 'License', path: '/license' },
]

/** feature 裁剪门控（Sidebar.vue itemFeature 同款：VITE_FEATURE_X=false 隐藏）。 */
const ITEM_FEATURE: Record<string, string> = {
  usage: 'USAGE',
  memory: 'MEMORY',
  workflows: 'WORKFLOW',
  forge: 'FORGE',
  cluster: 'CLUSTER',
  security: 'SECURITY',
  scanner: 'SECURITY',
  sandbox: 'SANDBOX',
  board: 'BOARD',
  terminal: 'TERMINAL',
}

// ---- 会话时间分组（SkinSidebar L104-138 迁移）----

function sessionTitle(s: SessionEntry): string {
  return s.title || s.firstMessage || s.id.slice(0, 8)
}

function relTime(s: SessionEntry): string {
  if (!s.lastTime) return ''
  const t = Date.parse(s.lastTime)
  if (Number.isNaN(t)) return ''
  const diff = Date.now() - t
  if (diff < 60000) return '刚刚'
  if (diff < 3600000) return `${Math.floor(diff / 60000)} 分钟前`
  if (diff < 86400000) return `${Math.floor(diff / 3600000)} 小时前`
  return `${Math.floor(diff / 86400000)} 天前`
}

function groupSessions(sessions: SessionEntry[]): { label: string; items: SessionEntry[] }[] {
  const now = new Date()
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()
  const dayMs = 86400000
  const groups = [
    { label: '今天', items: [] as SessionEntry[] },
    { label: '昨天', items: [] as SessionEntry[] },
    { label: '7 天内', items: [] as SessionEntry[] },
    { label: '更早', items: [] as SessionEntry[] },
  ]
  for (const s of sessions) {
    const t = s.lastTime ? Date.parse(s.lastTime) : NaN
    if (Number.isNaN(t)) groups[3].items.push(s)
    else if (t >= startOfToday) groups[0].items.push(s)
    else if (t >= startOfToday - dayMs) groups[1].items.push(s)
    else if (t >= startOfToday - 7 * dayMs) groups[2].items.push(s)
    else groups[3].items.push(s)
  }
  return groups.filter((g) => g.items.length > 0)
}

// ---- 状态栏取数（AppLayout fetchStatusbar 迁入）----

interface StatusbarStatus {
  version?: string
  model?: string
}

/** 活跃模型展示名（provider/name → name 段；OverviewView 同款口径）。 */
function shortModel(m?: string): string {
  if (!m) return ''
  const i = m.indexOf('/')
  return i >= 0 ? m.slice(i + 1) : m
}

function fetchStatusbar(): void {
  const stored = localStorage.getItem('nemesisbot_auth_token')
  const url = stored
    ? `${apiUrl('/api/status')}?token=${encodeURIComponent(stored)}`
    : apiUrl('/api/status')
  httpGet<StatusbarStatus>(url)
    .then((s) => {
      // 分隔符宿主拼好（structure.html 直接 bind；  对齐原版间距）
      skinProjection.statusVersion = s.version ? ` · v${s.version}` : ''
      const m = shortModel(s.model)
      skinProjection.model = m ? ` · ${m}` : ''
    })
    .catch(() => {
      // 拉取失败静默——状态栏少两段展示，不炸
    })
}

let wired = false

/**
 * 装配投影与动作白名单（AppLayout setup 调用；幂等单例）。
 * `router` 必须从 setup 上下文传入（useRouter 不能在模块层调用）。
 */
export function setupSkinProjection(router: Router): void {
  if (wired) return
  wired = true

  const appStore = useAppStore()
  const sessionStore = useSessionStore()
  const chatStore = useChatStore()
  const auth = useAuthStore()
  const { request } = useWSAPI()
  const toast = useToast()
  const { theme, toggleTheme } = useTheme()

  const scope = effectScope()
  scope.run(() => {
    // 引擎挂投影（唯一数据通道）+ 白名单（唯一交互出口）。
    const engine = getSkinEngine()
    engine.projection = skinProjection

    registerSkinActions(engine.actionHandlers, {
      route: (arg) => {
        if (!arg) return
        engine.local.moreOpen = false
        appStore.showMobileSidebar = false
        void router.push(arg)
      },
      'new-chat': () => {
        void sessionStore.create().then((sid) => {
          if (!sid) {
            toast.error('新建会话失败')
            return
          }
          void router.push('/')
        })
      },
      'switch-session': (arg) => {
        if (!arg) return
        engine.local.moreOpen = false
        sessionStore.switchTo(arg)
        appStore.showMobileSidebar = false
        void router.push('/')
      },
      'remove-session': (arg) => {
        const s = sessionStore.sessions.find((x) => x.id === arg)
        if (!s) return
        if (!window.confirm(`删除会话「${sessionTitle(s)}」？此操作不可恢复。`)) return
        void sessionStore.remove(arg)
      },
      'fill-input': (arg) => {
        chatStore.input = arg ? `${arg}：` : ''
        chatStore.focusInputNonce++
      },
      'chat-send': () => {
        bridgeSend()
      },
      'chat-stop': () => {
        bridgeStop()
      },
      'estop-toggle': () => {
        void toggleEstop()
      },
      'toggle-theme': () => {
        toggleTheme()
      },
      logout: () => {
        auth.logout()
      },
    })

    // 连接 / 移动抽屉 / 品牌（meta 到位自动跟随）。
    watchEffect(() => {
      skinProjection.brand = skinState.meta?.brand || skinState.id
      skinProjection.version = skinState.meta?.version || ''
      skinProjection.scenes = skinState.meta?.scenes || []
      skinProjection.connected = appStore.connected
      skinProjection.connectionText = appStore.connected ? '已连接' : '未连接'
      skinProjection.readyText = appStore.connected ? '就绪' : '离线'
      skinProjection.mobileOpen = appStore.showMobileSidebar
    })

    // 主导航 active 随路由。
    watchEffect(() => {
      const p = router.currentRoute.value.path
      skinProjection.navPrimary = NAV_PRIMARY.map((n) => ({ ...n, active: p === n.path }))
    })

    // flyout：feature 裁剪 + 未注册路由双过滤（点了跳空的的不列）。
    skinProjection.navMore = NAV_MORE.filter(
      (it) =>
        (ITEM_FEATURE[it.id]
          ? (import.meta.env['VITE_FEATURE_' + ITEM_FEATURE[it.id]] as string | undefined) !== 'false'
          : true) && router.resolve(it.path).matched.length > 0
    ).map((it) => ({ label: it.label, path: it.path }))

    // 会话历史（时间分组 + relTime + active 全宿主算好）。
    watchEffect(() => {
      const sessions = sessionStore.sessions
      const cur = sessionStore.currentId
      const p = router.currentRoute.value.path
      skinProjection.sessionGroups = groupSessions(sessions).map((g) => ({
        label: g.label,
        sessions: g.items.map((s) => ({
          id: s.id,
          title: sessionTitle(s),
          relTime: relTime(s),
          active: s.id === cur && p === '/',
          pinned: false,
        })),
      }))
      skinProjection.hasSessions = sessions.length > 0
      skinProjection.currentSessionId = cur || ''
    })

    // Full Access / 主题投影。
    watchEffect(() => {
      skinProjection.fullAccess = fullAccess.value
      skinProjection.theme = theme.value
      skinProjection.themeToggleLabel = theme.value === 'dark' ? '切换到亮色模式' : '切换到暗色模式'
    })

    // ---- v3 chat 切片：messages（同源 marked contentHtml）/ busy / 输入 /
    // 模式。contentHtml 只在 chat 槽在场时渲染成本才值得：无槽 = 置空并
    // 跳过全量映射；markdown 按内容字符串缓存（长会话追加消息不再对历史
    // 全量重渲，cache 超 500 条整体清——内容寻址，watchdog 重建也稳定）。
    const chatSlotActive = computed(() => skinHasSlot('chat'))
    const mdCache = new Map<string, string>()
    watchEffect(() => {
      skinProjection.chatBusy = chatStore.streaming
      skinProjection.inputText = chatStore.input
      skinProjection.mode = chatStore.agentMode
    })
    watchEffect(() => {
      if (!chatSlotActive.value) {
        skinProjection.messages = []
        return
      }
      if (mdCache.size > 500) mdCache.clear()
      const msgs = chatStore.messages
      skinProjection.messages = msgs.map((m, i) => {
        let contentHtml = ''
        if (m.role === 'assistant') {
          contentHtml = mdCache.get(m.content) ?? renderMarkdownHtml(m.content, { breaks: true })
          mdCache.set(m.content, contentHtml)
        }
        return {
          id: m.rowIndex ?? i,
          role: m.role,
          isUser: m.role === 'user',
          isError: m.role === 'error',
          content: m.content,
          contentHtml,
          time: m.timestamp,
          model: m.model ?? '',
          sourceNode: m.sourceNode ?? '',
          imageCount: m.imageCount ?? 0,
        }
      })
    })

    // 输入 sink（皮肤输入框 → chat.input 唯一写通道）。
    registerSkinFields(getSkinEngine().fieldSinks, {
      'chat.input': (v) => {
        chatStore.input = v
      },
    })

    // ---- v3 pages 切片：按路由注册的 WSAPI 装载器 ----
    // page:<path> 槽**替换**内置视图 → 视图不挂载、数据必须自装载（独立
    // 于视图生命周期）。路由激活且槽在场时拉一次；契约 = WSAPI 响应**原文**
    // （pages[path] 即响应对象，皮肤按 wsapi-commands 文档绑定字段，宿主
    // 零映射）。未注册路由 = 拉不到数据（皮肤自担空态布局）。
    const pageLoaders: Record<string, () => Promise<unknown>> = {
      '/models': () => request('models', 'list', {}, 8000),
      '/persona': () => request('persona', 'list', {}, 8000),
      '/skills': () => request('skills', 'installed', {}, 8000),
    }
    // 竞态守卫：切走再切回时，前一轮慢响应不得覆盖新一轮数据。
    const pageFetchSeq: Record<string, number> = {}
    watch(
      () => router.currentRoute.value.path,
      (p) => {
        const loader = pageLoaders[p]
        if (!loader || !skinHasSlot('page:' + p)) return
        const seq = (pageFetchSeq[p] = (pageFetchSeq[p] ?? 0) + 1)
        loader()
          .then((resp) => {
            // 拉取期间路由已切走 / 已有更新一轮 → 丢弃（防串台/防旧覆新）
            if (router.currentRoute.value.path !== p) return
            if (pageFetchSeq[p] !== seq) return
            skinProjection.pages[p] = (resp ?? {}) as Record<string, unknown>
          })
          .catch(() => {
            // WS 未就绪/后端不可用——保留旧数据（皮肤读到什么渲染什么）
          })
      },
      { immediate: true }
    )
  })

  // ---- E-Stop + 签名（Sidebar.vue 同款口径；sidebar 槽在场才轮询）----

  let estopTimer: ReturnType<typeof setInterval> | undefined

  async function refreshEstop(): Promise<void> {
    try {
      const resp = await request('estop', 'status', {}, 5000)
      skinProjection.estopEngaged = !!(resp && resp.engaged)
      skinProjection.estopLabel = skinProjection.estopEngaged ? '释放急停' : '急停'
    } catch {
      // WS 未就绪或后端不可用——保持原状态，下个轮询周期再试
    }
  }

  async function toggleEstop(): Promise<void> {
    if (skinProjection.estopBusy) return
    skinProjection.estopBusy = true
    try {
      const cmd = skinProjection.estopEngaged ? 'release' : 'trigger'
      const resp = await request('estop', cmd, {}, 5000)
      skinProjection.estopEngaged = !!(resp && resp.engaged)
    } catch (e) {
      console.error('[E-Stop] toggle failed:', e)
    } finally {
      skinProjection.estopBusy = false
    }
    skinProjection.estopLabel = skinProjection.estopEngaged ? '释放急停' : '急停'
  }
  // 签名徽标（进程内不变，取到一次即止；estop 轮询里捎带重试）。
  let sigLoaded = false
  const sigState = { mode: '', locked: false, result: null as string | null, keyFp: null as string | null, detail: '' }

  function projectSignature(): void {
    if (!sigLoaded) {
      skinProjection.signature = { visible: false, state: 'unsigned', label: '', title: '' }
      return
    }
    const { mode, locked, result, keyFp, detail } = sigState
    // 三色口径（Sidebar sigBadge）：Valid=绿 / Tampered=红 / 其余=黄。
    let state: string
    let label: string
    if (result === 'Valid') {
      state = 'verified'
      label = locked ? '🔒 签名已验证' : '签名已验证'
    } else if (result === 'Tampered') {
      state = 'invalid'
      label = '签名已被篡改'
    } else if (mode === 'off') {
      state = 'unverified'
      label = '签名验证 关'
    } else {
      state = 'unverified'
      label = '签名验证异常'
    }
    const parts = [`模式: ${mode}${locked ? '（锁定版，config 不可关）' : ''}`]
    if (result) parts.push(`启动自验: ${result}`)
    if (keyFp) parts.push(`签名者: ${keyFp}`)
    if (detail) parts.push(detail)
    skinProjection.signature = { visible: true, state, label, title: parts.join('\n') }
  }

  async function refreshSigVerify(): Promise<void> {
    if (sigLoaded) return
    try {
      const resp: any = await request('security', 'signature_verify_status', {}, 5000)
      if (!resp || !resp.injected) return // 测试/降级装配：不显示
      sigState.mode = resp.mode ?? ''
      sigState.locked = !!resp.locked
      sigState.result = resp.last_result ?? null
      sigState.keyFp = resp.key_fp ?? null
      sigState.detail = resp.detail ?? ''
      sigLoaded = true
    } catch {
      // WS 未就绪——保持未加载，下个轮询周期再试
    }
    projectSignature()
  }

  watch(
    () => skinHasSlot('sidebar'),
    (on) => {
      if (on && !estopTimer) {
        void refreshEstop()
        void refreshSigVerify()
        estopTimer = setInterval(() => {
          void refreshEstop()
          void refreshSigVerify() // 已加载即 no-op
        }, 10000)
      } else if (!on && estopTimer) {
        clearInterval(estopTimer)
        estopTimer = undefined
        projectSignature()
      }
    },
    { immediate: true }
  )

  // 状态栏取数：statusbar 槽在场拉一次（皮肤热切重进不再重复拉也无妨——
  // 同接口幂等；AppLayout 原行为同款）。
  watch(
    () => skinHasSlot('statusbar'),
    (on) => {
      if (on) fetchStatusbar()
    },
    { immediate: true }
  )

  // 会话列表入场拉取（store 内带 5s 缓存与静默容错；ChatView 也会拉，
  // 重复零开销）。
  watch(
    () => skinHasSlot('sidebar'),
    (on) => {
      if (on) {
        void sessionStore.fetchList()
        void sessionStore.fetchProjects()
      }
    },
    { immediate: true }
  )
}
