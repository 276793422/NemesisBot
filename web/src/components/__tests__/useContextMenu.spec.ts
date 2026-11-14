import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

// 2026-11-12 全局自制右键菜单（PLAN 234 子需求 B）：区域分发三态 + 开关
// off 直通 + 粘贴权限拒绝降级 + 撤销触发 + 互斥 closers。菜单渲染本体
// ContextMenu.vue 挂载一次，事件经 installContextMenu() 的全局 listener。

import ContextMenu from '../ContextMenu.vue'
import { installContextMenu, useContextMenu } from '../../composables/useContextMenu'

const writeTextMock = vi.fn().mockResolvedValue(undefined)
const readTextMock = vi.fn().mockResolvedValue('CLIP')
const queryPermsMock = vi.fn().mockResolvedValue({ state: 'granted' })

beforeEach(() => {
  writeTextMock.mockClear()
  readTextMock.mockClear()
  queryPermsMock.mockClear().mockResolvedValue({ state: 'granted' })
  Object.defineProperty(navigator, 'clipboard', {
    value: { writeText: writeTextMock, readText: readTextMock },
    configurable: true,
  })
  Object.defineProperty(navigator, 'permissions', {
    value: { query: queryPermsMock },
    configurable: true,
  })
  if (typeof (document as any).execCommand !== 'function') {
    ;(document as any).execCommand = vi.fn()
  }
  useContextMenu().close()
  document
    .querySelectorAll('.ctx-menu, .ctx-overlay')
    .forEach((el) => el.remove())
  localStorage.removeItem('ui.customContextMenu')
})

/** 挂渲染体 + 装全局 listener（幂等），返回菜单项文本列表读取器。 */
function setup() {
  const w = mount(ContextMenu)
  installContextMenu()
  return {
    w,
    items: () =>
      Array.from(document.querySelectorAll('.ctx-item')).map((b) => ({
        text: (b.textContent ?? '').trim(),
        disabled: (b as HTMLButtonElement).disabled,
      })),
  }
}

function ctxMenuOn(el: Element) {
  el.dispatchEvent(
    new MouseEvent('contextmenu', {
      bubbles: true,
      cancelable: true,
      clientX: 20,
      clientY: 20,
    }),
  )
}

describe('全局自定义右键菜单（区域分发/开关/降级）', () => {
  it('输入框右键 → 复制/粘贴/全选/撤销 四项；撤销触发 execCommand("undo")', async () => {
    const { w, items } = setup()
    const ta = document.createElement('textarea')
    document.body.appendChild(ta)
    ta.focus()

    ctxMenuOn(ta)
    await flushPromises()

    const names = items().map((i) => i.text)
    expect(names).toEqual(['复制', '粘贴', '全选', '撤销'])

    const execSpy = (document as any).execCommand as ReturnType<typeof vi.fn>
    const undo = Array.from(document.querySelectorAll('.ctx-item')).find((b) =>
      b.textContent?.includes('撤销'),
    )
    ;(undo as HTMLElement).click()
    await flushPromises()
    expect(execSpy).toHaveBeenCalledWith('undo')
    expect(document.querySelector('.ctx-menu')).toBeNull()
    ta.remove()
    w.unmount()
  })

  it('消息区右键（.message-content）→ 复制/全选 两项；无气泡时复制取整条减 meta', async () => {
    const { w, items } = setup()
    const wrap = document.createElement('div')
    wrap.className = 'message-content'
    wrap.textContent = '回复正文'
    document.body.appendChild(wrap)

    ctxMenuOn(wrap)
    await flushPromises()

    expect(items().map((i) => i.text)).toEqual(['复制', '全选'])
    const copy = Array.from(document.querySelectorAll('.ctx-item')).find((b) =>
      b.textContent?.includes('复制'),
    )
    ;(copy as HTMLElement).click()
    await flushPromises()
    expect(writeTextMock).toHaveBeenCalledWith('回复正文')
    wrap.remove()
    w.unmount()
  })

  // 2026-11-12 run10 活体缺陷回归锁：.message-content 的整条 textContent
  // 会把 meta 行（时间戳/模型徽章）、「⋯」按钮、📷 图片 chip 一起带进剪贴板
  // （实测 clip 多出「06:40:47test · testai-1.1 ⋯」）。复制必须只取气泡正文
  // （.message-bubble，剥 .msg-image-chip）；无气泡（压缩注记等）回落整条减
  // .message-time。
  it('消息区复制取气泡正文：meta 行/「⋯」/📷 chip 不进剪贴板', async () => {
    const { w } = setup()
    const wrap = document.createElement('div')
    wrap.className = 'message-content'
    wrap.innerHTML =
      '<div class="message-bubble">回复正文<span class="msg-image-chip">📷 2</span></div>' +
      '<div class="message-time"><span>06:40:47</span><span class="model-badge">testai-1.1</span>' +
      '<span class="msg-actions"><button class="msg-menu-trigger">⋯</button></span></div>'
    document.body.appendChild(wrap)

    ctxMenuOn(wrap)
    await flushPromises()
    const copy = Array.from(document.querySelectorAll('.ctx-item')).find((b) =>
      b.textContent?.includes('复制'),
    )
    ;(copy as HTMLElement).click()
    await flushPromises()
    expect(writeTextMock).toHaveBeenCalledWith('回复正文')
    wrap.remove()
    w.unmount()
  })

  it('其余区域右键 → 静默（preventDefault 成立、不弹菜单）', async () => {
    const { w, items } = setup()
    const ev = new MouseEvent('contextmenu', {
      bubbles: true,
      cancelable: true,
      clientX: 20,
      clientY: 20,
    })
    document.body.dispatchEvent(ev)
    await flushPromises()
    expect(ev.defaultPrevented).toBe(true)
    expect(items()).toHaveLength(0)
    w.unmount()
  })

  it('开关 off：listener 直通（不 preventDefault、无菜单）——浏览器原生菜单回归', async () => {
    localStorage.setItem('ui.customContextMenu', 'off')
    const { w, items } = setup()
    const ta = document.createElement('textarea')
    document.body.appendChild(ta)
    const ev = new MouseEvent('contextmenu', { bubbles: true, cancelable: true })
    ta.dispatchEvent(ev)
    await flushPromises()
    expect(ev.defaultPrevented).toBe(false)
    expect(items()).toHaveLength(0)
    ta.remove()
    w.unmount()
  })

  it('粘贴权限 denied → 粘贴项置灰 + title 提示（诚实降级）', async () => {
    queryPermsMock.mockResolvedValue({ state: 'denied' })
    const { w, items } = setup()
    const ta = document.createElement('textarea')
    document.body.appendChild(ta)

    ctxMenuOn(ta)
    await flushPromises()
    await flushPromises() // canPaste 异步回填 disabled 需一轮微任务

    const paste = items().find((i) => i.text === '粘贴')
    expect(paste?.disabled).toBe(true)
    const pasteEl = Array.from(document.querySelectorAll('.ctx-item')).find((b) =>
      b.textContent?.includes('粘贴'),
    ) as HTMLElement
    expect(pasteEl.title).toContain('Ctrl+V')
    ta.remove()
    w.unmount()
  })

  it('互斥 closers：右键事件到达即回调已登记的关闭函数（无论区域命中）', async () => {
    const { w } = setup()
    const closer = vi.fn()
    const unregister = useContextMenu().registerCloser(closer)
    ctxMenuOn(document.body)
    await flushPromises()
    expect(closer).toHaveBeenCalledTimes(1)
    unregister()
    ctxMenuOn(document.body)
    await flushPromises()
    expect(closer).toHaveBeenCalledTimes(1) // 注销后不再回调
    w.unmount()
  })
})

