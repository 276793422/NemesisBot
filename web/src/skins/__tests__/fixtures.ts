/**
 * 引擎测试 fixtures：合法结构样例 + 恶意 corpus（**只增不减**——每次
 * 发现新绕过手法必须在这里补用例钉死，sanitize 回归 = 安全事件）。
 */

/** 四槽最小合法结构（engine v1 声明在根 template 上）。 */
export const STRUCTURE_MINIMAL = `
<!-- bot 皮肤结构（示例） -->
<template data-nb-slot="titlebar" data-nb-engine="1">
  <div class="bar"><span data-nb-bind="brand"></span></div>
</template>
<template data-nb-slot="sidebar">
  <aside class="sb">
    <button data-nb-action="new-chat">新建</button>
    <nav>
      <button data-nb-for="n in navPrimary" data-nb-action="route:{n.path}">
        <span data-nb-bind="n.label"></span>
      </button>
    </nav>
  </aside>
</template>
`

/** engine 版本高于引擎支持 → 整包拒载。 */
export const STRUCTURE_ENGINE_TOO_NEW = `
<template data-nb-slot="titlebar" data-nb-engine="99"><div></div></template>
`

/** 无槽 → 拒载（null）。 */
export const STRUCTURE_NO_SLOTS = `<div><p>什么都没有</p></div>`

// ---- 恶意 corpus：每条 = 一种攻击手法，断言清洗后无残留 ----

export const MALICIOUS_SCRIPT_TAG = `
<template data-nb-slot="titlebar" data-nb-engine="1">
  <div>ok<script>window.__pwned = 1</script></div>
</template>
`

export const MALICIOUS_SCRIPT_IN_SVG = `
<template data-nb-slot="titlebar" data-nb-engine="1">
  <svg><script>window.__pwned = 1</script><circle r="4"/></svg>
</template>
`

export const MALICIOUS_IFRAME_OBJECT = `
<template data-nb-slot="titlebar" data-nb-engine="1">
  <div>
    <iframe src="https://evil.example"></iframe>
    <object data="https://evil.example"></object>
    <embed src="https://evil.example">
    <link rel="stylesheet" href="https://evil.example/x.css">
    <meta http-equiv="refresh" content="0;url=https://evil.example">
    <base href="https://evil.example/">
  </div>
</template>
`

export const MALICIOUS_EVENT_HANDLERS = `
<template data-nb-slot="titlebar" data-nb-engine="1">
  <div onmouseover="window.__pwned=1" onerror="x" ONCLICK="window.__pwned=1" onload="y">ok</div>
</template>
`

export const MALICIOUS_JS_URLS = `
<template data-nb-slot="titlebar" data-nb-engine="1">
  <a href="javascript:window.__pwned=1">a</a>
  <a href="  JaVaScRiPt:window.__pwned=1">b</a>
  <img src="vbscript:x">
  <svg><a xlink:href="javascript:window.__pwned=1"><text>y</text></a></svg>
  <form action="javascript:window.__pwned=1"><button formaction="javascript:window.__pwned=1">go</button></form>
  <a href="data:text/html,<script>window.__pwned=1</script>">d1</a>
  <a href="DATA:IMAGE/SVG+XML;base64,PHN2Zz4=">d2</a>
  <img src="data:application/x-httpd-php,x">
  <img srcset="ok.png 1x, javascript:window.__pwned=1 2x" src="ok.png">
  <img srcset="data:text/html;base64,PHN2Zz4= 1x" src="ok.png">
</template>
`

/** data: URL 放行正控：栅格图片 MIME 必须保留（内联图标合法用途）。 */
export const ALLOWED_DATA_URLS = `
<template data-nb-slot="titlebar" data-nb-engine="1">
  <img src="data:image/png;base64,iVBORw0KGgo=" alt="p">
  <img src="data:image/jpeg;base64,/9j/4AAQ" alt="j">
  <img src="data:image/webp;base64,UklGR" alt="w">
  <img srcset="data:image/gif;base64,R0lGOD 1x, ok.png 2x" src="ok.png">
</template>
`

export const MALICIOUS_SRCDOC = `
<template data-nb-slot="titlebar" data-nb-engine="1">
  <iframe srcdoc="&lt;script&gt;window.__pwned=1&lt;/script&gt;"></iframe>
</template>
`

/** 槽内嵌套 template = 槽逃逸尝试（嵌套 template 删，外槽保留）。 */
export const MALICIOUS_NESTED_TEMPLATE = `
<template data-nb-slot="titlebar" data-nb-engine="1">
  <div>ok</div>
  <template data-nb-slot="sidebar"><script>window.__pwned=1</script></template>
</template>
`

/** 注释复活尝试（IE 条件注释形态）。 */
export const MALICIOUS_COMMENT = `
<template data-nb-slot="titlebar" data-nb-engine="1">
  <!--[if IE]><script>window.__pwned=1</script><![endif]-->
  <div>ok</div>
</template>
`
