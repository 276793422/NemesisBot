/**
 * 桥子路径 base 感知（goal：反向桥与多设备汇聚，一期批次二）。
 *
 * 经中继远程访问本机面板时页面运行在 `/d/<node_id>/` 子路径下，设备侧
 * web server 会在 HTML 里注入 `<base href="/d/<node_id>/">`（配合 vite
 * 相对构建解决静态资源引用）；但 `<base>` 不影响 JS 运行时构造的 URL
 * （`/api/...`、`/ws` 等根相对字符串会打到中继根）——所有 fetch/
 * WebSocket/EventSource 构造统一经本 helper 加前缀。
 *
 * 本地直连时页面无 `<base>` 标签，`appBase()` 返回空串 → 行为与现状
 * 完全一致（零回归）。
 */

/** 读取设备侧注入的 base 前缀（直连 = 空串；经桥 = `/d/<node_id>`）。 */
export function appBase(): string {
  const href = document.querySelector('base')?.getAttribute('href')
  if (!href || href === '/') return ''
  return href.endsWith('/') ? href.slice(0, -1) : href
}

/** API 路径加 base 前缀（path 以 `/` 开头）。 */
export function apiUrl(path: string): string {
  return appBase() + path
}

/** WebSocket URL 加 base 前缀（path 以 `/` 开头）。 */
export function wsUrl(path: string): string {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return protocol + '//' + window.location.host + appBase() + path
}
