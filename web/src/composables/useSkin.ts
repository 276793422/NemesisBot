/**
 * 皮肤装载（.nbskin → CSS 端点 + structure 端点 + script 端点 → 注入）。
 *
 * 冷启动同步注入在 index.html 内联脚本（防 FOUC）：localStorage 缓存的
 * CSS 先行 + 结构缓存回放为 inert `<template data-nb-structure-cache>`
 * （不进活 DOM，等 SkinSlot 消费）；本模块负责**异步校准**——服务端是
 * 真相源：
 *   - CSS：`/skins/active.css`（或预览 `/skins/<id>`）→ `X-Skin-Id` 头
 *     校准 `data-skin` 属性与缓存；
 *   - 结构：同 id 的 `/skins/active/structure`（或 `/skins/<id>/structure`）
 *     → 清洗 → 与已挂内容比对 → 变化才原子重建（structRev++）；404 /
 *     版本不符 / 清洗掏空 → 摘结构回落原生布局（CSS 照常 = 纯换色包）；
 *   - 脚本（P2a）：同 id 的 `/skins/active/script`（或 `/skins/<id>/script`）
 *     → 404/403 静默（无脚本 / 闸关 = 纯 CSS 模式）→ 200 查同意缓存
 *     （`nemesisbot_skin_script_ok`，按整包 sha256 前 16 位盖章 = 包一
 *     更新即需重新同意）→ 已同意注入执行 / 未同意挂 `scriptPending`
 *     （CSS 先生效不阻塞，等设置页同意卡裁决）；
 *   - 全部失败 → 摘属性清缓存，回落内置观感。
 *
 * **预览模式（`?skin=<id>`）永不注入脚本**——预览只看观感；脚本只在
 * 真实激活（`?skin=` 缺省的 active 路径）后注入。
 *
 * **刷新边界（裁定 2）**：live 注入的 JS 状态不可被 CSS 热切干净卸载——
 * `scriptLiveId` 非空时任何出口（切向任何目的地 / 运行中关开关）由调用方
 * `location.reload()`（`skinScriptNeedsReload()` 判据）；从无脚本状态进入
 * 脚本皮肤 = 渐进注入免刷新。
 *
 * `?skin=<id>` 查询参数 = 临时预览覆盖；`?skin=default` 强制回落内置
 * （CSS 与结构缓存一并清）。`applySkinRefresh()` = 运行中重应用（设置页
 * 皮肤 tab 热切用），免刷新换肤。
 *
 * 结构引擎（v2）：`getSkinEngine()` 模块单例；`skinState.slots` 记录当前
 * 包提供的槽位名，`skinHasSlot(name)` 是挂载点条件的唯一判据——CSS-only
 * 包 slots 为空 = 全部槽位回落原生组件；`structRev` 驱动 SkinSlot 原子
 * 重建。`consumeStructureCache()` 同步消费 T0 缓存（SkinSlot 首挂时调用，
 * 幂等——命中时皮肤骨架随首帧渲染，无原生闪）。
 */

import { reactive } from 'vue'
import { wsStatus } from './useWebSocket'
import { useWSAPI } from './useWSAPI'
import { parseSkinStructure } from '../skins/sanitize'
import { SkinStructureEngine } from '../skins/engine'

/** 皮肤 manifest 元数据子集（投影 brand/version/scenes 来源）。 */
export interface SkinMeta {
  /** 品牌 display 名（manifest.brand，空回落 name/id） */
  brand: string
  /** 主页启动器场景标签（manifest.scenes） */
  scenes: string[]
  /** manifest 版本（展示用） */
  version: string
}

const STRUCTURE_CACHE_KEY = 'nemesisbot_skin_structure'
/** 脚本同意缓存：`{ [skinId]: <整包 sha256 前 16 位> }`——按包内容重确认
 * （包一更新即失配 → 重新走同意卡）。 */
const SCRIPT_OK_KEY = 'nemesisbot_skin_script_ok'

/** 皮肤装载响应式状态（模块级单例；applySkin* 系列读写）。
 * slots = 当前包提供的槽位名；structRev 递增驱动 SkinSlot 原子重建；
 * scriptPending = 待同意脚本的皮肤 id（'' = 无；设置页同意卡 watch 用）。 */
export const skinState = reactive<{
  id: string
  meta: SkinMeta | null
  slots: string[]
  structRev: number
  scriptPending: string
}>({
  id: '',
  meta: null,
  slots: [],
  structRev: 0,
  scriptPending: '',
})

// ---- 结构引擎单例 ----

