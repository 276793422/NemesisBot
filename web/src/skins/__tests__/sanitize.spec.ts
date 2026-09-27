import { describe, it, expect } from 'vitest'
import { parseSkinStructure, sanitizeCssUrls } from '../sanitize'
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

describe('sanitizeCssUrls：CSS 外链零请求（与 Rust sanitize_css_urls 同 corpus）', () => {
  // —— @import 字符串形态 ——
  it('@import 外链字符串改写 about:blank（双引号）', () => {
    expect(sanitizeCssUrls('@import "https://evil.com/x.css";')).toBe(
      '@import "about:blank";',
    )
  })
  it('@import 外链字符串改写（单引号 / @IMPORT 大写 / 协议相对）', () => {
    expect(sanitizeCssUrls("@import 'http://evil.com/x.css';")).toBe(
      "@import 'about:blank';",
    )
    expect(sanitizeCssUrls('@IMPORT "https://evil.com/x.css";')).toBe(
      '@IMPORT "about:blank";',
    )
    expect(sanitizeCssUrls('@import "//evil.com/x.css";')).toBe(
      '@import "about:blank";',
    )
  })
  it('@import 合法目标原样放行；@importer 不误命中；url( 形态由 url 分支接管', () => {
    expect(sanitizeCssUrls('@import "skin/base.css";')).toBe('@import "skin/base.css";')
    expect(sanitizeCssUrls('@importer{color:red}')).toBe('@importer{color:red}')
    expect(sanitizeCssUrls('@import url(https://evil.com/x.css);')).toBe(
      '@import url(about:blank);',
    )
  })

  // —— CSS 转义现形 ——
  it('\\75rl( 转义标识符现形拦截', () => {
    expect(sanitizeCssUrls('a{background:\\75rl(http://evil.com/x.png)}')).toBe(
      'a{background:url(about:blank)}',
    )
  })
  it('u\\72 l( 中段转义现形拦截', () => {
    expect(sanitizeCssUrls('a{background:u\\72 l(http://evil.com/x.png)}')).toBe(
      'a{background:url(about:blank)}',
    )
  })
  it('url(\\68 ttp:// 目标转义现形拦截', () => {
    expect(sanitizeCssUrls('a{background:url(\\68 ttp://evil.com/x.png)}')).toBe(
      'a{background:url(about:blank)}',
    )
  })
  it('双重解码走私面封死：\\5c 75rl( 输出字面 \\ 再转义', () => {
    // 输出 \\75rl(...)：浏览器解析输出 ≡ 浏览器解析原文（都得不到 url(）
    expect(sanitizeCssUrls('a{background:\\5c 75rl(http://evil.com/x.png)}')).toBe(
      'a{background:\\\\75rl(http://evil.com/x.png)}',
    )
  })
  it('合法转义语义等价放行（content: "\\201C"）', () => {
    expect(sanitizeCssUrls('a{content:"\\201C"}')).toBe('a{content:"\u201C"}')
  })

  // —— url( 常规面 ——
  it('url( 外链（引号/裸/大小写）改写 about:blank', () => {
    expect(sanitizeCssUrls('a{background:url("https://evil.com/x.png")}')).toBe(
      'a{background:url("about:blank")}',
    )
    expect(sanitizeCssUrls('a{background:url(HTTP://EVIL.COM/x)}')).toBe(
      'a{background:url(about:blank)}',
    )
    expect(sanitizeCssUrls('a{background:url(//evil.com/x.png)}')).toBe(
      'a{background:url(about:blank)}',
    )
  })
  it('url( 合法目标放行：相对路径 + 栅格 data:', () => {
    expect(sanitizeCssUrls('a{background:url(img/logo.png)}')).toBe(
      'a{background:url(img/logo.png)}',
    )
    expect(sanitizeCssUrls('a{background:url(data:image/png;base64,iVBOR)}')).toBe(
      'a{background:url(data:image/png;base64,iVBOR)}',
    )
  })
  it('url( 非栅格 data: 拦截（svg 等）', () => {
    expect(sanitizeCssUrls('a{background:url(data:image/svg+xml;base64,PHN2Zw==)}')).toBe(
      'a{background:url(about:blank)}',
    )
  })
})

describe('sanitizeCssUrls：structure 路径接线（<style> 文本 + style 属性）', () => {
  it('<style> 元素文本内容外链 url 改写', () => {
    const r = parseSkinStructure(
      `<template data-nb-slot="titlebar" data-nb-engine="1">` +
        `<style>.x{background:url(http://evil.com/beacon.png)}@import "https://evil.com/y.css";</style>` +
        `<div class="x">ok</div></template>`
    )!
    const css = r.slots.get('titlebar')!.querySelector('style')!.textContent ?? ''
    expect(css).not.toContain('evil.com')
    expect(css).toContain('url(about:blank)')
    expect(css).toContain('@import "about:blank"')
  })

  it('style 属性值外链 url 改写', () => {
    const r = parseSkinStructure(
      `<template data-nb-slot="titlebar" data-nb-engine="1">` +
        `<div style="background:url(https://evil.com/beacon.png)">ok</div></template>`
    )!
    const style = r.slots.get('titlebar')!.querySelector('div')!.getAttribute('style') ?? ''
    expect(style).not.toContain('evil.com')
    expect(style).toContain('about:blank')
  })

  it('structure 内合法 CSS 不受损（相对路径 + 栅格 data:）', () => {
    const r = parseSkinStructure(
      `<template data-nb-slot="titlebar" data-nb-engine="1">` +
        `<style>.x{background:url(img/bg.png);mask:url(data:image/png;base64,iVBOR)}</style>` +
        `<div style="color:red">ok</div></template>`
    )!
    const css = r.slots.get('titlebar')!.querySelector('style')!.textContent ?? ''
    expect(css).toContain('url(img/bg.png)')
    expect(css).toContain('url(data:image/png;base64,iVBOR)')
  })
})
