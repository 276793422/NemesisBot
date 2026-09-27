/**
 * 皮肤动作白名单注册表：宿主 → 皮肤的唯一交互出口。
 *
 * `data-nb-action` 命名两形态：`name`（无参）/ `name:arg`（冒号后全部
 * 为 arg，arg 本身可含冒号——如 `route:/settings`）。未知动作由引擎
 * warn-once no-op，这里只管注册表构造与批量装配。
 */

import type { SkinActionHandler, SkinFieldSink } from './types'

/** 新建空注册表（引擎 `engine.actionHandlers` 直接挂这个 Map）。 */
export function createSkinActionRegistry(): Map<string, SkinActionHandler> {
  return new Map()
}

/** 批量装配（后注册覆盖先注册；F3 装配完整白名单）。 */
export function registerSkinActions(
  registry: Map<string, SkinActionHandler>,
  handlers: Record<string, SkinActionHandler>
): void {
  for (const [name, h] of Object.entries(handlers)) {
    registry.set(name, h)
  }
}

/** 输入 sink 批量装配（v3 data-nb-field；与动作白名单同构的写通道——
 * 皮肤只能写这里注册过的 sink，宿主全权控制写入口语义）。 */
export function registerSkinFields(
  registry: Map<string, SkinFieldSink>,
  sinks: Record<string, SkinFieldSink>
): void {
  for (const [name, s] of Object.entries(sinks)) {
    registry.set(name, s)
  }
}
