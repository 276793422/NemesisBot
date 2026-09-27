/**
 * 皮肤结构清洗（安全边界）——**不信任包作者**。
 *
 * structure.html 经本模块清洗后才允许进缓存与挂载：
 * - 剥离危险元素（整子树）：script / iframe / frame / frameset / object /
 *   embed / applet / base / meta / link / noscript / template（槽载体
 *   本身除外——槽**内部**嵌套 template 一律删，防槽逃逸）；svg/math 内
 *   同样命中（按 localName 判定）。
 * - 剥离属性：全部 `on*`、`srcdoc`；URL 属性值匹配 `^\s*(javascript|
 *   vbscript):i`（script scheme 注入）；`data:` URL 仅放行栅格图片 MIME
 *   （脚本惰性子资源，包内联图标合法用途），`data:text/html`、
 *   `data:image/svg+xml`（导航面可携带 script）、`data:application/*` 等
 *   一律剥除——实弹 CDP 验证打出（2026-09-27），单测 corpus 已补。
 *   `srcset` 逐候选检查（整值正则会漏 `a.jpg 1x, javascript:x 2x` 嵌入形态）。
 * - 注释整体删除（防 IE 条件注释复活，产物干净）。
 * - 保留：class/id/style/data- 与 aria- 前缀属性、svg 形状、`<style>`、
 *   `<form>`（form 无提交通道——formaction 已剥、submit 走 click 代理白名单）。
 * - **CSS 外链零请求**（2026-09-28 加固，与 Rust `crates/nemesis-web/src/
 *   skins.rs::sanitize_css_urls` 同规则同 corpus）：`<style>` 文本内容与
 *   `style` 属性值经 `sanitizeCssUrls`——`url(…)` / `@import "…"` 目标为
 *   http(s)/协议相对/非白名单 data: 时改写 `about:blank`（v2 时代「跟踪
 *   像素属资源可控议题」的放行立场同步废止）；CSS 转义解码后扫描（
 *   `\75rl(` 现形），输出字面 `\` 再转义防二次解码走私。
 *
 * localStorage 结构缓存**只存清洗后 HTML**；恶意 corpus 单测只增不减。
 */

import { SKIN_ENGINE_VERSION } from './types'

/** 整子树剥离的元素（localName 小写比较；svg/math 内同样命中）。 */
const STRIP_ELEMENTS = new Set([
  'script',
  'iframe',
  'frame',
  'frameset',
  'object',
  'embed',
  'applet',
  'base',
  'meta',
  'link',
  'noscript',
  'template',
])

/** URL 语义属性（值匹配 script scheme 时剥除；其余 URL 值放行——包作者
 * 可放自有 logo 资源；外链 CSS 面的零请求语义由 sanitizeCssUrls 承担，
 * 本表不拦外链 URL 本身）。 */
const URL_ATTRS = new Set([
  'href',
  'src',
  'xlink:href',
  'action',
  'formaction',
  'poster',
  'cite',
  'srcset',
  'background',
  'dynsrc',
  'lowsrc',
  'data',
  'codebase',
  'icon',
  'longdesc',
  'usemap',
  'manifest',
  'profile',
])

const SCRIPT_SCHEME = /^\s*(javascript|vbscript):/i

/** data: URL 放行面 = 仅栅格图片 MIME（脚本惰性；svg+xml 文档可携带
 * script 不放行——结构内图标走内联 <svg> 元素）。 */
const DATA_URL_ALLOWED =
  /^\s*data:image\/(png|jpe?g|gif|webp|bmp|avif|apng|x-icon|vnd\.microsoft\.icon|tiff?)[;,]/i
const DATA_URL_SCHEME = /^\s*data:/i

/** 单个 URL 值是否危险（script scheme / 非 白名单 data:）。 */
function isBlockedUrl(value: string): boolean {
  if (SCRIPT_SCHEME.test(value)) return true
  return DATA_URL_SCHEME.test(value) && !DATA_URL_ALLOWED.test(value)
}

/** srcset 值逐候选取 URL 段检查（`a.png 1x, javascript:x 2x` 嵌入形态）。 */
function isBlockedSrcset(value: string): boolean {
  return value.split(',').some((candidate) => {
    const url = candidate.trim().split(/\s+/)[0] ?? ''
    return url !== '' && isBlockedUrl(url)
  })
}

