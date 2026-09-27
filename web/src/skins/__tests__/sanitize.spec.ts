import { describe, it, expect } from 'vitest'
import { parseSkinStructure } from '../sanitize'
import {
  STRUCTURE_MINIMAL,
  STRUCTURE_ENGINE_TOO_NEW,
  STRUCTURE_NO_SLOTS,
  MALICIOUS_SCRIPT_TAG,
  MALICIOUS_SCRIPT_IN_SVG,
  MALICIOUS_IFRAME_OBJECT,
  MALICIOUS_EVENT_HANDLERS,
  MALICIOUS_JS_URLS,
  MALICIOUS_SRCDOC,
  MALICIOUS_NESTED_TEMPLATE,
  MALICIOUS_COMMENT,
  ALLOWED_DATA_URLS,
} from './fixtures'

/** 清洗后 HTML 里不允许出现的任何痕迹。 */
function assertNoDanger(frag: DocumentFragment): void {
  const html = (frag as ParentNode).textContent ?? ''
  for (const el of frag.querySelectorAll('*')) {
    const tag = el.localName.toLowerCase()
    expect(['script', 'iframe', 'frame', 'frameset', 'object', 'embed', 'applet', 'base', 'meta', 'link', 'noscript', 'template']).not.toContain(tag)
    for (const attr of Array.from(el.attributes)) {
      expect(attr.name.toLowerCase().startsWith('on'), `on* 属性残留: ${attr.name}`).toBe(false)
      expect(attr.name.toLowerCase()).not.toBe('srcdoc')
      expect(attr.value.toLowerCase()).not.toContain('javascript:')
      expect(attr.value.toLowerCase()).not.toContain('vbscript:')
      expect(attr.value.toLowerCase()).not.toContain('data:text/html')
      expect(attr.value.toLowerCase()).not.toContain('data:image/svg+xml')
      expect(attr.value.toLowerCase()).not.toContain('data:application')
    }
  }
  // 内容层也不允许内联事件字符串漏网（textContent 只会含纯文本，双保险）
  expect(html).not.toContain('window.__pwned')
}

describe('parseSkinStructure：解析与协议协商', () => {
  it('最小结构：两槽解析、engine=1、槽内容为 DocumentFragment', () => {
    const r = parseSkinStructure(STRUCTURE_MINIMAL)
    expect(r).not.toBeNull()
    expect(r!.engine).toBe(1)
    expect([...r!.slots.keys()].sort()).toEqual(['sidebar', 'titlebar'])
    expect(r!.slots.get('titlebar')!.querySelector('[data-nb-bind="brand"]')).toBeTruthy()
  })

  it('engine 高于支持值 → null（整包拒载）', () => {
    expect(parseSkinStructure(STRUCTURE_ENGINE_TOO_NEW)).toBeNull()
  })

  it('无槽 → null', () => {
    expect(parseSkinStructure(STRUCTURE_NO_SLOTS)).toBeNull()
  })

  it('engine 缺省 = 1；重复槽名先到先得', () => {
    const r = parseSkinStructure(
      `<template data-nb-slot="titlebar"><b>first</b></template>` +
        `<template data-nb-slot="titlebar"><b>second</b></template>` +
        `<template data-nb-slot=""><b>匿名槽忽略</b></template>`
    )
    expect(r!.engine).toBe(1)
    expect(r!.slots.get('titlebar')!.textContent).toContain('first')
    expect(r!.slots.size).toBe(1)
  })
})

describe('parseSkinStructure：恶意 corpus（只增不减）', () => {
  const CASES: [string, string][] = [
    ['script 标签', MALICIOUS_SCRIPT_TAG],
    ['svg 内 script', MALICIOUS_SCRIPT_IN_SVG],
    ['iframe/object/embed/link/meta/base', MALICIOUS_IFRAME_OBJECT],
    ['on* 事件属性（含大小写变体）', MALICIOUS_EVENT_HANDLERS],
    ['javascript:/vbscript:/data: URL（含混淆、危险 MIME、srcset 嵌入）', MALICIOUS_JS_URLS],
    ['iframe srcdoc', MALICIOUS_SRCDOC],
    ['槽内嵌套 template（槽逃逸）', MALICIOUS_NESTED_TEMPLATE],
    ['IE 条件注释复活', MALICIOUS_COMMENT],
  ]
  for (const [name, fixture] of CASES) {
    it(`剥除: ${name}`, () => {
      const r = parseSkinStructure(fixture)
      expect(r, '恶意包不应拒载（应清洗后可用）').not.toBeNull()
      const frag = r!.slots.get('titlebar')!
      assertNoDanger(frag)
    })
  }

  it('svg 形状保留（script 删、circle 留）', () => {
    const r = parseSkinStructure(MALICIOUS_SCRIPT_IN_SVG)!
    expect(r.slots.get('titlebar')!.querySelector('circle')).toBeTruthy()
    expect(r.slots.get('titlebar')!.querySelector('script')).toBeNull()
  })

  it('data: 栅格图片放行正控（内联图标合法用途）', () => {
    const r = parseSkinStructure(ALLOWED_DATA_URLS)!
    expect(r).not.toBeNull()
    const frag = r!.slots.get('titlebar')!
    const srcs = [...frag.querySelectorAll('img')].map((el) => el.getAttribute('src') ?? '')
    expect(srcs.filter((s) => s.startsWith('data:image/'))).toHaveLength(3)
    const srcset = frag.querySelector('img[srcset]')!.getAttribute('srcset') ?? ''
    expect(srcset).toContain('data:image/gif;base64,R0lGOD')
    expect(srcset).toContain('ok.png 2x')
  })

  it('槽逃逸尝试只删嵌套 template，外槽内容保留', () => {
    const r = parseSkinStructure(MALICIOUS_NESTED_TEMPLATE)!
    const frag = r.slots.get('titlebar')!
    expect(frag.querySelector('div')?.textContent).toContain('ok')
    expect(frag.querySelectorAll('template').length).toBe(0)
    // 嵌套槽没有注册成第二槽
    expect(r.slots.has('sidebar')).toBe(false)
  })

  it('<style> 保留（皮肤包可带结构内样式）', () => {
    const r = parseSkinStructure(
      `<template data-nb-slot="titlebar" data-nb-engine="1"><style>.x{color:red}</style><div class="x">ok</div></template>`
    )!
    expect(r.slots.get('titlebar')!.querySelector('style')).toBeTruthy()
  })
})
