import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// BUG-20261102-320 回归钉：桥接子路径形态（<base href="/d/<node_id>/">）
// 下 useSkin 的全部 HTTP fetch 必须经 appBase() 加前缀。
//
// 修复前：useSkin 全部裸根相对 fetch（/skins/active.css 等）——<base>
// 不影响 JS 运行时构造的 URL（appBase.ts 头注释的明确契约），请求打到
// 中继根：中继自身是完整 web server（有自己的 /skins/* 路由）→ 桥接访
// 问远端节点时加载的是**中继节点的皮肤包**（CSS/结构/脚本三层串台），
// 而非被访问节点的皮肤。
// 修复后：入口三处（applySkinBoot / applySkinRefresh / resolveScriptConsent
// 的再验）经 apiUrl() 包装；派生 URL（structure/script 从 cssUrl.replace
// 派生）随之带前缀。直连形态 appBase()=空串 → URL 逐字节不变（既有
// useSkin.*.spec 的根相对路由键零回归）。

const CSS = 'html[data-skin="bridged"] { --accent: #bead00; }'

const wsRequestMock = vi.fn()
vi.mock('../useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => wsRequestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import { skinState, skinScriptNeedsReload, applySkinBoot, applySkinRefresh, resolveScriptConsent, clearSkinStructure } from '../useSkin'

const BASE = '/d/node-a'

function cssResponse(id: string, body = CSS): Response {
  return new Response(body, {
    status: 200,
    headers: { 'Content-Type': 'text/css', 'X-Skin-Id': id },
  })
}

function scriptResponse(js: string, sha: string): Response {
  return new Response(js, {
    status: 200,
    headers: {
      'Content-Type': 'text/javascript; charset=utf-8',
      'X-Skin-Id': 'bridged',
      'X-Skin-Sha256': sha,
    },
  })
}

/** script 链为 fire-and-forget——排空微任务再断言。 */
async function settle(): Promise<void> {
  for (let i = 0; i < 6; i++) await Promise.resolve()
  await new Promise((r) => setTimeout(r, 0))
}

function scriptEl(): HTMLScriptElement | null {
  return document.head.querySelector('script[data-nb-skin-script]')
}

/** 全量复位（同 useSkin.script.spec 的出口形态）。 */
async function resetAll(): Promise<void> {
  history.replaceState(null, '', '/?skin=default')
  vi.unstubAllGlobals()
  await applySkinBoot()
  history.replaceState(null, '', '/')
  clearSkinStructure()
  skinState.id = ''
  skinState.meta = null
  skinState.scriptPending = ''
  localStorage.clear()
  document.head
    .querySelectorAll('style[data-skin-sheet], script[data-nb-skin-script]')
    .forEach((n) => n.remove())
  document.documentElement.removeAttribute('data-skin')
  document.head.querySelector('base')?.remove()
  wsRequestMock.mockReset()
}

beforeEach(async () => {
  await resetAll()
})
afterEach(async () => {
  await resetAll()
})

describe('BUG-20261102-320：桥接子路径形态皮肤请求带 base 前缀', () => {
  it('applySkinBoot（active 链）：css/structure 请求全部带 /d/<node> 前缀且皮肤真装上', async () => {
    document.head.insertAdjacentHTML('beforeend', `<base href="${BASE}/">`)
    const f = vi.fn().mockImplementation((url: string) => {
      if (url === `${BASE}/skins/active.css`) return Promise.resolve(cssResponse('bridged'))
      return Promise.resolve(new Response('', { status: 404 }))
    })
    vi.stubGlobal('fetch', f)
    try {
      await applySkinBoot()
      await settle()
      const urls = f.mock.calls.map((c: any[]) => String(c[0]))
      expect(urls.length).toBeGreaterThanOrEqual(2) // css + structure
      expect(urls.every((u: string) => u.startsWith(BASE))).toBe(true)
      expect(urls).toContain(`${BASE}/skins/active.css`)
      expect(urls).toContain(`${BASE}/skins/active/structure`)
      // 皮肤真装上（而非 404 回落内置）
      expect(skinState.id).toBe('bridged')
      expect(document.documentElement.getAttribute('data-skin')).toBe('bridged')
    } finally {
      document.head.querySelector('base')?.remove()
    }
  })

  it('script 链与同意卡再验请求同样带前缀', async () => {
    document.head.insertAdjacentHTML('beforeend', `<base href="${BASE}/">`)
    const JS = 'window.__nb_probe__ = 1'
    const SHA = 'ef'.repeat(32)
    const f = vi.fn().mockImplementation((url: string) => {
      if (url === `${BASE}/skins/active.css`) return Promise.resolve(cssResponse('bridged'))
      if (url === `${BASE}/skins/active/script`) return Promise.resolve(scriptResponse(JS, SHA))
      return Promise.resolve(new Response('', { status: 404 }))
    })
    vi.stubGlobal('fetch', f)
    try {
      await applySkinBoot()
      await settle()
      // 未同意 → 挂 pending（不注入）
      expect(skinState.scriptPending).toBe('bridged')
      expect(scriptEl()).toBeNull()
      // 同意卡 allow → 再验 fetch（显式 id 路径）也必须带前缀命中
      vi.stubGlobal('fetch', vi.fn().mockImplementation((url: string) => {
        if (url === `${BASE}/skins/bridged/script`) return Promise.resolve(scriptResponse(JS, SHA))
        return Promise.resolve(new Response('', { status: 404 }))
      }))
      await resolveScriptConsent(true)
      expect(scriptEl()).not.toBeNull()
      expect(scriptEl()!.textContent).toBe(JS)
      expect(skinScriptNeedsReload()).toBe(true)
    } finally {
      document.head.querySelector('base')?.remove()
    }
  })

  it('applySkinRefresh（热切）同样带前缀；预览模式（?skin=）的 CSS 请求也带前缀', async () => {
    document.head.insertAdjacentHTML('beforeend', `<base href="${BASE}/">`)
    const f = vi.fn().mockImplementation((url: string) => {
      if (url === `${BASE}/skins/active.css`) return Promise.resolve(cssResponse('bridged'))
      return Promise.resolve(new Response('', { status: 404 }))
    })
    vi.stubGlobal('fetch', f)
    try {
      await applySkinBoot()
      await settle()
      f.mockClear()
      await applySkinRefresh()
      await settle()
      let urls = f.mock.calls.map((c: any[]) => String(c[0]))
      expect(urls.length).toBeGreaterThanOrEqual(1)
      expect(urls.every((u: string) => u.startsWith(BASE))).toBe(true)
      expect(urls).toContain(`${BASE}/skins/active.css`)

      // 预览路径
      history.replaceState(null, '', '/?skin=bridged')
      f.mockClear()
      await applySkinBoot()
      await settle()
      urls = f.mock.calls.map((c: any[]) => String(c[0]))
      expect(urls.every((u: string) => u.startsWith(BASE))).toBe(true)
      expect(urls).toContain(`${BASE}/skins/bridged`)
      history.replaceState(null, '', '/')
    } finally {
      document.head.querySelector('base')?.remove()
    }
  })

  it('直连形态（无 <base>）URL 不变：根相对路由键照常命中', async () => {
    const f = vi.fn().mockImplementation((url: string) => {
      if (url === '/skins/active.css') return Promise.resolve(cssResponse('plain'))
      return Promise.resolve(new Response('', { status: 404 }))
    })
    vi.stubGlobal('fetch', f)
    await applySkinBoot()
    await settle()
    const urls = f.mock.calls.map((c: any[]) => String(c[0]))
    expect(urls).toContain('/skins/active.css')
    expect(skinState.id).toBe('plain')
  })
})
