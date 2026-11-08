// 统一工作台 kind 注册表（人工触点体系 P2，2026-11-07）：与后端
// workbench.get/respond 的 kind dispatch 同构——route param 形态
// `touchpoint-{kind}` 与后端 kind_of strip_prefix 对齐；default_action
// 镜像 crates/nemesis-board/src/touchpoint.rs default_action_for
// （spec→proceed_with_draft / prototype→proceed_current_version）。
// 新 kind = 数组加一行（组件 + 标签），视图骨架零改动。
import { markRaw, type Component } from 'vue'
import SpecWorkbench from './SpecWorkbench.vue'
import PrototypeWorkbench from './PrototypeWorkbench.vue'

export interface WorkbenchKindDef {
  /** 后端 kind serde 值（workbench.get/respond 的 kind 参数）。 */
  kind: string
  /** route param 形态（/workbench/:kind/:id）。 */
  fullKind: string
  label: string
  icon: string
  component: Component
  /** 超时默认动作文案（镜像 default_action_for）。 */
  defaultAction: string
  defaultActionLabel: string
}

export const WORKBENCH_KINDS: WorkbenchKindDef[] = [
  {
    kind: 'spec',
    fullKind: 'touchpoint-spec',
    label: '规格确认',
    icon: '🎯',
    component: markRaw(SpecWorkbench),
    defaultAction: 'proceed_with_draft',
    defaultActionLabel: '按草案继续',
  },
  {
    kind: 'prototype',
    fullKind: 'touchpoint-prototype',
    label: '原型评审',
    icon: '🧪',
    component: markRaw(PrototypeWorkbench),
    defaultAction: 'proceed_current_version',
    defaultActionLabel: '按当前版继续',
  },
]

/** route param → kind 定义（strip 'touchpoint-' 前缀镜像后端 kind_of）。 */
export function resolveKind(routeKind: string): WorkbenchKindDef | null {
  const k = routeKind.startsWith('touchpoint-') ? routeKind.slice('touchpoint-'.length) : routeKind
  return WORKBENCH_KINDS.find((d) => d.kind === k) || null
}

/** 触点载荷解析（payload 是 JSON 字符串；坏载荷诚实返回空对象不炸渲染）。 */
export function parsePayload(payload: string | null | undefined): any {
  try {
    return JSON.parse(payload || '{}') || {}
  } catch {
    return {}
  }
}