// 2026-11-13 消息区菜单项贡献位（文件链接「复制路径/打开所在目录」汇入面）：
// 贡献项追加在默认项之后（菜单底部）、空数组不出项、注销对称。
describe('消息区菜单项贡献位（registerContextMenuItems）', () => {
  /** 建一个带文件链接的消息区元素（模拟 linkify 产出形态）。 */
  function makeMessageWithLink(): { wrap: HTMLElement; a: HTMLAnchorElement } {
    const wrap = document.createElement('div')
    wrap.className = 'message-content'
    wrap.innerHTML =
      '<p>报告在 <a class="file-link" data-path="C:/ws/reports/a.md">C:/ws/reports/a.md</a> 已生成</p>'
    document.body.appendChild(wrap)
    return { wrap, a: wrap.querySelector('a')! }
  }

  it('贡献项追加在消息区菜单底部；点项回调带链接路径', async () => {
    const { w, items } = setup()
    const opener = vi.fn()
    const unregister = useContextMenu().registerContextMenuItems((t) => {
      const a = t instanceof HTMLElement ? t.closest('a.file-link') : null
      return a
        ? [
            { label: '复制路径', run: () => {} },
            { label: '打开所在目录', run: () => opener((a as HTMLAnchorElement).dataset.path) },
          ]
        : []
    })
    const { wrap, a } = makeMessageWithLink()

    ctxMenuOn(a)
    await flushPromises()
    expect(items().map((i) => i.text)).toEqual(['复制', '全选', '复制路径', '打开所在目录'])

    const open = Array.from(document.querySelectorAll('.ctx-item')).find((b) =>
      b.textContent?.includes('打开所在目录'),
    ) as HTMLElement
    open.click()
    await flushPromises()
    expect(opener).toHaveBeenCalledWith('C:/ws/reports/a.md')

    // 注销对称：再右键回落两默认项。
    unregister()
    ctxMenuOn(a)
    await flushPromises()
    expect(items().map((i) => i.text)).toEqual(['复制', '全选'])
    wrap.remove()
    w.unmount()
  })

  it('非链接目标贡献者返回空数组 → 菜单保持默认两项（openAt 空数组兜底不炸）', async () => {
    const { w, items } = setup()
    useContextMenu().registerContextMenuItems(() => [])
    const { wrap } = makeMessageWithLink()

    ctxMenuOn(wrap)
    await flushPromises()
    expect(items().map((i) => i.text)).toEqual(['复制', '全选'])
    wrap.remove()
    w.unmount()
  })
})
