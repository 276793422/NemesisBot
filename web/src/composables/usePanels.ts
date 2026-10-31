/**
 * 面板注册表（2026-10-31 插件 UI 面板扩展 P2）。
 *
 * 数据源 = WSAPI `plugins.ui.registry`（声明面投影，后端真相源：
 * crates/nemesis-web/src/handlers/plugins_wasm.rs::ui_registry_impl——
 * 扫 plugins_wasm/ 载荷目录，kind=panel 且开关 enabled 的 plugin.toml
 * 直接读；与运行态解耦，gateway 重启窗口内锚点仍在）。
 * 动作/数据通道 = `plugins.ui.invoke`（estop 闸在代理层 plugins.rs；
 * guest `handle-ui-command` 转发；业务失败是 200 内 `{"error":...}`
 * 形态，由调用方判读——不是 WS 错误）。
 *
 * 设计边界（v1）：skins/navModel.ts 的静态契约（契约测试钉死
 * 「grouped ⊆ NAV_GROUPS」不变量）不动；【定制】动态分组在 Sidebar
 * 渲染层合并（panelNavItems 供数）；路由走静态参数形态
 * `/panel/:route`（深链免 addRoute 时窗；route 段唯一性由后端注册面
 * 保证，碰撞时 pageByRoute 先到先得）。
 */
import { ref } from 'vue'
import type { NavItemModel } from '../skins/navModel'
import { useWSAPI } from './useWSAPI'

export interface PanelColumn {
  key: string
  label: string
}

export interface PanelField {
  key: string
  label: string
  /** string | text | bool | int | secret（manifest.rs validate_view_decl 值域）。 */
  field_type: string
  placeholder?: string
  hint?: string
}

export interface PanelButton {
  label: string
  command: string
  confirm?: boolean
  danger?: boolean
}

/** 视图声明（v0 五词汇：form/table/timeline/detail/actions）。 */
export interface PanelViewDecl {
  kind: string
  /** `cmd://<command>`（guest）或 `flow://<template>/<view>`（P3 流程引擎）。 */
  bind: string
  columns: PanelColumn[]
  fields: PanelField[]
  buttons: PanelButton[]
}

export interface PanelPage {
  route: string
  title: string
  icon?: string
  views: PanelViewDecl[]
}

export interface PanelPlugin {
  slug: string
  name: string
  /** 载荷在场（manifest 读取成功）；false = 开关开着但载荷丢失。 */
  payload: boolean
  pages: PanelPage[]
}

/** 单视图的数据槽：取数状态 + guest 回包。 */
export interface PanelSlotState {
  loading: boolean
  error: string
  data: any
  /** cmd = guest 取数 / flow = 流程引擎（P3 前占位） / none = 纯动作视图。 */
  from: 'cmd' | 'flow' | 'none'
}

/** 面板页侧栏统一图标（单 path d，24×24 stroke 词汇，同 navModel 约定）。 */
export const PANEL_NAV_ICON =
  'M20.5 11H19V7a2 2 0 0 0-2-2h-4V3.5a2.5 2.5 0 0 0-5 0V5H4a2 2 0 0 0-2 2v3.8h1.5a2.7 2.7 0 0 1 0 5.4H2V20a2 2 0 0 0 2 2h3.8v-1.5a2.7 2.7 0 0 1 5.4 0V22H17a2 2 0 0 0 2-2v-4h1.5a2.5 2.5 0 0 0 0-5z'

/** 模块级单例清单：Sidebar / PanelView / 插件页共享同一份。 */
const panels = ref<PanelPlugin[]>([])
let inflight: Promise<void> | null = null

/** 侧栏【定制】组条目（NavItemModel 形态，渲染层合并进 visibleNavGroups）。 */
export function panelNavItems(): NavItemModel[] {
  return panels.value.flatMap((p) =>
    p.pages.map((pg) => ({
      id: `panel/${pg.route}`,
      label: pg.title,
      icon: PANEL_NAV_ICON,
      route: `/panel/${pg.route}`,
    })),
  )
}

export function usePanels() {
  /**
   * 拉取注册表（并发去重；失败保持现状等下一轮——轮询自愈语义，
   * 与 Sidebar estop 徽标同款）。WS 未初始化时 request 直接 reject，
   * 静默吞掉等轮询即可。
   */
  async function fetchPanels(): Promise<void> {
    if (inflight) return inflight
    inflight = (async () => {
      try {
        const { request } = useWSAPI()
        const resp: any = await request('plugins', 'ui.registry', {}, 8000)
        const next = Array.isArray(resp?.panels) ? resp.panels : []
        // 变化去重：Sidebar 10s 轮询常态下内容不变——整单替换会打爆
        // 依赖 panels 的 computed/watch（PanelView 每 10s 整页重取）。
        if (JSON.stringify(next) !== JSON.stringify(panels.value)) panels.value = next
      } catch {
        // 保持现有清单，下个轮询周期再试（诚实降级，不弹错）。
      } finally {
        inflight = null
      }
    })()
    return inflight
  }

  /** route 段 → (插件, 页面)。未注册（未装/未启用）回 null。 */
  function pageByRoute(routeSeg: string): { plugin: PanelPlugin; page: PanelPage } | null {
    for (const p of panels.value) {
      const page = p.pages.find((pg) => pg.route === routeSeg)
      if (page) return { plugin: p, page }
    }
    return null
  }

  /** guest 命令转发（业务失败是 200 内 `{"error":...}`，由调用方判读）。 */
  async function invokePanel(
    slug: string,
    command: string,
    args: Record<string, unknown> = {},
  ): Promise<any> {
    const { request } = useWSAPI()
    return request('plugins', 'ui.invoke', { slug, command, args }, 30000)
  }

  return { panels, fetchPanels, pageByRoute, invokePanel }
}
