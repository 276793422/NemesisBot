// marked 单一渲染入口（全前端共用）：所有 markdown-body 输出同源，
// 表格统一包一层 .md-table-wrap——宽表横向滚动而非把单元格压成每字一行
// （真实表格盒 width:max-content 拿 intrinsic 宽度；匿名盒方案无效，
// 详见 components.css .md-table-wrap 注释）。
import DOMPurify from 'dompurify'
import { marked } from 'marked'

export interface MarkdownRenderOptions {
  /** 换行语义：聊天=true（单换行成 <br>），文档页=false（标准 GFM）。 */
  breaks?: boolean
}

export function renderMarkdownHtml(text: string, opts?: MarkdownRenderOptions): string {
  // 代码高亮走事后 DOM 路径（渲染后对 pre code 跑 hljs.highlightElement，
  // 见 ChatPanel renderCodeBlocks）——marked v15 已移除 highlight 选项，
  // 此处不收该参数。
  const raw = (marked as any).parse(text, {
    gfm: true,
    breaks: opts?.breaks ?? false,
  }) as string
  // marked 默认放行 raw HTML——模型输出（或被注入后的输出）混入
  // <script>/<img onerror> 会被当真渲染。此处是全部 6 个 v-html 渲染面的
  // 唯一消毒闸（2026-09-26 复检挂账高优 Canvas-#2）；hljs 高亮是消毒后
  // 的 DOM 事后增强，不受影响。style 属性整体禁掉：模型内容不需要内联
  // 样式，禁掉即消灭整类 CSS 注入向量（DOMPurify 默认只净化不剥除）。
  const html = DOMPurify.sanitize(raw, { FORBID_ATTR: ['style'] })
  // marked 输出的表格标签无属性、形态稳定——就地包装为滚动容器。
  return html
    .replace(/<table>/g, '<div class="md-table-wrap"><table>')
    .replace(/<\/table>/g, '</table></div>')
}
