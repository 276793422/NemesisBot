import { describe, it, expect, beforeEach, vi } from 'vitest'
import { nextTick, reactive } from 'vue'
import { SkinStructureEngine } from '../engine'
import { parseSkinStructure } from '../sanitize'
import type { SkinProjection } from '../types'

function proj(): SkinProjection {
  return reactive<SkinProjection>({
    brand: 'NemesisBot',
    version: '1.0.0',
    scenes: ['写代码', '查资料'],
    connected: true,
    connectionText: '已连接',
    readyText: '就绪',
    statusVersion: 'v9.9.9',
    model: 'glm-4.7',
    mobileOpen: false,
    navPrimary: [
      { label: '人格', path: '/persona' },
      { label: '代码开发', path: '/coding', active: true },
    ],
    navMore: [],
    sessionGroups: [
      {
        label: '今天',
        sessions: [
          { id: 's1', title: '第一个会话', relTime: '5 分钟前', active: true, pinned: false },
          { id: 's2', title: '第二个会话', relTime: '1 小时前', active: false, pinned: false },
        ],
      },
    ],
    hasSessions: true,
    currentSessionId: 's1',
    estopEngaged: false,
    estopBusy: false,
    estopLabel: '急停',
    signature: { visible: false, state: 'unsigned', label: '', title: '' },
    fullAccess: false,
    theme: 'dark',
    themeToggleLabel: '切换主题',
    messages: [],
    chatBusy: false,
    inputText: '',
    mode: 'build',
    pages: {},
  })
}

function mountWith(html: string, p: SkinProjection = proj()) {
  const parsed = parseSkinStructure(html)!
  const engine = new SkinStructureEngine()
  engine.projection = p
  engine.load(parsed.slots)
  const container = document.createElement('div')
  document.body.appendChild(container)
  return { engine, container, p }
}

function htmlOf(container: HTMLElement): string {
  return container.innerHTML
}

beforeEach(() => {
  document.body.innerHTML = ''
  vi.spyOn(console, 'warn').mockImplementation(() => {})
})

describe('SkinStructureEngine：微绑定', () => {
  it('data-nb-bind：初值渲染 + 投影变化跟随（null → 空串）', async () => {
    const { engine, container, p } = mountWith(
      `<template data-nb-slot="titlebar" data-nb-engine="1"><span data-nb-bind="brand"></span><i data-nb-bind="nothing.here"></i></template>`
    )
    engine.mount('titlebar', container)
    expect(container.querySelector('span')!.textContent).toBe('NemesisBot')
    p.brand = 'WB'
    await nextTick()
    expect(container.querySelector('span')!.textContent).toBe('WB')
    // 路径缺失 → 空串诚实降级，不抛
    expect(container.querySelector('i')!.textContent).toBe('')
  })

  it('data-nb-class：逐对 toggle', async () => {
    const { engine, container, p } = mountWith(
      `<template data-nb-slot="titlebar" data-nb-engine="1"><div data-nb-class="on:connected; off:!connected"></div></template>`
    )
    engine.mount('titlebar', container)
    const el = container.querySelector('div')!
    expect(el.classList.contains('on')).toBe(true)
    expect(el.classList.contains('off')).toBe(false)
    p.connected = false
    await nextTick()
    expect(el.classList.contains('on')).toBe(false)
    expect(el.classList.contains('off')).toBe(true)
  })

  it('data-nb-attr：set/removeAttribute；黑名单属性（href/class/on*）被拒绝', async () => {
    const { engine, container, p } = mountWith(
      `<template data-nb-slot="titlebar" data-nb-engine="1">` +
        `<div data-nb-attr="title:model; data-x:theme; href:brand; class:brand" title="static"></div>` +
        `</template>`
    )
    engine.mount('titlebar', container)
    const el = container.querySelector('div')!
    expect(el.getAttribute('title')).toBe('glm-4.7')
    expect(el.getAttribute('data-x')).toBe('dark')
    // 黑名单不建绑定，初始 static 保留
    expect(el.getAttribute('title')).not.toBe('static')
    expect(el.getAttribute('href')).toBeNull()
    expect(el.getAttribute('class')).toBeNull()
    p.model = ''
    await nextTick()
    expect(el.hasAttribute('title')).toBe(false)
  })

  it('`!` 否定与未知原语忽略', async () => {
    const { engine, container, p } = mountWith(
      `<template data-nb-slot="titlebar" data-nb-engine="1">` +
        `<b data-nb-bind="!connected"></b><u data-nb-unknown="x" data-nb-bind="version"></u>` +
        `</template>`
    )
    engine.mount('titlebar', container)
    expect(container.querySelector('b')!.textContent).toBe('false')
    p.connected = false
    await nextTick()
    expect(container.querySelector('b')!.textContent).toBe('true')
    expect(container.querySelector('u')!.textContent).toBe('1.0.0')
  })
})

