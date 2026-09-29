/**
 * `window.NemesisSkin` 皮肤脚本契约层（P2b，plan §6）。
 *
 * **目的**：皮肤脚本只对稳定契约编程，宿主内部重构自由不受限，契约破坏
 * 在宿主 CI 红（`__tests__/contract.spec.ts`，对齐 WSAPI `commands()` L1
 * 注册表先例），而不是三方脚本里静默烂。
 *
 * **版本化**：`NemesisSkin.version` 整数递增；破坏性变更走版本协商（皮肤
 * manifest `compat` 字段现成）。增量演进原则：只加不改，加字段不 bump。
 *
 * **安装时机**：`main.ts` 同步调用 `installNemesisSkin()`，且必须先于
 * `applySkinBoot()`——脚本注入走网络异步路径，同步安装保证任何皮肤脚本
 * 执行时契约必然在场（无竞态）。
 *
 * 只读投影（theme/session）：P3 需要时增补（plan：增量演进），本版不含。
 */

import { router } from '../router'
import { flatNavModel, type NavItemContract } from './navModel'
import { configSchema, schemaPageIds, type SchemaPage } from './configSchema'

/** 契约版本（破坏性变更才递增）。 */
export const NEMESIS_SKIN_VERSION = 1

export interface NemesisSkinNav {
  /** 结构化导航模型（feature 门控过滤后；deep copy，脚本改不动宿主态）。 */
  model(): NavItemContract[]
  /** 订阅模型变更：订阅即回调一次（当前模型）；返回退订函数。当前模型
   * 为编译期常量（VITE_FEATURE 门控），变更发布为增量演进预留。 */
  onChange(cb: (model: NavItemContract[]) => void): () => void
}

export interface NemesisSkinConfig {
  /** 配置页字段 schema；未知页返回 null。 */
  schema(pageId: string): SchemaPage | null
  /** 全部页 id（脚本可枚举渲染）。 */
  pages(): string[]
  /** 当前配置值（整棵 config JSON；表单初始化用，取不到的路径回落 schema.default）。 */
  current(): Promise<Record<string, unknown>>
  /** 写回单字段（等价 WSAPI `config.set_field`；安全 8 层/审计同源）。 */
  set(path: string, value: unknown): Promise<void>
}

export interface NemesisSkinApi {
  version: number
  /** 按稳定路由 id 导航；未知 id 返回 false（不抛错，脚本侧诚实降级）。 */
  navigate(routeId: string): boolean
  nav: NemesisSkinNav
  config: NemesisSkinConfig
}

declare global {
  interface Window {
    NemesisSkin?: NemesisSkinApi
  }
}

const listeners = new Set<(model: NavItemContract[]) => void>()

function api(): NemesisSkinApi {
  return {
    version: NEMESIS_SKIN_VERSION,
    navigate(routeId: string): boolean {
      const item = flatNavModel().find((i) => i.id === routeId)
      if (!item) return false
      void router.push(item.route)
      return true
    },
    nav: {
      model(): NavItemContract[] {
        return flatNavModel().map((i) => ({ ...i }))
      },
      onChange(cb: (model: NavItemContract[]) => void): () => void {
        listeners.add(cb)
        cb(flatNavModel().map((i) => ({ ...i })))
        return () => listeners.delete(cb)
      },
    },
    config: {
      schema(pageId: string): SchemaPage | null {
        const page = configSchema(pageId)
        if (!page) return null
        return { ...page, fields: page.fields.map((f) => ({ ...f })) }
      },
      pages(): string[] {
        return schemaPageIds()
      },
      async current(): Promise<Record<string, unknown>> {
        const { useWSAPI } = await import('../composables/useWSAPI')
        const cfg = (await useWSAPI().request('config', 'get')) as Record<string, unknown>
        return cfg ?? {}
      },
      async set(path: string, value: unknown): Promise<void> {
        const { useWSAPI } = await import('../composables/useWSAPI')
        await useWSAPI().request('config', 'set_field', { path, value })
      },
    },
  }
}

/** 安装契约（幂等；重复调用不覆盖——单页应用只装一次）。 */
export function installNemesisSkin(): void {
  if (window.NemesisSkin) return
  window.NemesisSkin = api()
}

/** 供宿主未来重发布导航模型（增量演进预留；当前无调用方）。 */
export function publishNavModelChange(): void {
  const model = flatNavModel().map((i) => ({ ...i }))
  listeners.forEach((cb) => cb(model))
}
