/**
 * 全局自定义右键菜单（2026-11-12 子需求 B，模块化可开关）。
 *
 * 职责：全局 `contextmenu` listener 注册（App.vue 挂载一次）+ 按 e.target
 * 区域分发菜单项 + 开关状态。菜单渲染本体在 ContextMenu.vue（读本模块状态）。
 *
 * 开关真相源：`localStorage["ui.customContextMenu"]`（`"on"` 默认 / `"off"`；
 * SettingsView 可视化开关）。`off` 态：listener 直接 return（不 preventDefault）
 * ——浏览器原生菜单原样回来，零残留。未来要彻底还原浏览器行为时移除 App
 * 挂载点即可——本模块自包含，不侵入其他逻辑。
 *
 * 区域分发（handler 内按 e.target 向上最近匹配，命中即弹对应菜单并
 * preventDefault）：
 * - 输入框（textarea/input）→ 复制 / 粘贴 / 全选 / 撤销
 * - ChatPanel 消息区（.message-content）→ 复制 / 全选
 * - 其余区域 → 仅 preventDefault 不弹菜单（彻底静默）
 *
 * 与 ChatPanel 消息「⋯」菜单互斥单开：open() 会触发 registered closers
 * （ChatPanel 注册自己的关闭回调）；反向（右键打开时关「⋯」菜单）由
 * ChatPanel 侧覆盖层 @contextmenu 处理。
 *
 * 消息区菜单项贡献位（registerContextMenuItems）：组件可注册贡献函数，
 * 消息区分发命中时按返回值**追加菜单项到默认项（复制/全选）之后（底部）**
 * ——ChatPanel 用它给文件链接（a.file-link）追加「复制路径/打开所在目录」
 * （链接上下文与会话归属在组件侧，composable 保持纯 DOM 无组件依赖）。
 */
import { reactive, ref } from 'vue'

export interface ContextMenuItem {
  label: string
  title?: string
  disabled?: boolean
  run: () => void
}

export interface ContextMenuState {
  x: number
  y: number
  items: ContextMenuItem[]
}

const state = ref<ContextMenuState | null>(null)

/** 互斥单开注册表：任一方开菜单前先关别人（ChatPanel「⋯」菜单在此登记）。 */
const closers = new Set<() => void>()

/** 消息区菜单项贡献者注册表：返回的项追加在区域默认项之后（菜单底部）。 */
const contributors = new Set<(t: EventTarget | null) => ContextMenuItem[]>()

export function useContextMenu() {
  /** 开关读取（每次打开现读——SettingsView 改完立即生效，无需事件通知）。 */
  function enabled(): boolean {
    try {
      return (localStorage.getItem('ui.customContextMenu') ?? 'on') !== 'off'
    } catch {
      return true
    }
  }

  function close() {
    state.value = null
  }

  /** 弹菜单（区域分发命中后调用）。x/y 贴边收口由 ContextMenu.vue 渲染侧做。
   *  items 包一层 reactive——粘贴权限探测是异步回填（openAt 后改 disabled），
   *  非 reactive 数组的裸变更不触发重渲染。 */
  function openAt(x: number, y: number, items: ContextMenuItem[]) {
    if (!items.length) return
    state.value = reactive({ x, y, items })
  }

  /** 「⋯」等其它菜单体系的互斥登记：右键事件到达（无论区域是否命中）先
   *  回调关闭它们——右键永远压掉「⋯」菜单（互斥单开）。 */
  function registerCloser(fn: () => void): () => void {
    closers.add(fn)
    return () => closers.delete(fn)
  }

  /** 消息区菜单项贡献登记：消息区分发命中时逐个回调（传原始 e.target），
   *  返回项追加在默认项（复制/全选）之后；返回空数组 = 本次不出项。注销
   *  闭包对称（组件卸载时消费）。 */
  function registerContextMenuItems(
    fn: (t: EventTarget | null) => ContextMenuItem[],
  ): () => void {
    contributors.add(fn)
    return () => contributors.delete(fn)
  }

  return { state, enabled, close, openAt, registerCloser, registerContextMenuItems }
}

// ---------------------------------------------------------------------------
// 区域分发 + 操作实现（纯 DOM，无组件依赖）
// ---------------------------------------------------------------------------

/** e.target 向上最近匹配 textarea/input（不含只读 input——只读没有编辑菜单
 *  语义，全选/复制仍可用，但粘贴/撤销无意义；统一仍给全菜单，无效操作由
 *  浏览器自然 no-op，不猜测输入形态）。 */
function nearestEditable(t: EventTarget | null): HTMLElement | null {
  let el = t instanceof HTMLElement ? t : null
  while (el) {
    const tag = el.tagName
    if (tag === 'TEXTAREA' || tag === 'INPUT') return el
    if (el.classList.contains('message-content')) return null // 消息区优先级高于内部 input（理论上无）
    el = el.parentElement
  }
  return null
}

function nearestMessageContent(t: EventTarget | null): HTMLElement | null {
  let el = t instanceof HTMLElement ? t : null
  while (el) {
    if (el.classList.contains('message-content')) return el
    el = el.parentElement
  }
  return null
}

function selectedText(): string {
  const sel = window.getSelection()
  return sel ? sel.toString() : ''
}

