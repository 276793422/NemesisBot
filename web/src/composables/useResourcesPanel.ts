import type { ComputedRef, InjectionKey } from 'vue'

/**
 * 资源窗开合控制（2026-11-19 提案 C 落地）：ChatView 持有状态机（滞后带/
 * 自动开/意志优先），经 provide 把「开合只读视图 + toggle」暴露给 ChatPanel
 * ——输入框右侧原【收起工具栏】按钮改为【打开/收起资源窗】（用户原始提案
 * 原文：「输入框右边的【收起工具栏】按钮，就可以调整成【收起资源窗】或者
 * 【打开资源窗】」）。inject 缺省 null（无 ChatView 的挂载面 = 按钮不渲染，
 * 无死按钮）。
 */
export interface ResourcesPanelCtl {
  open: ComputedRef<boolean>
  toggle: () => void
}

export const RESOURCES_PANEL_CTL: InjectionKey<ResourcesPanelCtl> = Symbol('resources-panel-ctl')

// ---- 判宽阈值与纯函数（2026-11-25 从 ChatView 抽出：单一真相源 + 可单测）----

/** 资源窗并排形态下限 = ChatPanel 上限 944（含滚动条余量）+ 面板最小 340。 */
export const RESOURCES_OPEN_AT = 1284
/** 滞后带下缘（低于 OPEN_AT 一截——拖动穿越阈值时不在临界点反复翻转）。 */
export const RESOURCES_CLOSE_AT = 1180

/**
 * 资源窗宽/窄判定（滞后带纯函数）。
 *
 * 非首屏：≥OPEN_AT 判宽、<CLOSE_AT 判窄、带间保持原值（拖动防抖）。
 * 首屏（isFirst=true）：无既有状态可「保持」，带间宽度按并排下限硬判——
 * 否则宽判定初值 true 会把 1180~1283 的带间宽度误判成宽屏走并排，而行内
 * 固定宽列（ChatPanel 944 + 资源窗 min 300 + 会话侧栏 260）超出可用宽时，
 * 唯一没有缩窄保护的浏览器点名面板列（min-width:0）被 flex 挤成 0 宽不可
 * 见（2026-11-25 实录：1440 视口可用 1200，景 6 toBeVisible 失败）。
 */
export function resolveResourcesWide(w: number, prev: boolean, isFirst: boolean): boolean {
  if (isFirst) return w >= RESOURCES_OPEN_AT
  return w >= RESOURCES_OPEN_AT ? true : w < RESOURCES_CLOSE_AT ? false : prev
}
