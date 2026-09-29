import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

// P2b 契约层守护（对齐 WSAPI commands() L1 注册表先例）：window.NemesisSkin
// 存在性 + 模型形状 + schema 漂移防线。契约破坏 = 这里红（宿主 CI），
// 不许烂在三方脚本里。

const wsRequestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => wsRequestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import { installNemesisSkin, NEMESIS_SKIN_VERSION } from '../contract'
import { NAV_GROUPS, visibleNavGroups, flatNavModel } from '../navModel'
import { allSchemaFields } from '../configSchema'
import { router } from '../../router'
import configDefault from '../../../../nemesisbot/config/config.default.json'

beforeEach(() => {
  delete (window as { NemesisSkin?: unknown }).NemesisSkin
  wsRequestMock.mockReset()
})
afterEach(() => {
  vi.restoreAllMocks()
})

describe('NemesisSkin 契约 API', () => {
  it('install 后 window.NemesisSkin 在场且形状完整；幂等不覆盖', () => {
    installNemesisSkin()
    const api = window.NemesisSkin
    expect(api).toBeTruthy()
    expect(api!.version).toBe(NEMESIS_SKIN_VERSION)
    expect(typeof api!.navigate).toBe('function')
    expect(typeof api!.nav.model).toBe('function')
    expect(typeof api!.nav.onChange).toBe('function')
    expect(typeof api!.config.schema).toBe('function')
    expect(typeof api!.config.pages).toBe('function')

    // 幂等：二次安装不换实例（脚本持有的引用继续有效）
    const ref = api!
    installNemesisSkin()
    expect(window.NemesisSkin).toBe(ref)
  })

  it('nav.model() 形状：{id,label,icon,route,group} 全字符串、id 唯一', () => {
    installNemesisSkin()
    const model = window.NemesisSkin!.nav.model()
    expect(model.length).toBeGreaterThan(10)
    const ids = new Set<string>()
    for (const item of model) {
      expect(typeof item.id).toBe('string')
      expect(item.id.length).toBeGreaterThan(0)
      expect(typeof item.label).toBe('string')
      expect(typeof item.icon).toBe('string')
      expect(item.route.startsWith('/')).toBe(true)
      expect(typeof item.group).toBe('string')
      ids.add(item.id)
    }
    expect(ids.size).toBe(model.length)
  })

  it('model() 是深拷贝：改返回值不影响宿主模型', () => {
    installNemesisSkin()
    const a = window.NemesisSkin!.nav.model()
    a[0].label = '篡改'
    const b = window.NemesisSkin!.nav.model()
    expect(b[0].label).not.toBe('篡改')
    expect(b.map((i) => i.id)).toEqual(a.map((i) => i.id))
  })

  it('navigate(已知 id) 走 router.push；未知 id 返回 false 不抛错', async () => {
    installNemesisSkin()
    const push = vi.spyOn(router, 'push').mockResolvedValue(undefined as never)
    expect(window.NemesisSkin!.navigate('models')).toBe(true)
    expect(push).toHaveBeenCalledWith('/models')
    expect(window.NemesisSkin!.navigate('no-such-page')).toBe(false)
    expect(push).toHaveBeenCalledTimes(1)
  })

  it('nav.onChange 订阅即回调当前模型；退订后不再回调', () => {
    installNemesisSkin()
    const seen: number[] = []
    const unsub = window.NemesisSkin!.nav.onChange((m) => seen.push(m.length))
    expect(seen).toEqual([flatNavModel().length]) // 订阅即发布一次
    unsub()
    window.NemesisSkin!.nav.onChange(() => seen.push(-1)) // 二次订阅（验证退订不影响新订阅者）
    expect(seen.length).toBe(2)
  })

  it('config.schema(已知页) 返回字段；未知页返回 null', () => {
    installNemesisSkin()
    const page = window.NemesisSkin!.config.schema('agents')
    expect(page).not.toBeNull()
    expect(page!.id).toBe('agents')
    expect(page!.fields.length).toBeGreaterThanOrEqual(10)
    for (const f of page!.fields) {
      expect(f.key.startsWith('agents.')).toBe(true)
      expect(f.label.length).toBeGreaterThan(0)
      expect(['boolean', 'number', 'string', 'enum']).toContain(f.type)
      expect(f.default).toBeDefined()
    }
    // enum 有选项；number 有范围；秘密字段标记
    const mode = page!.fields.find((f) => f.key === 'agents.defaults.concurrent_request_mode')
    expect(mode!.options).toEqual(['queue', 'reject', 'steer'])
    expect(mode!.default).toBe('queue')
    const temp = page!.fields.find((f) => f.key === 'agents.defaults.temperature')
    expect(temp!.range).toEqual({ min: 0, max: 2, step: 0.05 })
    const channels = window.NemesisSkin!.config.schema('channels')
    const tg = channels!.fields.find((f) => f.key === 'channels.telegram.token')
    expect(tg!.secret).toBe(true)
    expect(window.NemesisSkin!.config.schema('no-such')).toBeNull()
  })

  it('config.pages() 覆盖全部页 id', () => {
    installNemesisSkin()
    const pages = window.NemesisSkin!.config.pages()
    for (const p of ['agents', 'channels', 'tools', 'logging', 'gateway', 'memory']) {
      expect(pages).toContain(p)
    }
    for (const p of pages) {
      expect(window.NemesisSkin!.config.schema(p)).not.toBeNull()
    }
  })

  it('config.current()/set() 走 WSAPI config.get/set_field（P3-b 写回通路）', async () => {
    installNemesisSkin()
    wsRequestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'get') return Promise.resolve({ agents: { defaults: { temperature: 0.3 } } })
      if (cmd === 'set_field') return Promise.resolve({ ok: true })
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })
    const cfg = (await window.NemesisSkin!.config.current()) as {
      agents: { defaults: { temperature: number } }
    }
    expect(cfg.agents.defaults.temperature).toBe(0.3)
    await window.NemesisSkin!.config.set('agents.defaults.temperature', 0.9)
    expect(wsRequestMock).toHaveBeenCalledWith('config', 'set_field', {
      path: 'agents.defaults.temperature',
      value: 0.9,
    })
  })
})

