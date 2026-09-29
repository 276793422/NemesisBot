import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

// useSkin 脚本载荷链（P2a）：端点裁决（404/403/200）→ 同意缓存（按整包
// sha256 前 16 位盖章）→ 注入/挂 pending → 同意卡裁决 → 刷新边界（裁定 2：
// live 注入后任何出口都要求整页刷新）。fetch 全 mock；WSAPI 审计 mock；
// localStorage 用 vitest.setup.ts 的 MemoryStorage shim。
// jsdom 不执行动态内联脚本（runScripts 关）——注入断言钉在元素在场 +
// 属性 + 内容上；真实执行由 CDP E2E 兜底。

const CSS = 'html[data-skin="scripty"] { --accent: #cafe00; }'
const JS = 'window.__nb_skin_probe__ = "ran"'
const SHA = 'ab'.repeat(32)
const SHA2 = 'cd'.repeat(32)

const wsRequestMock = vi.fn()
vi.mock('../useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => wsRequestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import {
  skinState,
  skinScriptNeedsReload,
  resolveScriptConsent,
  discardScriptPending,
  clearSkinStructure,
  applySkinBoot,
  applySkinRefresh,
} from '../useSkin'

function cssResponse(id: string, body = CSS): Response {
  return new Response(body, {
    status: 200,
    headers: { 'Content-Type': 'text/css', 'X-Skin-Id': id },
  })
}

function scriptResponse(js = JS, sha = SHA): Response {
  return new Response(js, {
    status: 200,
    headers: {
      'Content-Type': 'text/javascript; charset=utf-8',
      'X-Skin-Id': 'scripty',
      'X-Skin-Sha256': sha,
    },
  })
}

