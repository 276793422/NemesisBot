// 外链 scheme 闸（单一真相源）：只放行 http(s) 绝对 URL。
//
// 背景（BUG-20261101-274）：board 交付记录的 external_url / MCP 供应商链接
// 源自 connector 调用的模型回复——被注入的输出可携带 `javascript:` 伪
// URL，直接绑 `:href` 即成点击执行向量（Dashboard 域内持全量 WSAPI
// 权限）。显示文本走 {{ }} 插值天然安全；只有 href/src 属性绑定需要
// 本闸。后端 flow_engine `safe_external_url` 是源头闸（落账前），本函数是
// 渲染闸（防历史脏数据与其他写入口）。
//
// 用法：`:href="safeExternalUrl(u) || undefined"`——unsafe 返 ''，与
// undefined 或运算后 Vue 省略 href 属性 → 锚点惰性化为纯文本。
const SAFE_EXTERNAL_URL_RE = /^https?:\/\//i

export function safeExternalUrl(url: string | null | undefined): string {
  if (!url) return ''
  const u = url.trim()
  return SAFE_EXTERNAL_URL_RE.test(u) ? u : ''
}
