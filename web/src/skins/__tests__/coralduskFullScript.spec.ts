import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { flatNavModel } from '../navModel'

// P3 皮肤脚本（skins/coraldusk-full/skin/main.js）行为面单测：jsdom 里
// new Function 直接执行 IIFE，契约 API 用 mock。降级链（P3 契约协商）、
// 导航重组（P3-a）、友好表单（P3-b）、双壳切换（P3-c）在此全部钉死。

const SCRIPT = readFileSync(
  resolve(import.meta.dirname, '../../../../skins/coraldusk-full/skin/main.js'),
  'utf8',
)

type NavItem = { id: string; label: string; icon: string; route: string; group: string }
type Field = {
  key: string
  label: string
  type: 'boolean' | 'number' | 'string' | 'enum'
  range?: { min: number; max: number; step?: number }
  options?: string[]
  secret?: boolean
  default: unknown
}

function makeApi(overrides: Record<string, unknown> = {}) {
  return {
    version: 1,
    navigate: vi.fn(() => true),
    nav: {
      model: vi.fn((): NavItem[] => [
        { id: 'chat', label: '聊天', icon: 'm', route: '/', group: '主要' },
        { id: 'overview', label: '主页', icon: 'h', route: '/overview', group: '主要' },
        { id: 'usage', label: '用量', icon: 'u', route: '/usage', group: '主要' },
        { id: 'models', label: '模型', icon: 'd', route: '/models', group: '管理' },
        { id: 'security', label: '安全', icon: 's', route: '/security', group: '安全' },
      ]),
    },
    config: {
      schema: vi.fn((pid: string) =>
        pid === 'agents'
          ? {
              id: 'agents',
              title: 'Agent',
              fields: [
                { key: 'agents.defaults.restrict_to_workspace', label: '限制工作区', type: 'boolean', default: true },
                { key: 'agents.defaults.temperature', label: '温度', type: 'number', range: { min: 0, max: 2, step: 0.05 }, default: 0.7 },
                { key: 'agents.defaults.concurrent_request_mode', label: '并发模式', type: 'enum', options: ['queue', 'reject', 'steer'], default: 'queue' },
                { key: 'agents.defaults.workspace', label: '工作区', type: 'string', default: './ws' },
                { key: 'tools.web.brave.api_key', label: 'Brave Key', type: 'string', secret: true, default: '' },
              ] as Field[],
            }
          : null,
      ),
      pages: vi.fn(() => ['agents']),
      current: vi.fn(() =>
        Promise.resolve({
          agents: { defaults: { restrict_to_workspace: false, temperature: 0.3, concurrent_request_mode: 'reject', workspace: '/my/ws' }, },
          tools: { web: { brave: { api_key: 'sk-live' } } },
        }),
      ),
      set: vi.fn(() => Promise.resolve()),
    },
    ...overrides,
  }
}

function runScript(api: unknown): void {
  ;(window as { NemesisSkin?: unknown }).NemesisSkin = api
  // eslint-disable-next-line @typescript-eslint/no-implied-eval, no-new-func
  new Function(SCRIPT)()
}

const stateAttr = () => document.documentElement.getAttribute('data-nb-skin-state')
const shellAttr = () => document.documentElement.getAttribute('data-nb-shell')

const navEl = () => document.querySelector('[data-nb-skin-nav]')
const chipEl = () => document.querySelector<HTMLButtonElement>('[data-nb-skin-shell-chip]')

/** 冲掉 openForm 的 Promise.all 链（微任务 + 宏任务各一拍）。 */
const flush = () => new Promise<void>((r) => setTimeout(r, 0))

beforeEach(() => {
  document.head.innerHTML = ''
  document.body.innerHTML = ''
  document.documentElement.removeAttribute('data-nb-skin-state')
  document.documentElement.removeAttribute('data-nb-shell')
  localStorage.clear()
  delete (window as { NemesisSkin?: unknown }).NemesisSkin
})
afterEach(() => {
  vi.restoreAllMocks()
})