function fetchMock(routes: Record<string, Response>): ReturnType<typeof vi.fn> {
  return vi.fn().mockImplementation((url: string) => {
    const hit = routes[url]
    if (!hit) return Promise.resolve(new Response('', { status: 404 }))
    return Promise.resolve(hit)
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

/** 全量复位。scriptLiveId 是模块私有态（注入后残留会污染后续用例——
 * allow 判据 `!scriptLiveId` / needsReload 判据都读它），无公开 setter，
 * 唯一合法清场路径 = 裁定 2 的出口（`?skin=default`），复位走同一出口。 */
async function resetAll(): Promise<void> {
  history.replaceState(null, '', '/?skin=default')
  vi.unstubAllGlobals() // default 分支不发请求，先摘 fetch 桩防串扰
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
  wsRequestMock.mockReset()
}

beforeEach(async () => {
  await resetAll()
})
afterEach(async () => {
  await resetAll()
})

describe('useSkin 脚本载荷链', () => {
  it('闸关（403）→ 静默纯 CSS：不注入、不挂 pending', async () => {
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('scripty'),
      '/skins/active/script': new Response('', { status: 403 }),
    }))
    await applySkinBoot()
    await settle()
    expect(skinState.id).toBe('scripty')
    expect(document.documentElement.getAttribute('data-skin')).toBe('scripty')
    expect(scriptEl()).toBeNull()
    expect(skinState.scriptPending).toBe('')
  })

  it('无脚本（404）→ 纯 CSS 照常，无任何脚本现场', async () => {
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('scripty'),
      // /skins/active/script 缺路由 → 404
    }))
    await applySkinBoot()
    await settle()
    expect(skinState.id).toBe('scripty')
    expect(scriptEl()).toBeNull()
    expect(skinState.scriptPending).toBe('')
  })

  it('有脚本无同意 → CSS 先生效，脚本挂 pending 等同意卡', async () => {
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('scripty'),
      '/skins/active/script': scriptResponse(),
    }))
    await applySkinBoot()
    await settle()
    expect(skinState.id).toBe('scripty')
    expect(scriptEl()).toBeNull() // 未同意绝不执行
    expect(skinState.scriptPending).toBe('scripty')
    expect(skinScriptNeedsReload()).toBe(false) // pending 不是 live
  })

  it('已同意且包未变（sha 前 16 位盖章命中）→ 免卡直接注入', async () => {
    localStorage.setItem('nemesisbot_skin_script_ok', JSON.stringify({ scripty: SHA.slice(0, 16) }))
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('scripty'),
      '/skins/active/script': scriptResponse(),
    }))
    await applySkinBoot()
    await settle()
    const el = scriptEl()
    expect(el).not.toBeNull()
    expect(el!.getAttribute('data-nb-skin-script')).toBe('scripty')
    expect(el!.textContent).toBe(JS)
    expect(skinState.scriptPending).toBe('') // 不出卡
    expect(skinScriptNeedsReload()).toBe(true) // live 了 → 出口要刷新
  })

  it('包更新（sha 变）→ 旧同意作废，重新挂 pending', async () => {
    localStorage.setItem('nemesisbot_skin_script_ok', JSON.stringify({ scripty: SHA.slice(0, 16) }))
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('scripty'),
      '/skins/active/script': scriptResponse(JS, SHA2), // 同 id 新包
    }))
    await applySkinBoot()
    await settle()
    expect(scriptEl()).toBeNull()
    expect(skinState.scriptPending).toBe('scripty')
  })

  it('同意卡 allow → 注入 + 写同意缓存 + 审计 allow', async () => {
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('scripty'),
      '/skins/active/script': scriptResponse(),
      '/skins/scripty/script': scriptResponse(), // allow 注入前再验闸（显式 id 路径）
    }))
    await applySkinBoot()
    await settle()
    expect(skinState.scriptPending).toBe('scripty')

    await resolveScriptConsent(true)
    expect(scriptEl()).not.toBeNull()
    expect(scriptEl()!.textContent).toBe(JS)
    const map = JSON.parse(localStorage.getItem('nemesisbot_skin_script_ok') || '{}')
    expect(map.scripty).toBe(SHA.slice(0, 16))
    expect(wsRequestMock).toHaveBeenCalledWith('skins', 'script_consent', {
      id: 'scripty',
      decision: 'allow',
    })
    expect(skinScriptNeedsReload()).toBe(true)
  })

  it('审计命令失败不推翻用户 allow（注入照常）', async () => {
    wsRequestMock.mockRejectedValue(new Error('ws down'))
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('scripty'),
      '/skins/active/script': scriptResponse(),
      '/skins/scripty/script': scriptResponse(), // allow 注入前再验闸
    }))
    await applySkinBoot()
    await settle()
    await resolveScriptConsent(true)
    expect(scriptEl()).not.toBeNull()
    const map = JSON.parse(localStorage.getItem('nemesisbot_skin_script_ok') || '{}')
    expect(map.scripty).toBe(SHA.slice(0, 16))
  })

  it('同意卡 deny → 不注入、不写缓存、审计 deny', async () => {
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('scripty'),
      '/skins/active/script': scriptResponse(),
    }))
    await applySkinBoot()
    await settle()
    await resolveScriptConsent(false)
    expect(scriptEl()).toBeNull()
    expect(skinState.scriptPending).toBe('') // 卡收起
    expect(localStorage.getItem('nemesisbot_skin_script_ok')).toBeNull()
    expect(wsRequestMock).toHaveBeenCalledWith('skins', 'script_consent', {
      id: 'scripty',
      decision: 'deny',
    })
    expect(skinScriptNeedsReload()).toBe(false)
  })

  it('缺陷 #9：同意期间总闸被关（再验 403）→ 不注入；用户授权（缓存+审计）仍成立', async () => {
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('scripty'),
      '/skins/active/script': scriptResponse(),
      '/skins/scripty/script': new Response('', { status: 403 }), // 同意期间闸被关
    }))
    await applySkinBoot()
    await settle()
    expect(skinState.scriptPending).toBe('scripty')

    await resolveScriptConsent(true)
    expect(scriptEl()).toBeNull() // 闸关 = JS 不执行（服务端权威闸单点保证）
    const map = JSON.parse(localStorage.getItem('nemesisbot_skin_script_ok') || '{}')
    expect(map.scripty).toBe(SHA.slice(0, 16)) // 授权记录成立（闸开重激活时自动注入）
    expect(wsRequestMock).toHaveBeenCalledWith('skins', 'script_consent', {
      id: 'scripty',
      decision: 'allow',
    })
    expect(skinScriptNeedsReload()).toBe(false)
  })

  it('缺陷 #9：同意期间包被换（再验 sha 失配）→ 不注入（卡上内容戳与现场不符）', async () => {
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('scripty'),
      '/skins/active/script': scriptResponse(),
      '/skins/scripty/script': scriptResponse(JS, SHA2), // 现场已是新包
    }))
    await applySkinBoot()
    await settle()
    await resolveScriptConsent(true)
    expect(scriptEl()).toBeNull()
    // 缓存 = 卡上旧包戳（用户同意的是卡上那个包；新包下次激活重新走卡）
    const map = JSON.parse(localStorage.getItem('nemesisbot_skin_script_ok') || '{}')
    expect(map.scripty).toBe(SHA.slice(0, 16))
    expect(skinScriptNeedsReload()).toBe(false)
  })

  it('discardScriptPending：闸关废弃 pending 后，allow 变 no-op（不注入不审计）', async () => {
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('scripty'),
      '/skins/active/script': scriptResponse(),
    }))
    await applySkinBoot()
    await settle()
    expect(skinState.scriptPending).toBe('scripty')

    discardScriptPending()
    expect(skinState.scriptPending).toBe('')
    await resolveScriptConsent(true)
    expect(scriptEl()).toBeNull()
    expect(wsRequestMock).not.toHaveBeenCalled()
    expect(localStorage.getItem('nemesisbot_skin_script_ok')).toBeNull()
  })

  it('裁定 2 出口 A：live 注入后 ?skin=default → 脚本现场全清（不再需要刷新）', async () => {
    localStorage.setItem('nemesisbot_skin_script_ok', JSON.stringify({ scripty: SHA.slice(0, 16) }))
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('scripty'),
      '/skins/active/script': scriptResponse(),
    }))
    await applySkinBoot()
    await settle()
    expect(skinScriptNeedsReload()).toBe(true)

    history.replaceState(null, '', '/?skin=default')
    const f = vi.fn()
    vi.stubGlobal('fetch', f)
    await applySkinBoot()
    history.replaceState(null, '', '/')
    expect(f).not.toHaveBeenCalled()
    expect(scriptEl()).toBeNull()
    expect(skinState.scriptPending).toBe('')
    expect(skinScriptNeedsReload()).toBe(false)
    // 同意缓存是用户授权记录（非观感缓存）——退出皮肤不撤销授权
    const map = JSON.parse(localStorage.getItem('nemesisbot_skin_script_ok') || '{}')
    expect(map.scripty).toBe(SHA.slice(0, 16))
  })

  it('裁定 2 出口 B：live 注入后热切到无皮肤（404）→ 脚本元素摘除', async () => {
    localStorage.setItem('nemesisbot_skin_script_ok', JSON.stringify({ scripty: SHA.slice(0, 16) }))
    vi.stubGlobal('fetch', fetchMock({
      '/skins/active.css': cssResponse('scripty'),
      '/skins/active/script': scriptResponse(),
    }))
    await applySkinBoot()
    await settle()
    expect(scriptEl()).not.toBeNull()

    vi.stubGlobal('fetch', fetchMock({}))
    await applySkinRefresh()
    await settle()
    expect(skinState.id).toBe('')
    expect(scriptEl()).toBeNull()
    expect(skinScriptNeedsReload()).toBe(false)
  })

  it('预览模式（?skin=）永不注入脚本：不发 script 请求、不挂 pending', async () => {
    history.replaceState(null, '', '/?skin=scripty')
    const f = fetchMock({
      // 预览路径 CSS 走 /skins/scripty（无 .css 后缀）；script URL 按同
      // 规则推导——若被误调用会命中此路由，断言用调用记录兜底
      '/skins/scripty': new Response(CSS, { status: 200, headers: { 'Content-Type': 'text/css' } }),
      '/skins/scripty/script': scriptResponse(),
    })
    vi.stubGlobal('fetch', f)
    await applySkinBoot()
    await settle()
    const urls = f.mock.calls.map((c: any[]) => String(c[0]))
    expect(urls.some((u: string) => u.endsWith('/script'))).toBe(false)
    expect(scriptEl()).toBeNull()
    expect(skinState.scriptPending).toBe('')
    expect(skinState.id).toBe('scripty') // 预览观感生效
  })
})