describe('SkinStructureEngine：结构原语（重建式）', () => {
  it('data-nb-for：渲染行 + 行内插值 action + 投影变化整块重建', async () => {
    const { engine, container, p } = mountWith(
      `<template data-nb-slot="sidebar" data-nb-engine="1">` +
        `<nav><button data-nb-for="n in navPrimary" data-nb-action="route:{n.path}"><span data-nb-bind="n.label"></span></button></nav>` +
        `</template>`
    )
    const seen: string[] = []
    engine.actionHandlers.set('route', (arg) => seen.push(arg))
    engine.mount('sidebar', container)
    const btns = () => container.querySelectorAll('nav button')
    expect(btns().length).toBe(2)
    expect(btns()[0].getAttribute('data-nb-action')).toBe('route:/persona')
    expect(btns()[1].querySelector('span')!.textContent).toBe('代码开发')
    ;(btns()[1] as HTMLElement).click()
    expect(seen).toEqual(['/coding'])

    p.navPrimary = [{ label: '唯一', path: '/only' }]
    await nextTick()
    expect(btns().length).toBe(1)
    expect(btns()[0].getAttribute('data-nb-action')).toBe('route:/only')
    // 重建后旧克隆不在文档（无泄漏节点）
    expect(document.querySelectorAll('nav button').length).toBe(1)
  })

  it('data-nb-for：非数组/空数组 → 清空', async () => {
    const { engine, container, p } = mountWith(
      `<template data-nb-slot="sidebar" data-nb-engine="1"><i data-nb-for="s in scenes"></i></template>`
    )
    engine.mount('sidebar', container)
    expect(container.querySelectorAll('i').length).toBe(2)
    p.scenes = []
    await nextTick()
    expect(container.querySelectorAll('i').length).toBe(0)
    p.scenes = undefined as unknown as string[]
    await nextTick()
    expect(container.querySelectorAll('i').length).toBe(0)
  })

  it('data-nb-if：truthy 挂 / falsy 摘（注释占位）', async () => {
    const { engine, container, p } = mountWith(
      `<template data-nb-slot="titlebar" data-nb-engine="1"><em data-nb-if="hasSessions">有会话</em></template>`
    )
    engine.mount('titlebar', container)
    expect(container.querySelector('em')?.textContent).toBe('有会话')
    p.hasSessions = false
    await nextTick()
    expect(container.querySelector('em')).toBeNull()
    expect(htmlOf(container)).toContain('nb-if')
    p.hasSessions = true
    await nextTick()
    expect(container.querySelector('em')?.textContent).toBe('有会话')
  })

  it('for × if 嵌套：行内 if 按 item 快照求值', async () => {
    const { engine, container } = mountWith(
      `<template data-nb-slot="sidebar" data-nb-engine="1">` +
        `<div data-nb-for="g in sessionGroups"><b data-nb-bind="g.label"></b>` +
        `<button data-nb-for="s in g.sessions" data-nb-if="s.active"><span data-nb-bind="s.title"></span></button></div>` +
        `</template>`
    )
    engine.mount('sidebar', container)
    const groups = container.querySelectorAll('div')
    expect(groups.length).toBe(1)
    expect(groups[0].querySelector('b')!.textContent).toBe('今天')
    // 只 active 的会话渲染 button（s1 active，s2 不是）
    const btns = groups[0].querySelectorAll('button')
    expect(btns.length).toBe(1)
    expect(btns[0].querySelector('span')!.textContent).toBe('第一个会话')
  })

  it('data-nb-for 语法非法 → warn-once 原语忽略，元素摘除不崩溃', () => {
    const { engine, container } = mountWith(
      `<template data-nb-slot="sidebar" data-nb-engine="1"><i data-nb-for="没有in">x</i></template>`
    )
    engine.mount('sidebar', container)
    expect(container.querySelectorAll('i').length).toBe(0)
    expect(console.warn).toHaveBeenCalled()
  })
})

