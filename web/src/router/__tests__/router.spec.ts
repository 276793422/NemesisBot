// @vitest-environment jsdom
import { describe, it, expect } from 'vitest'

// M6 补测（quality-hardening goal 2026-08-25）：UI 入口批次给 router 加的
// 两条路由（P2-1 /coding、P2-2 /sdk）。防「组件存在但路由丢了」的入口
// 断裂——Sidebar 链接指向 router path，路由缺失则点击落到 404 空页。
// 组件本体行为由 CodingView/SdkView spec 各自钉住。

import { router } from '../index'

describe('router 批次新增路由', () => {
  it('/coding 存在，name=coding，懒加载组件', () => {
    const r = router.getRoutes().find(r => r.path === '/coding')
    expect(r).toBeTruthy()
    expect(r!.name).toBe('coding')
    expect(typeof r!.components!.default).toBe('function')
  })

  it('/sdk 存在，name=sdk，懒加载组件', () => {
    const r = router.getRoutes().find(r => r.path === '/sdk')
    expect(r).toBeTruthy()
    expect(r!.name).toBe('sdk')
    expect(typeof r!.components!.default).toBe('function')
  })

  it('懒加载组件工厂解析到真实模块（import 不抛）', async () => {
    for (const path of ['/coding', '/sdk']) {
      const r = router.getRoutes().find(r => r.path === path)!
      const mod = await (r.components!.default as () => Promise<unknown>)()
      expect(mod).toBeTruthy()
      // Vite SFC 模块导出 render/setup 之一（default 组件对象）
      const comp = (mod as any).default ?? mod
      expect(typeof comp === 'object' || typeof comp === 'function').toBe(true)
    }
  })

  it('path 唯一性：无重复注册', () => {
    const paths = router.getRoutes().map(r => r.path)
    expect(new Set(paths).size).toBe(paths.length)
  })
})