let engine: SkinStructureEngine | null = null

/** 结构引擎单例（projection 装配投影/动作；SkinSlot 执行挂载）。 */
export function getSkinEngine(): SkinStructureEngine {
  if (!engine) engine = new SkinStructureEngine()
  return engine
}

/** 槽位在场判定（挂载点条件唯一判据；CSS-only 包恒 false = 原生组件）。 */
export function skinHasSlot(name: string): boolean {
  return skinState.slots.includes(name)
}

/** 清洗后槽位序列化（localStorage 结构缓存格式 = template 拼接）。 */
function serializeSlots(slots: Map<string, DocumentFragment>): string {
  const box = document.createElement('div')
  return [...slots.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([n, f]) => {
      box.textContent = ''
      box.appendChild(f.cloneNode(true))
      return `<template data-nb-slot="${n}">${box.innerHTML}</template>`
    })
    .join('')
}

/** 已挂载结构的签名（T3 校准比较用：内容一致不重建）。 */
let lastStructureSig = ''

/**
 * 采纳结构 HTML：清洗 → 拒载判定（版本协商失败/掏空 = null）→ 内容
 * 变化才 engine.load + structRev++（原子重建）。返回清洗后序列化
 * （缓存写用），拒载返回 null。
 */
function adoptStructure(html: string): string | null {
  const parsed = parseSkinStructure(html)
  if (!parsed || parsed.slots.size === 0) return null
  const serialized = serializeSlots(parsed.slots)
  if (serialized !== lastStructureSig) {
    getSkinEngine().load(parsed.slots)
    skinState.slots = [...parsed.slots.keys()]
    skinState.structRev++
    lastStructureSig = serialized
  }
  return serialized
}

/** 摘结构回落原生布局（404/拒载/皮肤关闭共用；CSS 不受影响）。 */
export function clearSkinStructure(): void {
  localStorage.removeItem(STRUCTURE_CACHE_KEY)
  lastStructureSig = ''
  getSkinEngine().load(new Map())
  skinState.slots = []
  skinState.structRev++
}

/**
 * 同步消费 T0 冷启动结构缓存（index.html 注入的 inert template；id 一致
 * 性已在写入侧保证）。天然幂等：消费即摘除，无 template = no-op。命中且
 * 清洗通过 → 皮肤骨架随首帧渲染；拒载 → 静默回落（T3 校准会再纠正）。
 */
export function consumeStructureCache(): void {
  const t = document.head.querySelector('template[data-nb-structure-cache]')
  if (!t) return
  const html = t.innerHTML
  t.remove()
  adoptStructure(html)
}

// ---- 脚本载荷（P2a：闸在服务端 403；同意在前端，审计走 skins.script_consent）----

/** 待同意脚本现场（id/js/sha）；`skinState.scriptPending` 是其响应式投影。 */
let pendingScript: { id: string; js: string; sha: string } | null = null

/** live 注入脚本的皮肤 id（'' = 无脚本在跑）。**不可逆状态**——非空时
 * 任何皮肤切换/开关关闭都必须整页刷新（JS 无法干净卸载）。 */
let scriptLiveId = ''

/** live 脚本在场判据（裁定 2 刷新边界的唯一判据；设置页切换/开关用）。 */
export function skinScriptNeedsReload(): boolean {
  return scriptLiveId !== ''
}

function readConsentMap(): Record<string, string> {
  try {
    return JSON.parse(localStorage.getItem(SCRIPT_OK_KEY) || '{}') as Record<string, string>
  } catch {
    return {}
  }
}

/** 已同意且包未变（sha 前 16 位盖章一致）→ 免卡直接注入。 */
function hasScriptConsent(id: string, sha: string): boolean {
  return readConsentMap()[id] === sha.slice(0, 16)
}

/** 注入执行（内联 textContent 免二次下载；先摘旧防重复注入）。 */
function injectSkinScript(id: string, js: string): void {
  document.head
    .querySelectorAll('script[data-nb-skin-script]')
    .forEach((el) => el.remove())
  const el = document.createElement('script')
  el.setAttribute('data-nb-skin-script', id)
  el.textContent = js
  document.head.appendChild(el)
  scriptLiveId = id
}

/**
 * 脚本载荷并行拉取（CSS/结构链并行；**预览路径不调用**——`?skin=` 永不
 * 注入脚本）。裁决：404 = 无脚本结束；403 = 闸关（纯 CSS 模式，静默）；
 * 200 = 查同意缓存 → 已同意注入 / 未同意挂 pending（CSS 先生效不阻塞）。
 */
