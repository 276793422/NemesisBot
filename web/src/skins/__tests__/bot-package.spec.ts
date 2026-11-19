/**
 * 包-引擎契约测试：读 skins 子模块真实包的 structure.html，过
 * sanitize + 引擎挂载全链——包在 skins 子模块手改后此 spec 钉死「引擎
 * 认不认」。路径跨仓库（../skins 是 git 子模块，checkout 后必在）。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { nextTick } from 'vue'
import { parseSkinStructure } from '../sanitize'
import { SkinStructureEngine } from '../engine'
import type { SkinProjection } from '../types'

// vitest 以 web/ 为 cwd（npm test 工作目录），skins 子模块在仓库根。
const SKINS_DIR = resolve(process.cwd(), '../skins')

function loadStructure(pkg: string): string {
  return readFileSync(`${SKINS_DIR}/${pkg}/skin/structure.html`, 'utf-8')
}

const PACKAGES = ['bot', 'openlikebuddy', 'focus']

// 每包声明的槽位（structure.html template 清单——加槽先改这里）
const EXPECTED_SLOTS: Record<string, string[]> = {
  bot: ['chat', 'sidebar', 'statusbar', 'titlebar'],
  openlikebuddy: ['launcher', 'sidebar', 'statusbar', 'titlebar'],
  focus: ['chat', 'sidebar'],
}

// 每包 brand 文案（sidebar/titlebar 品牌绑定渲染断言用）
const BRANDS: Record<string, string> = {
  bot: 'NemesisBot',
  openlikebuddy: 'Buddy',
  focus: 'Focus',
}

describe.each(PACKAGES)('皮肤包 %s：structure.html ↔ 引擎契约', (pkg) => {
  it('槽位齐全、engine=1、清洗后无危险残留', () => {
    const parsed = parseSkinStructure(loadStructure(pkg))
    expect(parsed, `${pkg} structure.html 必须能被引擎解析`).not.toBeNull()
    expect(parsed!.engine).toBe(1)
    // v3 起 chat 槽可选（bot 参考实现声明了它并折叠 launcher 语义；
    // openlikebuddy 未声明 = 内置聊天区 + launcher 槽原样；
    // focus 双槽 = 专注对话形态，无 chrome 顶/底栏）
    expect([...parsed!.slots.keys()].sort()).toEqual(EXPECTED_SLOTS[pkg]!)
    for (const frag of parsed!.slots.values()) {
      expect(frag.querySelector('script,iframe,object,embed,link,meta,base')).toBeNull()
      for (const el of frag.querySelectorAll('*')) {
        for (const attr of Array.from(el.attributes)) {
          expect(attr.name.toLowerCase().startsWith('on'), `${pkg}: on* 残留 ${attr.name}`).toBe(false)
        }
      }
    }
  })

  it('sidebar 槽关键动作路由在白名单语义内（静态段可枚举）', () => {
    const parsed = parseSkinStructure(loadStructure(pkg))!
    const frag = parsed.slots.get('sidebar')!
    const actions = [...frag.querySelectorAll('[data-nb-action]')].map((el) => el.getAttribute('data-nb-action'))
    for (const name of ['new-chat', 'route:', 'local:toggle:', 'switch-session:', 'remove-session:', 'estop-toggle', 'toggle-theme', 'logout']) {
      expect(actions.some((a) => (a ?? '').startsWith(name)), `${pkg} 缺动作 ${name}（现有: ${actions.join(',')}）`).toBe(true)
    }
  })

  it('全槽挂载不抛错且投影渲染出品牌文案', async () => {
    const parsed = parseSkinStructure(loadStructure(pkg))!
    const engine = new SkinStructureEngine()
    const projection: SkinProjection = {
      brand: BRANDS[pkg]!,
      version: '1', scenes: ['写代码'], connected: true, connectionText: '已连接',
      readyText: '就绪', statusVersion: '· v1', model: '· m', mobileOpen: false,
      navPrimary: [{ label: '人格', path: '/persona', active: true }], navMore: [],
      sessionGroups: [{ label: '今天', sessions: [{ id: 's1', title: 'T', relTime: '刚刚', active: false, pinned: false }] }],
      projectGroups: [],
      hasSessions: true, currentSessionId: 's1', sortLabel: '最近', estopEngaged: false, estopBusy: false,
      estopLabel: '急停', signature: { visible: false, state: 'unsigned', label: '', title: '' },
      fullAccess: false, theme: 'dark', themeToggleLabel: '',
      messages: [], chatBusy: false, inputText: '', mode: 'build', isPlan: false, work: null, pages: {},
    }
    engine.projection = projection
    engine.load(parsed.slots)
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const containers = new Map<string, HTMLElement>()
    for (const slot of parsed.slots.keys()) {
      const c = document.createElement('div')
      document.body.appendChild(c)
      containers.set(slot, c)
      expect(engine.mount(slot, c), `${pkg} 槽 ${slot} 挂载失败`).toBe(true)
    }
    await nextTick()
    // 品牌绑定渲染（titlebar + launcher 两处）
    const brandTexts = [...containers.values()].map((c) => c.textContent ?? '').join('|')
    expect(brandTexts).toContain(projection.brand)
    expect(brandTexts).toContain('急停')
    for (const [, c] of containers) c.remove()
    engine.unmountAll()
    warn.mockRestore()
  })
})

beforeEach(() => {
  document.body.innerHTML = ''
})
