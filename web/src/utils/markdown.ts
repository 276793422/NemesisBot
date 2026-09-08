// marked 单一渲染入口（全前端共用）：所有 markdown-body 输出同源，
// 表格统一包一层 .md-table-wrap——宽表横向滚动而非把单元格压成每字一行
// （真实表格盒 width:max-content 拿 intrinsic 宽度；匿名盒方案无效，
// 详见 components.css .md-table-wrap 注释）。
import { marked } from 'marked'

export interface MarkdownRenderOptions {
  /** 换行语义：聊天=true（单换行成 <br>），文档页=false（标准 GFM）。 */
  breaks?: boolean
}

export function renderMarkdownHtml(text: string, opts?: MarkdownRenderOptions): string {
  // 代码高亮走事后 DOM 路径（渲染后对 pre code 跑 hljs.highlightElement，
  // 见 ChatPanel renderCodeBlocks）——marked v15 已移除 highlight 选项，
  // 此处不收该参数。
  const html = (marked as any).parse(text, {
    gfm: true,
    breaks: opts?.breaks ?? false,
  }) as string
  // marked 输出的表格标签无属性、形态稳定——就地包装为滚动容器。
  return html
    .replace(/<table>/g, '<div class="md-table-wrap"><table>')
    .replace(/<\/table>/g, '</table></div>')
}