/** 就地清洗一棵子树（元素删整子树、属性剥危险值、注释删、CSS 外链零请求）。 */
function sanitizeTree(parent: ParentNode): void {
  for (const child of Array.from(parent.childNodes)) {
    if (child.nodeType === 1 /* ELEMENT_NODE */) {
      const el = child as Element
      if (STRIP_ELEMENTS.has(el.localName.toLowerCase())) {
        el.remove()
        continue
      }
      sanitizeAttrs(el)
      if (el.localName.toLowerCase() === 'style') {
        el.textContent = sanitizeCssUrls(el.textContent ?? '')
      }
      sanitizeTree(el)
    } else if (child.nodeType === 8 /* COMMENT_NODE */) {
      child.remove()
    }
  }
}

function sanitizeAttrs(el: Element): void {
  for (const attr of Array.from(el.attributes)) {
    const name = attr.name.toLowerCase()
    if (name.startsWith('on') || name === 'srcdoc') {
      el.removeAttribute(attr.name)
      continue
    }
    if (name === 'style') {
      // style 属性值 = 内联声明列表：url( 信标同样改写（@import 在属性
      // 里本就非法，扫描无害）。
      const cleaned = sanitizeCssUrls(el.getAttribute('style') ?? '')
      el.setAttribute('style', cleaned)
      continue
    }
    if (URL_ATTRS.has(name)) {
      const blocked =
        name === 'srcset' ? isBlockedSrcset(attr.value) : isBlockedUrl(attr.value)
      if (blocked) el.removeAttribute(attr.name)
    }
  }
}

// ---------------------------------------------------------------------------
// CSS 外链零请求（2026-09-28 加固；与 Rust sanitize_css_urls 同规则）
// ---------------------------------------------------------------------------

/** CSS 转义解码（CSS Syntax L3）+ 字面 `\` 再转义为 `\\`：`\75rl(` 解码
 * 后现形为 `url(`；再转义保证「浏览器解析输出 ≡ 浏览器解析原文」（否则
 * `\5c 75rl(http://…)` 会被浏览器对输出二次解码成 url( 走私）。 */
function decodeCssEscapes(css: string): string {
  let out = ''
  const n = css.length
  let i = 0
  while (i < n) {
    const c = css[i]
    if (c !== '\\') {
      out += c
      i++
      continue
    }
    let hex = ''
    let j = i + 1
    while (hex.length < 6 && j < n && /[\da-fA-F]/.test(css[j])) {
      hex += css[j]
      j++
    }
    if (hex) {
      let cp = parseInt(hex, 16)
      if (cp === 0 || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) cp = 0xfffd
      out += String.fromCodePoint(cp)
      if (j < n && /[ \t\n\r\f]/.test(css[j])) j++ // 至多一个空白终结符
      i = j
      continue
    }
    const nx = css[i + 1]
    if (nx === '\n') {
      i += 2 // \LF 续行：双消
    } else if (nx === '\r') {
      i += css[i + 2] === '\n' ? 3 : 2
    } else if (nx === undefined) {
      i++ // 尾部孤立 `\`：丢弃（浏览器同语义）
    } else {
      out += nx
      i += 2
    }
  }
  return out.replace(/\\/g, '\\\\')
}

/** 外链/非白名单 data: 判据（url( 目标与 @import 字符串目标共用）。 */
function cssTargetBlocked(tok: string): boolean {
  const t = tok.toLowerCase()
  const raster =
    ['image/png', 'image/jpeg', 'image/jpg', 'image/gif', 'image/webp'].indexOf(
      (t.slice(5).split(';')[0] ?? '').trim(),
    ) >= 0
  return (
    t.startsWith('https://') ||
    t.startsWith('http://') ||
    t.startsWith('//') ||
    (t.startsWith('data:') && !raster)
  )
}

/** 皮肤 CSS 外链零请求消毒：`url(…)` / `@import "…"` 目标为外链/坏
 * data: 时改写 `about:blank`。CSS 先转义解码（`\75rl(`、`\68 ttp://`
 * 现形）再扫描。与 Rust 侧 `sanitize_css_urls` 同 corpus 钉死。 */