describe('降级链（契约协商）', () => {
  it('契约缺失 → degraded:contract-missing，零 DOM 手术', () => {
    ;(window as { NemesisSkin?: unknown }).NemesisSkin = undefined
    new Function(SCRIPT)()
    expect(stateAttr()).toBe('degraded:contract-missing')
    expect(navEl()).toBeNull()
    expect(chipEl()).toBeNull()
    expect(document.head.querySelector('style')).toBeNull()
  })

  it('版本不符 → degraded:contract-version-N（不做猜测式兼容）', () => {
    runScript(makeApi({ version: 2 }))
    expect(stateAttr()).toBe('degraded:contract-version-2')
    expect(navEl()).toBeNull()
  })

  it('运行期异常 → degraded:error:<msg>，不裸抛', () => {
    const api = makeApi()
    ;(api.nav.model as ReturnType<typeof vi.fn>).mockImplementation(() => {
      throw new Error('boom')
    })
    runScript(api)
    expect(stateAttr()).toBe('degraded:error:boom')
    expect(chipEl()).toBeNull()
  })

  it('契约 v1 完整 → data-nb-skin-state=active', () => {
    runScript(makeApi())
    expect(stateAttr()).toBe('active')
    expect(navEl()).toBeTruthy()
    expect(chipEl()).toBeTruthy()
    // 壳样式由脚本注入（脚本死 → 壳样式消失 → 干净回落原生布局）
    const style = document.head.querySelector('style[data-nb-skin-shell-style]')
    expect(style).toBeTruthy()
    expect(style!.textContent).toContain('[data-nb-shell="friendly"] [data-nb-shell="sidebar"]')
    // 壳 CSS 只锚契约标记，绝不引用宿主内部类名
    expect(style!.textContent).not.toContain('aside.sidebar')
    expect(style!.textContent).not.toContain('.mobile-overlay')
  })
})

describe('P3-a 导航重组', () => {
  it('按 合并分组 渲染；模型里不存在的 id 自动省略（feature 裁剪收敛）', () => {
    runScript(makeApi())
    const titles = [...document.querySelectorAll('.nbk-nav-group-title')].map((e) => e.textContent)
    // 模型只有 chat/overview/usage/models/security → 空组（能力/高级/设置）不渲染
    expect(titles).toEqual(['主页', '安全'])
    const ids = [...document.querySelectorAll('.nbk-nav-item')].map((e) => e.getAttribute('data-nbk-id'))
    expect(ids).toEqual(['chat', 'overview', 'usage', 'models', 'security'])
    expect(document.querySelector('.nbk-nav-item')!.textContent).toContain('聊天')
  })

  it('完整性不变量：真实宿主模型（navModel）每个 id 都出现在导航（无隐藏页面）', () => {
    const model = flatNavModel()
    const api = makeApi()
    ;(api.nav.model as ReturnType<typeof vi.fn>).mockReturnValue(model)
    runScript(api)
    const ids = [...document.querySelectorAll('.nbk-nav-item')].map((e) => e.getAttribute('data-nbk-id'))
    expect(ids).toHaveLength(model.length)
    expect(new Set(ids)).toEqual(new Set(model.map((i) => i.id)))
  })

  it('完整性不变量：宿主新增页（分组表未认领的 id）→ 兜底尾组照样可达', () => {
    const api = makeApi()
    const model = [
      ...(api.nav.model() as NavItem[]),
      { id: 'future-page', label: '未来页', icon: 'x', route: '/future-page', group: '其他' },
    ]
    ;(api.nav.model as ReturnType<typeof vi.fn>).mockReturnValue(model)
    runScript(api)
    const ids = [...document.querySelectorAll('.nbk-nav-item')].map((e) => e.getAttribute('data-nbk-id'))
    expect(ids).toContain('future-page')
  })

  it('点击项 → navigate(id)（绝不自拼 URL）', () => {
    const api = makeApi()
    runScript(api)
    const item = document.querySelector('[data-nbk-id="overview"]') as HTMLElement
    item.click()
    expect(api.navigate).toHaveBeenCalledWith('overview')
  })

  it('当前路由高亮 + hashchange 跟随', () => {
    window.location.hash = '#/usage'
    runScript(makeApi())
    expect(document.querySelector('[data-nbk-id="usage"]')!.classList.contains('active')).toBe(true)
    expect(document.querySelector('[data-nbk-id="chat"]')!.classList.contains('active')).toBe(false)
    window.location.hash = '#/models'
    window.dispatchEvent(new Event('hashchange'))
    expect(document.querySelector('[data-nbk-id="models"]')!.classList.contains('active')).toBe(true)
    expect(document.querySelector('[data-nbk-id="usage"]')!.classList.contains('active')).toBe(false)
  })

  it('Vue Router hash 模式实况：history.pushState 不发 hashchange——脚本包 pushState 触发重绘', () => {
    runScript(makeApi())
    // 真实宿主路径：router.push → history.pushState（无 hashchange 事件）
    history.pushState(null, '', '#/security')
    expect(document.querySelector('[data-nbk-id="security"]')!.classList.contains('active')).toBe(true)
    expect(document.querySelector('[data-nbk-id="chat"]')!.classList.contains('active')).toBe(false)
  })
})