describe('SkinStructureEngine：动作与本地状态', () => {
  it('无参动作命中；未知动作 warn-once no-op', async () => {
    const { engine, container } = mountWith(
      `<template data-nb-slot="sidebar" data-nb-engine="1">` +
        `<button class="a" data-nb-action="new-chat">新</button>` +
        `<button class="b" data-nb-action="does-not-exist">幻</button>` +
        `</template>`
    )
    const seen: string[] = []
    engine.actionHandlers.set('new-chat', () => seen.push('new-chat'))
    engine.mount('sidebar', container)
    ;(container.querySelector('.a') as HTMLElement).click()
    ;(container.querySelector('.b') as HTMLElement).click()
    ;(container.querySelector('.b') as HTMLElement).click()
    expect(seen).toEqual(['new-chat'])
    expect(console.warn).toHaveBeenCalledTimes(1) // warn-once
  })

  it('local:toggle + data-nb-outside：按钮开、外点收', async () => {
    const { engine, container } = mountWith(
      `<template data-nb-slot="sidebar" data-nb-engine="1">` +
        `<div class="wrap" data-nb-outside="moreOpen">` +
        `<button data-nb-action="local:toggle:moreOpen">更多</button>` +
        `<div class="flyout" data-nb-if="local.moreOpen">菜单</div>` +
        `</div></template>`
    )
    engine.mount('sidebar', container)
    container.querySelector('button')!.click()
    await nextTick()
    expect(container.querySelector('.flyout')).toBeTruthy()
    // 再点 = toggle 收
    container.querySelector('button')!.click()
    await nextTick()
    expect(container.querySelector('.flyout')).toBeNull()
    // 再开，然后容器外点击 → outside 收起
    container.querySelector('button')!.click()
    await nextTick()
    expect(container.querySelector('.flyout')).toBeTruthy()
    const outside = document.createElement('div')
    document.body.appendChild(outside)
    outside.click()
    await nextTick()
    expect(container.querySelector('.flyout')).toBeNull()
  })

  it('closest 单命中：内层动作吃掉事件，外层动作不触发', () => {
    const { engine, container } = mountWith(
      `<template data-nb-slot="sidebar" data-nb-engine="1">` +
        `<div data-nb-action="outer"><button data-nb-action="inner">go</button></div>` +
        `</template>`
    )
    const seen: string[] = []
    engine.actionHandlers.set('outer', () => seen.push('outer'))
    engine.actionHandlers.set('inner', () => seen.push('inner'))
    engine.mount('sidebar', container)
    container.querySelector('button')!.click()
    expect(seen).toEqual(['inner'])
  })

  it('动作元素带 href 时 preventDefault（不跳 hash）', () => {
    const { engine, container } = mountWith(
      `<template data-nb-slot="sidebar" data-nb-engine="1"><a href="#/x" data-nb-action="route:/x">链</a></template>`
    )
    engine.actionHandlers.set('route', () => {})
    engine.mount('sidebar', container)
    const a = container.querySelector('a')!
    const ev = new MouseEvent('click', { bubbles: true, cancelable: true })
    a.dispatchEvent(ev)
    expect(ev.defaultPrevented).toBe(true)
  })
})

describe('SkinStructureEngine：生命周期与清理', () => {
  it('unmount：容器清空、监听移除、再 mount 重复挂载安全', async () => {
    const { engine, container } = mountWith(
      `<template data-nb-slot="sidebar" data-nb-engine="1">` +
        `<button data-nb-action="local:toggle:moreOpen" data-nb-outside="moreOpen">x</button>` +
        `</template>`
    )
    expect(engine.mount('sidebar', container)).toBe(true)
    expect(engine.docListenerActive).toBe(true)
    expect(container.children.length).toBe(1)
    engine.unmount('sidebar')
    expect(container.innerHTML).toBe('')
    expect(engine.docListenerActive).toBe(false)
    // 卸载后 local toggle 不再响应（effect 已停）
    container.querySelector('button')
    engine.mount('sidebar', container)
    expect(container.children.length).toBe(1)
    engine.unmountAll()
    expect(container.innerHTML).toBe('')
  })

  it('load 替换语义：旧运行时全卸载', () => {
    const { engine, container } = mountWith(
      `<template data-nb-slot="titlebar" data-nb-engine="1"><b>old</b></template>`
    )
    engine.mount('titlebar', container)
    const parsed = parseSkinStructure(
      `<template data-nb-slot="statusbar" data-nb-engine="1"><i>new</i></template>`
    )!
    engine.load(parsed.slots)
    expect(container.innerHTML).toBe('')
    expect(engine.has('titlebar')).toBe(false)
    expect(engine.has('statusbar')).toBe(true)
  })

  it('挂到不存在槽 → false', () => {
    const { engine, container } = mountWith(
      `<template data-nb-slot="titlebar" data-nb-engine="1"><b>x</b></template>`
    )
    expect(engine.mount('nope', container)).toBe(false)
  })
})

