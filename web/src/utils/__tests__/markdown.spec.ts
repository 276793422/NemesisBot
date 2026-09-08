// renderMarkdownHtml 单测：marked 单一渲染入口（全前端共用）。
// 关键契约：表格包 .md-table-wrap（宽表横向滚动不压列）、breaks 语义、
// 非 table 标签不受包装影响。
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
})