async function loadScriptFor(id: string, cssUrl: string): Promise<void> {
  const scriptUrl = `${cssUrl.replace(/\.css$/, '')}/script`
  try {
    const res = await fetch(scriptUrl, { cache: 'no-store' })
    if (!res.ok) return // 404 无脚本 / 403 闸关 / 其他：一律纯 CSS 继续
    const js = await res.text()
    const sha = res.headers.get('X-Skin-Sha256') || ''
    if (skinState.id !== id || !js || !sha) return // 拉取期间已切走 / 现场不全
    if (hasScriptConsent(id, sha)) {
      injectSkinScript(id, js)
    } else {
      pendingScript = { id, js, sha }
      skinState.scriptPending = id
    }
  } catch {
    // 网络失败：CSS 已生效，脚本静默放弃（下次刷新重试）
  }
}

/** 废弃未裁决的 pending 脚本（总闸关闭时调用——裁定 2 规则表「开关关 =
 * JS 不执行」：卡上 bytes-in-hand 不得在闸关后仍可被同意注入）。 */
export function discardScriptPending(): void {
  pendingScript = null
  skinState.scriptPending = ''
}

/**
 * 同意卡裁决（设置页调用；allow=true 注入并写同意缓存，false 仅清除
 * pending）。审计走 WSAPI `skins.script_consent`（失败不阻塞本地决定
 * ——授权在用户，审计尽力而为）。allow 后**注入前服务端再验闸**：卡上
 * 字节是闸开时抓的（bytes-in-hand 绕过服务端 403 权威闸），同意期间总闸
 * 可能已被关（再验 403）或包可能已被换（sha 失配）——再验不过 = 环境
 * 已变，不注入（同意缓存与审计已记用户授权；下次激活走正常链重判）。
 * 注入仍要求当前激活皮肤仍是该包且无 live 脚本（防串台/重复注入）。
 */
export async function resolveScriptConsent(allow: boolean): Promise<void> {
  const p = pendingScript
  if (!p) return
  pendingScript = null
  skinState.scriptPending = ''
  try {
    await useWSAPI().request('skins', 'script_consent', {
      id: p.id,
      decision: allow ? 'allow' : 'deny',
    })
  } catch {
    // 审计命令失败（WS 断开/模块裁剪）不推翻用户决定
  }
  if (!allow) return
  const map = readConsentMap()
  map[p.id] = p.sha.slice(0, 16)
  localStorage.setItem(SCRIPT_OK_KEY, JSON.stringify(map))
  try {
    const res = await fetch(`/skins/${p.id}/script`, { cache: 'no-store' })
    const sha = res.ok ? res.headers.get('X-Skin-Sha256') || '' : ''
    if (!res.ok || sha !== p.sha) return // 闸关（403）/ 包被换（sha 失配）：不注入
    const js = await res.text()
    if (skinState.id === p.id && !scriptLiveId) injectSkinScript(p.id, js)
  } catch {
    // 再验网络失败：不注入（fail-closed；下次激活重判）
  }
}

// ---- 元数据 ----

/** 元数据补拉（皮肤在场上但 meta 未就绪时调用；幂等）。 */
export async function ensureSkinMeta(): Promise<void> {
  const id = skinState.id
  if (!id || skinState.meta) return
  await refreshSkinMeta(id)
}

/** 等 WS 就绪（认证前 skin init 先跑，WSAPI 得等连接建立；上限 20s）。 */
async function waitWsReady(id: string, timeoutMs = 20000): Promise<boolean> {
  const deadline = Date.now() + timeoutMs
  while (wsStatus.value !== 'connected' && Date.now() < deadline) {
    if (skinState.id !== id) return false // 拉取前皮肤已切走
    await new Promise((r) => setTimeout(r, 300))
  }
  return wsStatus.value === 'connected'
}

async function refreshSkinMeta(id: string): Promise<void> {
  skinState.meta = null
  // 认证前 skin init 就会走到这里；WSAPI 走 WS，连接未建立时 sendRaw
  // 只会排队并触发无 token 的重连（服务端 401 循环）。等就绪再取。
  if (!(await waitWsReady(id))) return
  try {
    const { request } = useWSAPI()
    const detail = (await request('skins', 'detail', { id })) as {
      skin?: { manifest?: Record<string, unknown> }
    }
    // 拉取期间皮肤已切走 → 丢弃（防串台）
    if (skinState.id !== id) return
    const m = detail?.skin?.manifest
    if (!m) return
    const name = String(m.name || id)
    skinState.meta = {
      brand: String(m.brand || name),
      scenes: Array.isArray(m.scenes) ? m.scenes.map(String) : [],
      version: String(m.version || ''),
    }
  } catch {
    // skins 模块不可用（feature 裁剪 / 未装配 / 请求失败）→ meta 保持空，
    // 投影以 id 兜底；AppLayout 挂载后会再 ensureSkinMeta() 补拉一次。
  }
}