describe('导航模型与路由一致性（契约 vs router）', () => {
  it('每个 nav item 的 route 在 router 注册表里真实存在', () => {
    const routePaths = new Set(router.getRoutes().map((r) => r.path))
    for (const item of flatNavModel()) {
      expect(routePaths.has(item.route)).toBe(true)
    }
  })

  it('data-nb-shell 容器标记在场（皮肤壳 CSS 的唯一合法锚点；标记消失 = 红）', () => {
    // 皮肤脚本（如 coraldusk-full）只许锚这四个标记，绝不引用宿主内部
    // 类名——宿主重构类名不破皮肤；删/改名标记必须过这道红。
    const appLayout = readFileSync(resolve(import.meta.dirname, '../../components/AppLayout.vue'), 'utf8')
    const sidebar = readFileSync(resolve(import.meta.dirname, '../../components/Sidebar.vue'), 'utf8')
    expect(appLayout).toContain('data-nb-shell="root"')
    expect(appLayout).toContain('data-nb-shell="main"')
    expect(appLayout).toContain('data-nb-shell="mobile-overlay"')
    expect(sidebar).toContain('data-nb-shell="sidebar"')
  })

  it('feature 门控过滤在 Sidebar 数据源与契约模型间一致（同一份模块）', () => {
    const grouped = visibleNavGroups().flatMap((g) => g.items.map((i) => i.id))
    expect(grouped).toEqual(flatNavModel().map((i) => i.id))
    // 全量清单（未过滤）包含过滤后的每一项（门控只隐藏不新增）
    const all = new Set(NAV_GROUPS.flatMap((g) => g.items.map((i) => i.id)))
    for (const id of grouped) expect(all.has(id)).toBe(true)
  })
})

describe('config schema 漂移防线（v2 修订）', () => {
  type Leaf = { path: string; kind: 'scalar' | 'list' }

  function walkLeaves(node: unknown, path: string, out: Leaf[]): void {
    if (Array.isArray(node)) {
      out.push({ path: path + '[]', kind: 'list' })
    } else if (node && typeof node === 'object') {
      for (const [k, v] of Object.entries(node as Record<string, unknown>)) {
        walkLeaves(v, path ? `${path}.${k}` : k, out)
      }
    } else {
      out.push({ path, kind: 'scalar' })
    }
  }

  /** 显式 ignore：列表叶（v1 schema 不做列表编辑——allow_from/sync_to/
   * model_list 等由各专页管理）。新列表叶自动入此规则，无需逐条标注；
   * 标量叶必须逐叶显式映射（无 blanket ignore）。 */
  const isIgnored = (leaf: Leaf): boolean => leaf.kind === 'list'

  it('config.default.json 每个标量叶都有 schema 映射（新字段未映射 = 红）', () => {
    const leaves: Leaf[] = []
    walkLeaves(configDefault, '', leaves)
    const scalars = leaves.filter((l) => !isIgnored(l))
    expect(scalars.length).toBeGreaterThan(40)
    const mapped = new Set(allSchemaFields().map((f) => f.key))
    const missing = scalars.filter((l) => !mapped.has(l.path))
    expect(missing.map((l) => l.path)).toEqual([]) // 失败时列出全部缺映射路径
  })

  it('schema 每个键都在 config.default.json 里真实存在且类型一致（无幽灵字段）', () => {
    const leaves: Leaf[] = []
    walkLeaves(configDefault, '', leaves)
    const paths = new Set(leaves.map((l) => l.path))
    const ghost = allSchemaFields().filter((f) => !paths.has(f.key))
    expect(ghost.map((f) => f.key)).toEqual([])

    // 声明 type 与默认值实际类型一致（enum 是 string 的控件细化——默认值
    // 仍是字符串且必须 ∈ options；其余类型逐字段比对）
    const typeOf = (v: unknown): string =>
      typeof v === 'boolean' ? 'boolean' : typeof v === 'number' ? 'number' : 'string'
    for (const f of allSchemaFields()) {
      if (f.type === 'enum') {
        // enum 是 string 的控件细化：默认值仍是字符串且必须 ∈ options
        expect(typeOf(f.default)).toBe('string')
        expect(Array.isArray(f.options)).toBe(true)
        expect(f.options!.length).toBeGreaterThan(0)
        expect(f.options).toContain(f.default)
      } else {
        expect(f.type).toBe(typeOf(f.default))
      }
      if (f.type === 'number') {
        expect(f.range).toBeTruthy()
        expect(f.default).toBeGreaterThanOrEqual(f.range!.min)
        expect(f.default).toBeLessThanOrEqual(f.range!.max)
      }
    }
  })
})