export function sanitizeCssUrls(css: string): string {
  const decoded = decodeCssEscapes(css)
  // 逐字符 ASCII 小写（保持与 decoded 1:1 索引对齐——Unicode 形态的
  // toLowerCase 可能变长，切片会错位）。
  const lower = decoded.replace(/[A-Z]/g, (c) => c.toLowerCase())
  const isWs = (i: number) => i < lower.length && /[ \t\n\r\f]/.test(lower[i])
  let out = ''
  let off = 0
  let i = 0
  const n = lower.length
  while (i < n) {
    if (lower.startsWith('url(', i)) {
      // url( token：引号/裸目标，外链 → about:blank（引号结构保留）
      let j = i + 4
      while (isWs(j)) j++
      const q = lower[j] === '"' || lower[j] === "'" ? lower[j] : undefined
      const tokStart = q ? j + 1 : j
      let k = tokStart
      let closed = false
      while (k < n) {
        if (q ? lower[k] === q : lower[k] === ')') {
          closed = true
          break
        }
        k++
      }
      if (!closed) {
        out += decoded.slice(off)
        return out
      }
      let close = k
      if (q) {
        close = lower.indexOf(')', k + 1)
        if (close < 0) {
          out += decoded.slice(off)
          return out
        }
      }
      out += decoded.slice(off, tokStart)
      if (cssTargetBlocked(lower.slice(tokStart, k))) {
        out += 'about:blank'
      } else {
        out += decoded.slice(tokStart, k)
      }
      out += decoded.slice(k, close + 1)
      off = close + 1
      i = close + 1
    } else if (lower.startsWith('@import', i)) {
      const nx = lower[i + 7]
      if (nx !== undefined && !/[ \t\n\r\f"']/.test(nx)) {
        i++
        continue // @importer 等普通标识符不误命中
      }
      // @import 字符串形态：目标外链 → 内容 about:blank、引号保留；
      // url( 形态留给上面的 url( 分支。
      let j = i + 7
      while (isWs(j)) j++
      const q = lower[j]
      if (q !== '"' && q !== "'") {
        i++
        continue
      }
      const tokStart = j + 1
      let k = tokStart
      while (k < n && lower[k] !== q) k++
      if (k >= n) {
        out += decoded.slice(off)
        return out
      }
      if (cssTargetBlocked(lower.slice(tokStart, k))) {
        out += decoded.slice(off, tokStart)
        out += 'about:blank'
        out += decoded.slice(k, k + 1)
        off = k + 1
        i = k + 1
      } else {
        i = tokStart // 合法目标：主循环从内容处继续（宁严勿漏）
      }
    } else {
      i++
    }
  }
  out += decoded.slice(off)
  return out
}

/** 解析产物：协议版本 + 清洗后槽位表（槽名 → template content 克隆源）。 */
export interface ParsedSkinStructure {
  engine: number
  slots: Map<string, DocumentFragment>
}

/**
 * 解析 + 清洗 structure HTML。
 *
 * 约定：structure.html = 若干 `<template data-nb-slot="…">` 兄弟节点；
 * 协议版本由任一元素上的 `data-nb-engine` 声明（缺省 = 1）。返回
 * `null` = 整包拒载（engine 高于引擎支持值）。结构错误（无槽/槽名空、
 * 重复、engine 非法）按能力降级：空槽跳过、重复先到先得。
 */
export function parseSkinStructure(html: string): ParsedSkinStructure | null {
  const doc = new DOMParser().parseFromString(html, 'text/html')

  // 协议版本协商：高于引擎支持 → 拒载（回落原生 UI 由上层处理）。
  const engineEl = doc.querySelector('[data-nb-engine]')
  let engine = 1
  if (engineEl) {
    const v = Number.parseInt(engineEl.getAttribute('data-nb-engine') ?? '', 10)
    if (Number.isFinite(v) && v > 0) engine = v
  }
  if (engine > SKIN_ENGINE_VERSION) return null

  const slots = new Map<string, DocumentFragment>()
  for (const tpl of doc.querySelectorAll('template[data-nb-slot]')) {
    const name = (tpl.getAttribute('data-nb-slot') ?? '').trim()
    if (!name || slots.has(name)) continue
    const content = (tpl as HTMLTemplateElement).content
    sanitizeTree(content)
    slots.set(name, content)
  }
  if (slots.size === 0) return null
  return { engine, slots }
}
