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
 */

export async function applySkinBoot(): Promise<void> {
  const q = new URLSearchParams(location.search).get('skin')
  const root = document.documentElement

  // 显式退出皮肤：摘属性清缓存，不再请求
  if (q === 'default') {
    root.removeAttribute('data-skin')
    localStorage.removeItem('nemesisbot_skin')
    localStorage.removeItem('nemesisbot_skin_css')
    document.head.querySelector('style[data-skin-sheet]')?.remove()
    return
  }

  const url = q ? `/skins/${q}` : '/skins/active.css'
  try {
    const res = await fetch(url, { cache: 'no-store' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const css = (await res.text()).trim()
    if (!css) throw new Error('empty skin css')

    // id 真相源：预览参数 > active.css 的 X-Skin-Id 头 > 本地缓存
    const id =
      q ||
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
  } catch {
    // 服务端无皮肤可用（未配置 / 包缺失 / 404）→ 回落内置皮肤
    root.removeAttribute('data-skin')
    localStorage.removeItem('nemesisbot_skin')
    localStorage.removeItem('nemesisbot_skin_css')
    document.head.querySelector('style[data-skin-sheet]')?.remove()
  }
}
