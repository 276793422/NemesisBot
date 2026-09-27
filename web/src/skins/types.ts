/**
 * 皮肤结构引擎（v2）——共享类型。
 *
 * 皮肤包自带 UI 结构（structure.html，声明式 `data-nb-*` 原语标注），
 * 宿主引擎负责清洗、挂载与数据供给。包内**绝不执行任意代码**：
 * 数据经投影单向流入（引擎只读），交互经动作白名单唯一通道流出。
 */

/** 引擎支持的 structure 协议版本（`data-nb-engine` 协商；高于此值整包拒载）。 */
export const SKIN_ENGINE_VERSION = 1

/** 槽位名集合（宿主挂载点：SkinSlot name 属性）。 */
export type SkinSlotName = 'sidebar' | 'titlebar' | 'statusbar' | 'launcher'

/** 导航项（投影宿主算好 active；引擎只渲染）。 */
export interface SkinNavItem {
  label: string
  path: string
  /** 可选 SVG path d（16×16 viewBox 24 描边风格；缺省不渲染图标） */
  icon?: string
  active?: boolean
}

/** 会话行（时间分组/relTime/active 全部宿主算好）。 */
export interface SkinSessionItem {
  id: string
  title: string
  relTime: string
  active: boolean
  pinned: boolean
}

/** 会话时间分组（今天/昨天/7天内/更早，空组宿主已滤除）。 */
export interface SkinSessionGroup {
  label: string
  sessions: SkinSessionItem[]
}

/** 签名徽标三态投影（security.signature_verify_status 口径）。 */
export interface SkinSignatureBadge {
  visible: boolean
  /** 'verified' | 'unsigned' | 'invalid' | 'unverified' */
  state: string
  label: string
  title: string
}

/** 聊天消息投影（v3 chat 槽）。contentHtml 与内置 ChatPanel 同源
 * （renderMarkdownHtml / marked），信任等级与内置 v-html 路径一致——
 * 只配 `data-nb-html` 消费，包作者 HTML 仍走 sanitize。 */
export interface SkinChatMessage {
  /** 会话内稳定序号（渲染 key 语义；皮肤可作 data-nb-action 参数回传）。 */
  id: number
  role: 'user' | 'assistant' | 'error' | 'system'
  /** 角色派生布尔（引擎无比较表达式，类名切换全靠它们——宿主算好）。 */
  isUser: boolean
  isError: boolean
  /** 原始文本（data-nb-bind 用）。 */
  content: string
  /** marked 渲染 HTML（assistant 消息；其他角色为空串）。 */
  contentHtml: string
  time: string
  /** 供应商·模型名徽标（assistant 专有，缺省空）。 */
  model: string
  /** 集群 worker 节点名（缺省空）。 */
  sourceNode: string
  imageCount: number
}

/** 输入字段 sink（v3 data-nb-field）：皮肤输入框 → 宿主状态的唯一写通道，
 * 与动作白名单同构——写入口由宿主注册，皮肤只能写已注册的 sink。 */
export type SkinFieldSink = (value: string) => void

/**
 * 数据投影：宿主 → 皮肤的唯一数据通道（引擎只读）。
 * 字段语义见 docs/REPORT/2026-09-27_skin-structure-engine-v2.md。
 */
export interface SkinProjection {
  brand: string
  version: string
  /** 主页启动器场景标签（fill-input 动作预填） */
  scenes: string[]
  connected: boolean
  connectionText: string
  readyText: string
  statusVersion: string
  /** 活跃模型展示名（provider/name → name 段） */
  model: string
  /** 移动端抽屉开合（hamburger → showMobileSidebar） */
  mobileOpen: boolean
  navPrimary: SkinNavItem[]
  navMore: SkinNavItem[]
  sessionGroups: SkinSessionGroup[]
  hasSessions: boolean
  currentSessionId: string
  /** E-Stop 三态（estopEngaged 与 estopBusy 同时真 = 执行中） */
  estopEngaged: boolean
  estopBusy: boolean
  estopLabel: string
  signature: SkinSignatureBadge
  /** Full Access 放行态（useEditorMode） */
  fullAccess: boolean
  /** 当前主题（'dark' | 'light'） */
  theme: string
  themeToggleLabel: string
  // ---- v3 内容区切片 ----
  /** 聊天消息投影（当前会话；chat 槽用）。 */
  messages: SkinChatMessage[]
  /** 当前会话 streaming 态（发送中/生成中）。 */
  chatBusy: boolean
  /** 输入框内容（chat.input 投影；data-nb-value 预填用）。 */
  inputText: string
  /** 编辑器模式（'plan' | 'build'）。 */
  mode: string
  /** 管理页数据切片（page:<path> 槽用；视图经 useSkinPageData 注册，
   * 未注册路由为空对象——皮肤自担布局）。 */
  pages: Record<string, Record<string, unknown>>
}

/** 动作处理器：arg 为 `name:` 后的参数（无冒号动作恒空串）。 */
export type SkinActionHandler = (arg: string) => void
