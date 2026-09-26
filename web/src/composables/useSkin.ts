/**
 * 皮肤装载（.nbskin → /skins/*.css → `<style data-skin-sheet>` 注入）。
 *
 * 冷启动同步注入在 index.html 内联脚本（防 FOUC，WorkBuddy
 * applyCachedCssSync 同款：localStorage 缓存的 CSS 先行，等 bundle 就绪）；
 * 本模块负责**异步校准**——服务端 active.css 是真相源：
 *   - 拿到 CSS → 以 `X-Skin-Id` 响应头为准更新 `data-skin` 属性与缓存
 *     （缓存里的 id 可能过期：config 改了皮肤而浏览器缓存未清）；
 *   - 404 / 空（未配置皮肤、包缺失、`?skin=default`）→ 摘属性清缓存，
 *     回落内置皮肤。
 *
 * `?skin=<id>` 查询参数 = 临时预览覆盖（不依赖 config，直接取
 * `/skins/<id>`），适合看新皮肤效果；`?skin=default` 强制回落内置。
 *
 * `applySkinRefresh()` = 运行中重应用（设置页「皮肤」tab 的 set_active
 * 成功后调用）——同链路但无 `?skin` 预览分支，直接取 active.css；服务端
 * 404（= 皮肤已关）走同一回落链。免刷新换肤，v1 不做跨 tab push（其他
 * tab 下次刷新自然跟上）。
 *
 * 骨架槽位状态（`skinState`）：皮肤激活时 AppLayout 顶栏/状态栏、
 * ChatPanel 主页启动器按它条件渲染（`skinState.id` 非空 = 槽位在场）；
 * `skinState.meta` 来自 WSAPI `skins.detail` 的 manifest 子集（品牌名/
 * 场景标签），拉取失败（feature 裁剪 / skins 未装配 / WS 未就绪）时
 * 置空，槽位回落 id 兜底显示。
 */

import { reactive } from 'vue'
import { wsStatus } from './useWebSocket'

/** 皮肤 manifest 元数据子集（骨架槽位渲染用）。 */
export interface SkinMeta {
  /** 品牌 display 名（manifest.brand，空回落 name/id） */
  brand: string
  /** 主页启动器场景标签（manifest.scenes） */
  scenes: string[]
  /** manifest 版本（展示用） */
  version: string
}

/** 骨架槽位响应式状态（模块级单例；applySkin* 系列读写）。 */
export const skinState = reactive<{ id: string; meta: SkinMeta | null }>({
  id: '',
  meta: null,
})

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
    const { useWSAPI } = await import('./useWSAPI')
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
    // 槽位以 id 兜底；AppLayout 挂载后会再 ensureSkinMeta() 补拉一次。
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
    void refreshSkinMeta(id)
  } catch {
    // 服务端无皮肤可用（未配置 / 包缺失 / 404）→ 回落内置皮肤
    root.removeAttribute('data-skin')
    localStorage.removeItem('nemesisbot_skin')
    localStorage.removeItem('nemesisbot_skin_css')
    document.head.querySelector('style[data-skin-sheet]')?.remove()
    skinState.id = ''
    skinState.meta = null
  }
}

export async function applySkinBoot(): Promise<void> {
  const q = new URLSearchParams(location.search).get('skin')

  // 显式退出皮肤：摘属性清缓存，不再请求
  if (q === 'default') {
    const root = document.documentElement
    root.removeAttribute('data-skin')
    localStorage.removeItem('nemesisbot_skin')
    localStorage.removeItem('nemesisbot_skin_css')
    document.head.querySelector('style[data-skin-sheet]')?.remove()
    return
  }

  await applyFromUrl(q ? `/skins/${q}` : '/skins/active.css', q || '')
}

/** 运行中重应用当前皮肤（无 `?skin` 预览分支；设置页皮肤 tab 热切用）。 */
export async function applySkinRefresh(): Promise<void> {
  await applyFromUrl('/skins/active.css', '')
}
