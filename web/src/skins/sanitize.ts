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
 * 可放自有 logo 资源，跟踪像素属「资源可控」议题非代码执行）。 */
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

/** 就地清洗一棵子树（元素删整子树、属性剥危险值、注释删）。 */
function sanitizeTree(parent: ParentNode): void {
  for (const child of Array.from(parent.childNodes)) {
    if (child.nodeType === 1 /* ELEMENT_NODE */) {
      const el = child as Element
      if (STRIP_ELEMENTS.has(el.localName.toLowerCase())) {
        el.remove()
        continue
      }
      sanitizeAttrs(el)
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
    if (URL_ATTRS.has(name)) {
      const blocked =
        name === 'srcset' ? isBlockedSrcset(attr.value) : isBlockedUrl(attr.value)
      if (blocked) el.removeAttribute(attr.name)
    }
  }
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