/** 消息区「复制」的正文提取：气泡文本优先——.message-content 的整条
 *  textContent 会把 meta 行（时间戳/模型徽章）、「⋯」按钮、交付物卡片等
 *  外围 chrome 一起带出去（run10 实锚：clip 多出「06:40:47test ·
 *  testai-1.1 ⋯」）。气泡内图片 chip（📷 N）同为展示件，一并剥掉。
 *  无气泡的消息（压缩注记等）回落 content 减 meta 行。 */
function messageBodyText(msg: HTMLElement): string {
  const bubble = msg.querySelector('.message-bubble')
  if (bubble) {
    const clone = bubble.cloneNode(true) as HTMLElement
    clone.querySelectorAll('.msg-image-chip').forEach((el) => el.remove())
    return (clone.textContent ?? '').trim()
  }
  const clone = msg.cloneNode(true) as HTMLElement
  clone.querySelector('.message-time')?.remove()
  return (clone.textContent ?? '').trim()
}

/** 复制：选中文本 ?? 目标文本（输入框 = 当前值；消息区 = 气泡正文，
 *  见 messageBodyText）。写剪贴板宽松无权限面；失败走 catch 诚实 no-op
 *  （菜单已关，不打断）。 */
async function copyText(text: string) {
  if (!text) return
  try {
    await navigator.clipboard.writeText(text)
  } catch {
    // 剪贴板不可用（非安全上下文/宿主拒绝）——静默放弃，浏览器菜单兜底。
  }
}

/** 粘贴能力探测（菜单渲染前置灰用）：clipboard.readText 在无权限宿主里
 *  会 reject——这里只探测可用性不真的读（读到的内容插入才是 read 的职责，
 *  真插入在 run 时再做一次并处理拒绝）。 */
async function canPaste(): Promise<boolean> {
  try {
    const perms = (navigator as Navigator & { permissions?: Permissions }).permissions
    if (!perms?.query) return true // 无 permissions API（老 WebView）——乐观可用，run 时降级
    const st = await perms.query({ name: 'clipboard-read' as PermissionName })
    return st.state !== 'denied'
  } catch {
    return true // 查询本身不支持 = 未知，乐观可用（Chromium 默认 prompt）
  }
}

async function pasteInto(el: HTMLTextAreaElement | HTMLInputElement) {
  let text: string
  try {
    text = await navigator.clipboard.readText()
  } catch {
    return // 权限拒绝——菜单项已灰，run 期仍拒绝 = 静默放弃（Ctrl+V 兜底）
  }
  const start = el.selectionStart ?? el.value.length
  const end = el.selectionEnd ?? el.value.length
  const val = el.value
  el.value = val.slice(0, start) + text + val.slice(end)
  const pos = start + text.length
  el.focus()
  el.setSelectionRange(pos, pos)
  el.dispatchEvent(new Event('input', { bubbles: true }))
}

// ---------------------------------------------------------------------------
// App.vue 挂载的一次性全局 listener
// ---------------------------------------------------------------------------

let installed = false

export function installContextMenu(): void {
  if (installed) return
  installed = true
  window.addEventListener(
    'contextmenu',
    (e: MouseEvent) => {
      const { enabled, openAt, close } = useContextMenu()
      // off 态：直接 return，不 preventDefault——浏览器原生菜单原样回来。
      if (!enabled()) return
      e.preventDefault()
      // 互斥单开：右键永远压掉「⋯」菜单（无论本区域是否命中分发）。
      close()
      for (const c of closers) c()

      const editable = nearestEditable(e.target)
      if (editable) {
        const items: ContextMenuItem[] = [
          {
            label: '复制',
            disabled: !selectedText() && !('value' in editable && editable.value),
            run: () =>
              void copyText(
                selectedText() ||
                  ('value' in editable ? String(editable.value) : ''),
              ),
          },
          {
            label: '粘贴',
            title: '权限受限时请用 Ctrl+V',
            run: () => void pasteInto(editable as HTMLTextAreaElement),
          },
          {
            label: '全选',
            run: () => {
              ;(editable as HTMLTextAreaElement).focus()
              ;(editable as HTMLTextAreaElement).select()
            },
          },
          {
            label: '撤销',
            title: '输入框内撤销（与 Ctrl+Z 同源）',
            run: () => {
              editable.focus()
              document.execCommand('undo')
            },
          },
        ]
        // 粘贴权限拒绝 → 前置灰该项 + title 提示（诚实降级，不假装可用）。
        void canPaste().then((ok) => {
          if (!ok) {
            const paste = items.find((i) => i.label === '粘贴')
            if (paste) {
              paste.disabled = true
              paste.title = '剪贴板权限受限，请用 Ctrl+V'
            }
          }
        })
        openAt(e.clientX, e.clientY, items)
        return
      }

      const msg = nearestMessageContent(e.target)
      if (msg) {
        const items: ContextMenuItem[] = [
          {
            label: '复制',
            disabled: !selectedText() && !messageBodyText(msg),
            run: () => void copyText(selectedText() || messageBodyText(msg)),
          },
          {
            label: '全选',
            run: () => {
              const range = document.createRange()
              range.selectNodeContents(msg)
              const sel = window.getSelection()
              sel?.removeAllRanges()
              sel?.addRange(range)
            },
          },
        ]
        // 贡献项追加在默认项之后（菜单底部）——文件链接「复制路径/打开
        // 所在目录」等组件侧扩展在此汇入。
        for (const c of contributors) items.push(...c(e.target))
        openAt(e.clientX, e.clientY, items)
      }
      // 其余区域：已 preventDefault，不弹菜单（彻底静默）。
    },
    { capture: false },
  )
}
