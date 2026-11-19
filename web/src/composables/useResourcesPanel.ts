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