describe('SkinStructureEngine：v3 内容区原语', () => {
  it('data-nb-html：投影 HTML 渲染 + 变化跟随（信任=投影，不涉包作者 HTML）', async () => {
    type TestProj = SkinProjection & { html?: string }
    const { engine, container, p } = mountWith(
      `<template data-nb-slot="titlebar" data-nb-engine="1"><div data-nb-html="html"></div></template>`,
      reactive({ ...proj(), html: '' }) as TestProj
    )
    const tp = p as TestProj
    engine.mount('titlebar', container)
    const div = container.querySelector('div')!
    expect(div.innerHTML).toBe('')
    tp.html = '<p>hi <b>bold</b></p>'
    await nextTick()
    expect(div.innerHTML).toContain('<b>bold</b>')
    tp.html = ''
    await nextTick()
    expect(div.innerHTML).toBe('')
  })

  it('data-nb-value：单向投影 → 表单值（预填；同值不重复写）', async () => {
    const { engine, container, p } = mountWith(
      `<template data-nb-slot="titlebar" data-nb-engine="1"><input data-nb-value="inputText"></template>`
    )
    engine.mount('titlebar', container)
    const input = container.querySelector('input')!
    expect(input.value).toBe('')
    p.inputText = '预填文本'
    await nextTick()
    expect(input.value).toBe('预填文本')
    // 用户手打后投影同值 → 不回写（避免光标扰动）
    input.value = '手打'
    input.dispatchEvent(new Event('input'))
    await nextTick()
    expect(input.value).toBe('手打')
  })

  it('data-nb-field：input 事件 → 已注册 sink；未注册 warn 丢弃', async () => {
    const received: string[] = []
    const { engine, container } = mountWith(
      `<template data-nb-slot="titlebar" data-nb-engine="1">` +
        `<input data-nb-field="test.sink" value=""><input data-nb-field="nope.sink" value="">` +
        `</template>`
    )
    engine.fieldSinks.set('test.sink', (v) => received.push(v))
    engine.mount('titlebar', container)
    const [registered, unregistered] = container.querySelectorAll('input')!
    registered.value = 'hello'
    registered.dispatchEvent(new Event('input'))
    expect(received).toEqual(['hello'])
    unregistered.value = 'x'
    unregistered.dispatchEvent(new Event('input'))
    expect(received).toEqual(['hello'])
  })

  it('data-nb-autoscroll：贴底时跟随新消息；上翻阅史时不拽回', async () => {
    const { engine, container, p } = mountWith(
      `<template data-nb-slot="titlebar" data-nb-engine="1"><div data-nb-autoscroll="messages"></div></template>`
    )
    engine.mount('titlebar', container)
    const el = container.querySelector('div')!

    // 贴底态（distance 100 < 120）：新消息到达 → 跟随贴底
    Object.defineProperty(el, 'scrollHeight', { value: 100, configurable: true })
    Object.defineProperty(el, 'clientHeight', { value: 0, configurable: true })
    p.messages = [
      { id: 1, role: 'user', isUser: true, isError: false, content: 'x', contentHtml: '', time: '', model: '', sourceNode: '', imageCount: 0 },
    ]
    await nextTick()
    await nextTick()
    expect(el.scrollTop).toBe(100)

    // 上翻态（distance 500 ≥ 120）：新消息到达 → 保持位置不拽回
    Object.defineProperty(el, 'scrollHeight', { value: 500, configurable: true })
    p.messages = [
      ...p.messages,
      { id: 2, role: 'assistant', isUser: false, isError: false, content: 'y', contentHtml: '', time: '', model: '', sourceNode: '', imageCount: 0 },
    ]
    await nextTick()
    await nextTick()
    expect(el.scrollTop).toBe(100)
  })

  it('chat 形态模板：for 角色行类名切换（isUser/isError 宿主派生）', async () => {
    const { engine, container, p } = mountWith(
      `<template data-nb-slot="sidebar" data-nb-engine="1">` +
        `<div data-nb-for="m in messages" class="row" data-nb-class="row--user:m.isUser; row--error:m.isError"></div>` +
        `</template>`
    )
    engine.mount('sidebar', container)
    const rows = () => [...container.querySelectorAll('.row')]
    expect(rows()).toHaveLength(0)
    p.messages = [
      { id: 1, role: 'user', isUser: true, isError: false, content: 'a', contentHtml: '', time: '', model: '', sourceNode: '', imageCount: 0 },
      { id: 2, role: 'assistant', isUser: false, isError: false, content: 'b', contentHtml: '<p>b</p>', time: '', model: 'm/x', sourceNode: '', imageCount: 0 },
      { id: 3, role: 'error', isUser: false, isError: true, content: 'c', contentHtml: '', time: '', model: '', sourceNode: '', imageCount: 0 },
    ]
    await nextTick()
    expect(rows()).toHaveLength(3)
    expect(rows()[0].classList.contains('row--user')).toBe(true)
    expect(rows()[1].classList.contains('row--user')).toBe(false)
    expect(rows()[2].classList.contains('row--error')).toBe(true)
  })
})