// ---- 装载链 ----

/** 结构载荷并行拉取（CSS 成功后启动；404 = 纯 CSS 换色包 → 原生布局）。 */
async function loadStructureFor(id: string, cssUrl: string): Promise<void> {
  const structureUrl = `${cssUrl.replace(/\.css$/, '')}/structure`
  try {
    const res = await fetch(structureUrl, { cache: 'no-store' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const html = await res.text()
    if (skinState.id !== id) return // 拉取期间皮肤已切走
    const serialized = adoptStructure(html)
    if (!serialized) throw new Error('structure rejected')
    localStorage.setItem(STRUCTURE_CACHE_KEY, serialized)
  } catch {
    if (skinState.id === id) clearSkinStructure()
  }
}

async function applyFromUrl(url: string, forcedId: string): Promise<void> {
  const root = document.documentElement
  try {
    const res = await fetch(url, { cache: 'no-store' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const css = (await res.text()).trim()
    if (!css) throw new Error('empty skin css')

    // id 真相源：预览参数 > active.css 的 X-Skin-Id 头 > 本地缓存
    const id =
      forcedId ||
      res.headers.get('X-Skin-Id') ||
      localStorage.getItem('nemesisbot_skin') ||
      ''
    if (!id) throw new Error('no skin id')

    root.setAttribute('data-skin', id)
    let sheet = document.head.querySelector<HTMLStyleElement>(
      'style[data-skin-sheet]'
    )
    if (!sheet) {
      sheet = document.createElement('style')
      sheet.setAttribute('data-skin-sheet', '')
      document.head.appendChild(sheet)
    }
    sheet.textContent = css

    localStorage.setItem('nemesisbot_skin', id)
    localStorage.setItem('nemesisbot_skin_css', css)

    skinState.id = id
    // pending 卡只能指向当前皮肤：切到别的皮肤时废弃旧 pending（同 id
    // 重应用保留——loadScriptFor 会重判覆盖）。
    if (pendingScript && pendingScript.id !== id) discardScriptPending()
    void refreshSkinMeta(id)
    // 结构链与 CSS 校准并行（CSS-only 包 → 404 → 原生布局回落）
    void loadStructureFor(id, url)
    // 脚本链并行（P2a）——**预览路径（forcedId 非空）永不注入**：预览只
    // 看观感；脚本只在真实激活（active 路径）后注入。
    if (!forcedId) void loadScriptFor(id, url)
  } catch {
    // 服务端无皮肤可用（未配置 / 包缺失 / 404）→ 回落内置皮肤
    root.removeAttribute('data-skin')
    localStorage.removeItem('nemesisbot_skin')
    localStorage.removeItem('nemesisbot_skin_css')
    document.head.querySelector('style[data-skin-sheet]')?.remove()
    document.head
      .querySelectorAll('script[data-nb-skin-script]')
      .forEach((el) => el.remove())
    scriptLiveId = ''
    pendingScript = null
    skinState.scriptPending = ''
    clearSkinStructure()
    skinState.id = ''
    skinState.meta = null
  }
}

export async function applySkinBoot(): Promise<void> {
  const q = new URLSearchParams(location.search).get('skin')

  // 显式退出皮肤：摘属性清缓存（含结构与脚本现场），不再请求
  if (q === 'default') {
    const root = document.documentElement
    root.removeAttribute('data-skin')
    localStorage.removeItem('nemesisbot_skin')
    localStorage.removeItem('nemesisbot_skin_css')
    document.head.querySelector('style[data-skin-sheet]')?.remove()
    document.head
      .querySelectorAll('script[data-nb-skin-script]')
      .forEach((el) => el.remove())
    scriptLiveId = ''
    pendingScript = null
    skinState.scriptPending = ''
    clearSkinStructure()
    return
  }

  await applyFromUrl(q ? `/skins/${q}` : '/skins/active.css', q || '')
}

/** 运行中重应用当前皮肤（无 `?skin` 预览分支；设置页皮肤 tab 热切用）。 */
export async function applySkinRefresh(): Promise<void> {
  await applyFromUrl('/skins/active.css', '')
}
