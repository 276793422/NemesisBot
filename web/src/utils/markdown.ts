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

/**
 * HTML 转义（&<>"' 全量）：v-html 渲染面在异常降级路径上的兜底原语。
 * renderMarkdownHtml 是唯一消毒闸，但它依赖 marked/DOMPurify 正常工作——
 * 调用方 catch 回落时**不得**把原文直接喂 v-html（等于绕过消毒闸），
 * 必须先过本函数把活动内容降级为纯文本再进 HTML 上下文。
 */
export function escapeHtml(s: string): string {
  if (!s) return ''
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}

/**
 * HTML 片段消毒（渲染闸与皮肤引擎 data-nb-html 共用的单一配置真相源）。
 * DOMPurify 默认净化活动内容（script / on* 事件属性 / javascript: URI）；
 * style 属性整体禁掉——模型/皮肤内容不需要内联样式，禁掉即消灭整类 CSS
 * 注入向量（DOMPurify 默认只净化不剥除）。对已消毒 HTML 幂等。
 */
export function sanitizeHtmlFragment(html: string): string {
  return DOMPurify.sanitize(html, { FORBID_ATTR: ['style'] })
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
  // <script>/<img onerror> 会被当真渲染。此处是全部 v-html 渲染面的
  // 唯一消毒闸（2026-09-26 复检挂账高优 Canvas-#2）；hljs 高亮是消毒后
  // 的 DOM 事后增强，不受影响。消毒配置单一真相源见 sanitizeHtmlFragment
  // （皮肤引擎 data-nb-html 同源消费）。
  const html = sanitizeHtmlFragment(raw)
  // marked 输出的表格标签无属性、形态稳定——就地包装为滚动容器。
  const linked = linkifyFilePaths(html)
  return linked
    .replace(/<table>/g, '<div class="md-table-wrap"><table>')
    .replace(/<\/table>/g, '</table></div>')
}

// ---------------------------------------------------------------------------
// 文件路径 linkify（2026-10-19 用户报告问题 2：模型输出 markdown 里的文件
// 路径在 UI 上不可点击，用户得手动翻目录找文件）。
//
// 安全面：入口是**消毒后**的 DOM——本函数只把纯文本节点中的路径 token 包成
// `<a class="file-link" data-path>`（textContent 赋值，无 innerHTML 注入面），
// 跳过已有 <a> 与代码块 <pre>。点击行为由 ChatPanel 事件委托调 WSAPI
// `fs.open`，后端做 containment + 扩展名白名单（防「模型写 evil.bat → 聊天
// 链接 → 点击执行」链——非白名单类型降级为文件管理器定位，不执行）。
// ---------------------------------------------------------------------------

/// 白名单扩展名（与后端 fs.rs OPENABLE_EXTENSIONS 口径一致；裸文件名与
/// 带目录的相对路径必须命中白名单才成链，压普通英文句子的误链面）。
const LINKABLE_EXTS = 'md|txt|log|json|csv|pdf|png|jpg|jpeg|gif|webp|bmp'

/// 相对路径段字符：ASCII 词字符 + CJK 统一表意（含扩展 A）+ `@` `.` `-`
/// （连字符置尾免成区间）。中文文件名/目录名（`docs/总结.md`）是本环境
/// 常态——JS `\w` 只认 ASCII，不补 CJK 则中文相对路径永远不成链。
const SEG = `[\\w\\u3400-\\u4dbf\\u4e00-\\u9fff@.-]`

/// 路径 token 三形态合一：
/// 1. Windows 绝对路径（盘符 + 根 + 非空白段——无扩展名也收，点击开目录；
///    `[^\s...]` 天然收 CJK）；
/// 2. 带目录的相对路径（`src/notes.md`、`./reports/x.log`、`docs/总结.md`）；
/// 3. 裸文件名（`README.md`——必须命中白名单扩展名；**保持 ASCII 段类**：
///    收 CJK 会把「这是README.md」的「这是」吞进路径，反而退化）。
///   （`*` 不进字符类，markdown 强调 `**x**` 不会被吞。）
const PATH_TOKEN_RE = new RegExp(
  `[A-Za-z]:[\\\\/][^\\s<>\\\`"']*` +
    `|(?:${SEG}+[\\\\/])+${SEG}+` +
    `|[\\w@.-]+\\.(?:${LINKABLE_EXTS})\\b`,
  'g',
)
/// 无 g 副本（快速预判——/g 正则 .test 有 lastIndex 状态，不能复用）。
const PATH_TOKEN_TEST_RE = new RegExp(PATH_TOKEN_RE.source)

/// 尾部标点裁剪：贪婪匹配会把句尾 `。` `）` `)` 等吞进 token，成链前剥掉。
const TRAILING_PUNCT_RE = /[.,;:!?)\]}>'"`、。，；：！？）】》」』]+$/

function trimToken(raw: string): string {
  const trimmed = raw.replace(TRAILING_PUNCT_RE, '')
  // Windows 绝对路径至少要有一段内容（`C:\` 纯盘根不成链）。
  if (/^[A-Za-z]:[\\/]/.test(trimmed) && trimmed.length <= 3) return ''
  return trimmed
}

/** 判定 token 是否值得成链：绝对路径直接过；相对/裸名必须白名单扩展名。 */
function isLinkablePath(tok: string): boolean {
  if (!tok) return false
  if (/^[A-Za-z]:[\\/]/.test(tok)) return true
  const m = tok.match(new RegExp(`\\.(?:${LINKABLE_EXTS})$`, 'i'))
  return m !== null
}

/** 单个文本节点内的路径 token → <a class="file-link"> 序列替换。 */
function replacePathTokensIn(node: Text): void {
  const text = node.data
  const frag = document.createDocumentFragment()
  let idx = 0
  let changed = false
  for (const m of text.matchAll(PATH_TOKEN_RE)) {
    const tok = trimToken(m[0])
    if (!isLinkablePath(tok)) continue
    const at = m.index ?? 0
    frag.append(document.createTextNode(text.slice(idx, at)))
    const a = document.createElement('a')
    a.className = 'file-link'
    a.dataset.path = tok
    a.textContent = tok
    a.title = '打开文件'
    frag.append(a)
    // 被裁掉的尾部标点还原为普通文本。
    const rest = m[0].slice(tok.length)
    if (rest) frag.append(document.createTextNode(rest))
    idx = at + m[0].length
    changed = true
  }
  if (!changed) return
  frag.append(document.createTextNode(text.slice(idx)))
  node.replaceWith(frag)
}

/**
 * 消毒后 HTML 的文件路径 linkify（sanitize 之后、v-html 之前调用）。
 * 只处理文本节点；已有 <a> 内与代码块 <pre> 内跳过（inline <code> 参与——
 * 模型常用行内代码包路径）。
 */
export function linkifyFilePaths(html: string): string {
  if (!PATH_TOKEN_TEST_RE.test(html)) return html
  const holder = document.createElement('div')
  holder.innerHTML = html // 已过 DOMPurify，安全
  const walker = document.createTreeWalker(holder, NodeFilter.SHOW_TEXT)
  const targets: Text[] = []
  while (walker.nextNode()) {
    const t = walker.currentNode as Text
    const parent = t.parentElement
    if (!parent || parent.closest('a, pre')) continue
    if (PATH_TOKEN_TEST_RE.test(t.data)) targets.push(t)
  }
  for (const t of targets) replacePathTokensIn(t)
  return holder.innerHTML
}
