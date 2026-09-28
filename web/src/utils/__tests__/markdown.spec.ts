// renderMarkdownHtml 单测：marked 单一渲染入口（全前端共用）。
// 关键契约：表格包 .md-table-wrap（宽表横向滚动不压列）、breaks 语义、
// 非 table 标签不受包装影响、raw HTML 消毒（2026-09-26 挂账高优 Canvas-#2）。
import { describe, expect, it } from 'vitest'
import { renderMarkdownHtml } from '../markdown'

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

  it('事件属性被剥除（img onerror 向量）', () => {
    const html = renderMarkdownHtml('<img src="x" onerror="alert(1)">')
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
})