describe('P3-c 双壳', () => {
  it('默认 friendly 壳：原生侧栏隐藏选择器在场 + chip 文案「经典壳」', () => {
    runScript(makeApi())
    expect(shellAttr()).toBe('friendly')
    expect(chipEl()!.textContent).toBe('▤ 经典壳')
    expect(localStorage.getItem('nemesisbot_skin_shell')).toBe('friendly')
  })

  it('chip 点击切 classic：持久化 + chip 变「简洁壳」+ 再点切回', () => {
    runScript(makeApi())
    chipEl()!.click()
    expect(shellAttr()).toBe('classic')
    expect(localStorage.getItem('nemesisbot_skin_shell')).toBe('classic')
    expect(chipEl()!.textContent).toBe('✦ 简洁壳')
    chipEl()!.click()
    expect(shellAttr()).toBe('friendly')
    expect(localStorage.getItem('nemesisbot_skin_shell')).toBe('friendly')
  })

  it('持久化恢复：localStorage=classic 启动即 classic 壳', () => {
    localStorage.setItem('nemesisbot_skin_shell', 'classic')
    runScript(makeApi())
    expect(shellAttr()).toBe('classic')
    expect(chipEl()!.textContent).toBe('✦ 简洁壳')
  })
})

describe('P3-b 友好设置表单', () => {
  async function openAgentsForm(api = makeApi()) {
    runScript(api)
    ;(document.querySelector('.nbk-nav-foot .nbk-btn') as HTMLButtonElement).click()
    await flush()
    return api
  }

  it('打开表单：页签 + 按契约 schema 渲染控件，初值取 config.current()', async () => {
    await openAgentsForm()
    expect(document.querySelector('[data-nb-skin-form]')).toBeTruthy()
    expect(document.querySelector('.nbk-tab')!.textContent).toBe('Agent')
    // boolean → switch，初值 false（current 覆盖 default true）
    const sw = document.querySelector('.nbk-switch')!
    expect(sw.classList.contains('on')).toBe(false)
    // number+range → slider，初值 0.3
    const slider = document.querySelector<HTMLInputElement>('.nbk-range')!
    expect(slider.min).toBe('0')
    expect(slider.max).toBe('2')
    expect(slider.step).toBe('0.05')
    expect(slider.value).toBe('0.3')
    // enum → select，初值 reject
    const sel = document.querySelector<HTMLSelectElement>('.nbk-select')!
    expect([...sel.options].map((o) => o.value)).toEqual(['queue', 'reject', 'steer'])
    expect(sel.value).toBe('reject')
    // string → text input，初值取 current；secret → password + 不回显提示
    const inputs = [...document.querySelectorAll<HTMLInputElement>('.nbk-input')]
    expect(inputs[0].type).toBe('text')
    expect(inputs[0].value).toBe('/my/ws')
    expect(inputs[1].type).toBe('password')
    expect(inputs[1].value).toBe('sk-live')
    expect(document.querySelector('.nbk-form-body')!.textContent).toContain('密码框渲染')
  })

  it('写回走契约：开关点击 → config.set(key, 新布尔)；滑块 → Number；下拉 → 字符串', async () => {
    const api = await openAgentsForm()
    ;(document.querySelector('.nbk-switch') as HTMLButtonElement).click()
    expect(api.config.set).toHaveBeenCalledWith('agents.defaults.restrict_to_workspace', true)
    const slider = document.querySelector<HTMLInputElement>('.nbk-range')!
    slider.value = '1.5'
    slider.dispatchEvent(new Event('change'))
    expect(api.config.set).toHaveBeenCalledWith('agents.defaults.temperature', 1.5)
    const sel = document.querySelector<HTMLSelectElement>('.nbk-select')!
    sel.value = 'steer'
    sel.dispatchEvent(new Event('change'))
    expect(api.config.set).toHaveBeenCalledWith('agents.defaults.concurrent_request_mode', 'steer')
  })

  it('保存结果回显：成功 ✓ / 失败 ✕', async () => {
    const api = makeApi()
    ;(api.config.set as ReturnType<typeof vi.fn>).mockImplementation(() =>
      Promise.reject(new Error('denied')),
    )
    await openAgentsForm(api)
    ;(document.querySelector('.nbk-switch') as HTMLButtonElement).click()
    await flush()
    expect(document.querySelector('.nbk-hint.nbk-err')!.textContent).toContain('保存失败')
  })

  it('config.current() 拒绝 → 诚实报错不炸（表单不可用提示）', async () => {
    const api = makeApi()
    ;(api.config.current as ReturnType<typeof vi.fn>).mockImplementation(() =>
      Promise.reject(new Error('ws down')),
    )
    runScript(api)
    ;(document.querySelector('.nbk-nav-foot .nbk-btn') as HTMLButtonElement).click()
    await flush()
    expect(document.querySelector('.nbk-hint.nbk-err')!.textContent).toContain('配置读取失败')
    expect(stateAttr()).toBe('active') // 表单失败不算脚本降级——壳照常
  })
})
