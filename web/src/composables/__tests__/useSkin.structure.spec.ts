import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// useSkin 装载链（v2）：CSS ∥ structure 并行拉取、T0 冷启动缓存消费、
// T3 校准（内容一致不重建）、404/版本拒载回落原生布局、?skin=default 清缓存。
// fetch 全 mock；localStorage 用 vitest.setup.ts 的 MemoryStorage shim。

const CSS = 'html[data-skin="testskin"] { --accent: #123456; }'

const STRUCTURE_OK =
  `<template data-nb-slot="titlebar" data-nb-engine="1">` +
  `<header class="nb-titlebar"><span data-nb-bind="brand"></span></header></template>` +
  `<template data-nb-slot="statusbar" data-nb-engine="1">` +
  `<footer data-nb-bind="readyText"></footer></template>`

const STRUCTURE_V99 = STRUCTURE_OK.replace('data-nb-engine="1"', 'data-nb-engine="99"')

function cssResponse(id: string, body = CSS): Response {
  return new Response(body, {
    status: 200,
    headers: { 'Content-Type': 'text/css', 'X-Skin-Id': id },
  })
}

function fetchMock(routes: Record<string, Response | { status: number }>) {
  return vi.fn().mockImplementation((url: string) => {
    const hit = routes[url]
    if (!hit) return Promise.resolve(new Response('', { status: 404 }))
    if ('status' in hit && !(hit instanceof Response)) {
      return Promise.resolve(new Response('', { status: hit.status }))
    }
    return Promise.resolve(hit as Response)
  })
}

import {
  skinState,
  skinHasSlot,
  getSkinEngine,
  consumeStructureCache,
  clearSkinStructure,
  applySkinBoot,
  applySkinRefresh,
} from '../useSkin'
import { parseSkinStructure } from '../../skins/sanitize'

/** structure 链为 fire-and-forget（不阻塞 CSS 主链）——排空微任务再断言。 */
async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve()
  await new Promise((r) => setTimeout(r, 0))
}

function resetAll(): void {
  clearSkinStructure()
  skinState.id = ''
  skinState.meta = null
  skinState.structRev = 0
  localStorage.clear()
  document.head
    .querySelectorAll('style[data-skin-sheet], template[data-nb-structure-cache]')
    .forEach((n) => n.remove())
  document.documentElement.removeAttribute('data-skin')
  vi.unstubAllGlobals()
}

beforeEach(resetAll)
afterEach(resetAll)

describe('useSkin 结构装载链', () => {
  it('CSS + structure 双 200 → data-skin + 槽位就绪 + 双缓存写入', async () => {
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('testskin'),
      '/skins/active/structure': new Response(STRUCTURE_OK, { status: 200 }),
    }))
    await applySkinBoot()
    await settle()
    expect(skinState.id).toBe('testskin')
    expect(skinHasSlot('titlebar')).toBe(true)
    expect(skinHasSlot('statusbar')).toBe(true)
    expect(skinState.structRev).toBeGreaterThan(0)
    expect(localStorage.getItem('nemesisbot_skin_css')).toBe(CSS)
    expect(localStorage.getItem('nemesisbot_skin_structure')).toContain('data-nb-slot="titlebar"')
    expect(document.documentElement.getAttribute('data-skin')).toBe('testskin')
  })

  it('structure 404（纯 CSS 包）→ CSS 照常生效，槽位空 = 原生布局', async () => {
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('csstheme'),
      // /skins/active/structure 缺路由 → 404
    }))
    await applySkinBoot()
    await settle()
    expect(skinState.id).toBe('csstheme')
    expect(document.documentElement.getAttribute('data-skin')).toBe('csstheme')
    expect(skinState.slots).toEqual([])
    expect(localStorage.getItem('nemesisbot_skin_structure')).toBeNull()
  })

  it('structure 协议版本高于引擎 → 拒载回落原生（CSS 照常）', async () => {
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('future'),
      '/skins/active/structure': new Response(STRUCTURE_V99, { status: 200 }),
    }))
    await applySkinBoot()
    await settle()
    expect(skinState.id).toBe('future')
    expect(skinState.slots).toEqual([])
    expect(localStorage.getItem('nemesisbot_skin_structure')).toBeNull()
  })

  it('热切到无皮肤（404）→ data-skin 摘除 + 全缓存清 + 引擎卸载', async () => {
    // 先装上
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('testskin'),
      '/skins/active/structure': new Response(STRUCTURE_OK, { status: 200 }),
    }))
    await applySkinBoot()
    await settle()
    expect(skinHasSlot('titlebar')).toBe(true)
    // 热切：active 没了（皮肤已关）
    vi.stubGlobal('fetch', fetchMock({}))
    await applySkinRefresh()
    await settle()
    expect(skinState.id).toBe('')
    expect(document.documentElement.hasAttribute('data-skin')).toBe(false)
    expect(skinState.slots).toEqual([])
    expect(localStorage.getItem('nemesisbot_skin')).toBeNull()
    expect(localStorage.getItem('nemesisbot_skin_structure')).toBeNull()
    expect(document.head.querySelector('style[data-skin-sheet]')).toBeNull()
  })

  it('?skin=default → 强制回落，缓存全清，不发请求', async () => {
    localStorage.setItem('nemesisbot_skin', 'testskin')
    localStorage.setItem('nemesisbot_skin_css', CSS)
    localStorage.setItem('nemesisbot_skin_structure', STRUCTURE_OK)
    document.documentElement.setAttribute('data-skin', 'testskin')
    history.replaceState(null, '', '/?skin=default')
    const f = vi.fn()
    vi.stubGlobal('fetch', f)
    await applySkinBoot()
    history.replaceState(null, '', '/')
    expect(f).not.toHaveBeenCalled()
    expect(skinState.id).toBe('')
    expect(localStorage.getItem('nemesisbot_skin_structure')).toBeNull()
    expect(document.documentElement.hasAttribute('data-skin')).toBe(false)
  })

  it('T0 缓存消费：inert template → 同步槽位就绪；幂等只消费一次', () => {
    const t = document.createElement('template')
    t.setAttribute('data-nb-structure-cache', '')
    t.innerHTML = STRUCTURE_OK
    document.head.appendChild(t)

    consumeStructureCache()
    expect(skinHasSlot('titlebar')).toBe(true)
    expect(skinHasSlot('statusbar')).toBe(true)
    expect(document.head.querySelector('template[data-nb-structure-cache]')).toBeNull()

    // 二次消费 no-op（摘掉槽再验证不被复活）
    getSkinEngine().load(new Map())
    skinState.slots = []
    consumeStructureCache()
    expect(skinHasSlot('titlebar')).toBe(false)
  })

  it('T3 校准：T0 命中后拉到同内容 → 不重建（structRev 不变）', async () => {
    // T0：head 缓存先行
    const t = document.createElement('template')
    t.setAttribute('data-nb-structure-cache', '')
    t.innerHTML = STRUCTURE_OK
    document.head.appendChild(t)
    // T2：SkinSlot 挂载路径消费（同内容已在引擎）
    consumeStructureCache()
    const rev = skinState.structRev
    expect(rev).toBeGreaterThan(0)
    // T3：异步校准拉到同内容
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('testskin'),
      '/skins/active/structure': new Response(STRUCTURE_OK, { status: 200 }),
    }))
    await applySkinBoot()
    await settle()
    expect(skinState.structRev).toBe(rev) // 内容一致 → 无原子重建
    expect(skinHasSlot('titlebar')).toBe(true)
    // 缓存刷新为服务端内容（一致）
    expect(localStorage.getItem('nemesisbot_skin_structure')).toContain('data-nb-slot="titlebar"')
  })

  it('T0 缓存拒载（坏内容）→ 静默回落，皮肤 CSS 不受影响', () => {
    const t = document.createElement('template')
    t.setAttribute('data-nb-structure-cache', '')
    t.innerHTML = `<template data-nb-slot="x" data-nb-engine="99"><b>boom</b></template>`
    document.head.appendChild(t)
    consumeStructureCache()
    expect(skinState.slots).toEqual([])
    expect(document.head.querySelector('template[data-nb-structure-cache]')).toBeNull()
  })

  it('parseSkinStructure 与装载链共用：空槽结构拒载', () => {
    expect(parseSkinStructure('<template data-nb-slot="x" data-nb-engine="1"></template>')).not.toBeNull()
    expect(parseSkinStructure('<div>no slots</div>')).toBeNull()
  })
})
