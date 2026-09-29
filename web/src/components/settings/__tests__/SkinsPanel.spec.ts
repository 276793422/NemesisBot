import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// 设置页「皮肤」tab 面板测试（P1）。后端行为由
// crates/nemesis-web/src/handlers/skins/tests.rs 钉住（后端唯一真相源）；
// 这里钉 dispatch 语义与徽标/按钮态呈现。P2a：脚本总闸/同意卡/脚本徽标。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

const refreshMock = vi.fn()
const needsReloadMock = vi.fn(() => false)
const consentMock = vi.fn()
// skinState 必须在工厂内创建（vi.mock 提升：工厂运行早于本文件 body 的
// const 初始化；箭头闭包引用 body 变量没关系——惰性解引用）。测试体直接
// import 工厂产物拿同一个 reactive 实例。
vi.mock('../../../composables/useSkin', async () => {
  const { reactive } = await import('vue')
  const state = reactive({ id: '', meta: null, slots: [] as string[], structRev: 0, scriptPending: '' })
  return {
    applySkinRefresh: (...args: any[]) => refreshMock(...args),
    applySkinBoot: vi.fn(),
    skinState: state,
    resolveScriptConsent: (...args: any[]) => consentMock(...args),
    skinScriptNeedsReload: (...args: any[]) => needsReloadMock(),
    discardScriptPending: () => { state.scriptPending = '' }, // 与真实实现同语义
  }
})

const toastMock = { success: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() }
vi.mock('../../../composables/useToast', () => ({
  useToast: () => toastMock,
}))

const authedFetchMock = vi.fn()
vi.mock('../../../lib/authFetch', () => ({
  authedFetch: (...args: any[]) => authedFetchMock(...args),
}))

import SkinsPanel from '../SkinsPanel.vue'
import { skinState as skinStateMock } from '../../../composables/useSkin'

function skin(over: Record<string, unknown> = {}) {
  return {
    id: 'alpha',
    file: 'alpha.nbskin',
    status: 'ok',
    status_detail: null,
    signature: 'unsigned',
    sig_detail: null,
    has_script: false,
    manifest: {
      id: 'alpha',
      name: 'Alpha Skin',
      version: '1.0.0',
      author: 'zoo',
      description: 'A test skin',
      type: 'theme',
      variants: ['dark', 'light'],
      entry: 'skin/main.css',
    },
    sha256: 'ab'.repeat(32),
    id_mismatch: false,
    ...over,
  }
}

function listResp(skins: unknown[], over: Record<string, unknown> = {}) {
  return { dir: '/opt/bot/skins', dir_exists: true, skins, ...over }
}

function mountPanel() {
  return mount(SkinsPanel, { attachTo: document.body })
}

beforeEach(() => {
  requestMock.mockReset()
  refreshMock.mockReset()
  needsReloadMock.mockReset()
  needsReloadMock.mockReturnValue(false)
  consentMock.mockReset()
  authedFetchMock.mockReset()
  toastMock.success.mockClear()
  toastMock.error.mockClear()
  skinStateMock.id = ''
  skinStateMock.scriptPending = ''
  localStorage.clear()
})

describe('SkinsPanel', () => {
  it('renders default card + entries with badges and meta', async () => {
    requestMock.mockResolvedValue(listResp([skin(), skin({ id: 'signed', manifest: { id: 'signed', name: 'S', version: '2.0.0', author: '', description: '', type: 'theme', variants: [], entry: 'skin/s.css', }, signature: 'verified' })]))
    const w = mountPanel()
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('skins', 'list')
    const cards = w.findAll('.skin-card')
    expect(cards).toHaveLength(3) // default + 2 skins

    // 默认卡
    expect(cards[0].text()).toContain('默认观感')
    // 元数据与徽标
    const alpha = cards[1]
    expect(alpha.text()).toContain('Alpha Skin')
    expect(alpha.text()).toContain('v1.0.0')
    expect(alpha.text()).toContain('zoo')
    expect(alpha.text()).toContain('暗色')
    expect(alpha.text()).toContain('⚪ 未签名')
    expect(alpha.find('.skin-sha').text()).toHaveLength(12)
    const signed = cards[2]
    expect(signed.text()).toContain('✅ 已验证')
    w.unmount()
  })

  it('broken card is grayed with reason and no activate button', async () => {
    requestMock.mockResolvedValue(listResp([
      skin({ id: 'broken', status: 'broken', status_detail: 'ZIP 无法解析：bad archive', manifest: { id: 'broken', name: '', version: '', author: '', description: '', type: 'theme', variants: [], entry: null, } }),
    ]))
    const w = mountPanel()
    await flushPromises()

    const card = w.findAll('.skin-card')[1]
    expect(card.classes()).toContain('skin-broken')
    expect(card.text()).toContain('包体损坏')
    expect(card.text()).toContain('bad archive')
    // entry=null（theme 形态失效）→ 不渲染激活按钮
    expect(card.find('button.btn-primary').exists()).toBe(false)
    w.unmount()
  })

  it('no-payload skin (type=app, no entry) has no open-app and no activate button', async () => {
    requestMock.mockResolvedValue(listResp([
      skin({ id: 'buddy', manifest: { id: 'buddy', name: 'Buddy', version: '1.0.0', author: '', description: '', type: 'app', variants: [], entry: null } }),
    ]))
    const openSpy = vi.spyOn(window, 'open').mockImplementation(() => null)
    const w = mountPanel()
    await flushPromises()

    const card = w.findAll('.skin-card')[1]
    const buttons = card.findAll('button')
    // 无主题载荷 = 无任何操作入口（既不能设为默认观感，也不存在「打开
    // 应用」——app 形态已从皮肤语义中移除，面板不提供任何新开页面入口）
    expect(buttons.map((b) => b.text()).join()).not.toContain('设为默认观感')
    expect(buttons.map((b) => b.text()).join()).not.toContain('打开应用')
    expect(openSpy).not.toHaveBeenCalled()
    openSpy.mockRestore()
    w.unmount()
  })

  it('set_active dispatches then refreshes skin without reload', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(listResp([skin()]))
      if (cmd === 'set_active') return Promise.resolve({ active: 'alpha' })
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })
    refreshMock.mockResolvedValue(undefined)
    const w = mountPanel()
    await flushPromises()

    const card = w.findAll('.skin-card')[1]
    await card.find('button.btn-primary').trigger('click')
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('skins', 'set_active', { id: 'alpha' })
    expect(refreshMock).toHaveBeenCalledTimes(1)
    expect(toastMock.success).toHaveBeenCalled()
    w.unmount()
  })

  it('set_active failure surfaces error toast and skips refresh', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(listResp([skin()]))
      if (cmd === 'set_active') return Promise.reject(new Error('该皮肤无主题载荷'))
      return Promise.reject(new Error('unexpected'))
    })
    const w = mountPanel()
    await flushPromises()

    const card = w.findAll('.skin-card')[1]
    await card.find('button.btn-primary').trigger('click')
    await flushPromises()

    expect(refreshMock).not.toHaveBeenCalled()
    expect(toastMock.error).toHaveBeenCalledWith(expect.stringContaining('该皮肤无主题载荷'))
    w.unmount()
  })

  it('shows id mismatch note and invalid signature detail', async () => {
    requestMock.mockResolvedValue(listResp([
      skin({ id: 'renamed', id_mismatch: true, signature: 'invalid', sig_detail: 'Tampered(digest mismatch)', manifest: { id: 'original', name: 'R', version: '', author: '', description: '', type: 'theme', variants: [], entry: 'skin/m.css', } }),
    ]))
    const w = mountPanel()
    await flushPromises()

    const card = w.findAll('.skin-card')[1]
    expect(card.text()).toContain('manifest.id（original）与文件名（renamed）不一致')
    expect(card.find('.sig-badge').attributes('title')).toContain('Tampered(digest mismatch)')
    w.unmount()
  })

  it('missing skins dir surfaces honest state', async () => {
    requestMock.mockResolvedValue(listResp([], { dir: null, dir_exists: false }))
    const w = mountPanel()
    await flushPromises()

    expect(w.text()).toContain('皮肤系统未装配')
    w.unmount()
  })

  // ===== P2 下载三入口 + P3 五态/CRL（2026-09-27）=====

  it('official release install dispatches skins.install and shows result card with badge', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'list') return Promise.resolve(listResp([]))
      if (cmd === 'install') {
        expect(data).toEqual({ source: 'release', overwrite: false })
        return Promise.resolve({
          id: 'bot', file: 'bot.nbskin', signature: 'verified', sig_detail: null,
          manifest: { name: 'Bot', version: '1.0.0' }, sha256: 'ab'.repeat(32), overwritten: false,
        })
      }
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })
    const w = mountPanel()
    await flushPromises()

    await w.find('[data-test="dl-toggle"]').trigger('click')
    expect(w.find('[data-test="dl-panel"]').exists()).toBe(true)
    await w.find('[data-test="dl-official"]').trigger('click')
    await flushPromises()

    const result = w.find('[data-test="dl-result"]')
    expect(result.exists()).toBe(true)
    expect(result.text()).toContain('✅ 已验证')
    expect(result.text()).toContain('bot')
    expect(toastMock.success).toHaveBeenCalledWith('已安装 1 个皮肤包')
    // 安装成功后列表被重拉
    expect(requestMock).toHaveBeenCalledWith('skins', 'list')
    w.unmount()
  })

  it('URL install dispatches {url} payload', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(listResp([]))
      if (cmd === 'install') return Promise.resolve({ id: 'x', file: 'x.nbskin', signature: 'unsigned', sha256: 'ab'.repeat(32), overwritten: false, manifest: null })
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })
    const w = mountPanel()
    await flushPromises()

    await w.find('[data-test="dl-toggle"]').trigger('click')
    await w.find('[data-test="dl-url"]').setValue('https://example.com/x.nbskin')
    await w.find('[data-test="dl-url-go"]').trigger('click')
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('skins', 'install', {
      url: 'https://example.com/x.nbskin',
      overwrite: false,
    })
    w.unmount()
  })

  it('local file import posts raw body via authedFetch and lands result card', async () => {
    requestMock.mockResolvedValue(listResp([]))
    authedFetchMock.mockResolvedValue({
      ok: true,
      json: () => Promise.resolve({ id: 'imp', file: 'imp.nbskin', signature: 'unsigned', sha256: 'cd'.repeat(32), overwritten: false, manifest: null }),
    })
    const w = mountPanel()
    await flushPromises()

    await w.find('[data-test="dl-toggle"]').trigger('click')
    const input = w.find('input[type="file"]')
    const file = new File(['PK'], 'a.nbskin')
    Object.defineProperty(input.element, 'files', { value: [file] })
    await input.trigger('change')
    await flushPromises()

    expect(authedFetchMock).toHaveBeenCalledWith(
      '/api/skins/import',
      expect.objectContaining({ method: 'POST', body: file }),
    )
    expect(w.find('[data-test="dl-result"]').text()).toContain('imp')
    w.unmount()
  })

  it('import physical rejection surfaces error toast (no silent swallow)', async () => {
    requestMock.mockResolvedValue(listResp([]))
    authedFetchMock.mockResolvedValue({
      ok: false,
      status: 422,
      json: () => Promise.resolve({ error: 'rejected', message: '包体物理不可服务，已拒收' }),
    })
    const w = mountPanel()
    await flushPromises()

    await w.find('[data-test="dl-toggle"]').trigger('click')
    const input = w.find('input[type="file"]')
    Object.defineProperty(input.element, 'files', { value: [new File(['junk'], 'a.nbskin')] })
    await input.trigger('change')
    await flushPromises()

    expect(toastMock.error).toHaveBeenCalledWith(expect.stringContaining('物理不可服务'))
    w.unmount()
  })

  it('revoked signature renders 🚫 badge with detail title', async () => {
    requestMock.mockResolvedValue(listResp([
      skin({ signature: 'revoked', sig_detail: 'Revoked(KeyFp=aa..): key_leak' }),
    ]))
    const w = mountPanel()
    await flushPromises()

    const card = w.findAll('.skin-card')[1]
    expect(card.text()).toContain('🚫 已吊销')
    expect(card.find('.sig-badge').attributes('title')).toContain('Revoked(KeyFp')
    w.unmount()
  })

  it('crl snapshot line renders present/absent honestly', async () => {
    requestMock.mockResolvedValue(listResp([], {
      crl: { present: true, verified: true, expired: false, version: 3, valid_until: 0, entries: 2, note: null },
    }))
    const w = mountPanel()
    await flushPromises()
    expect(w.find('.crl-line').exists()).toBe(true)
    expect(w.find('.crl-line').text()).toContain('已验签生效')
    expect(w.find('.crl-line').text()).toContain('2 条')
    w.unmount()

    // 不在场 → 不渲染（离线诚实，全默认）
    requestMock.mockResolvedValue(listResp([]))
    const w2 = mountPanel()
    await flushPromises()
    expect(w2.find('.crl-line').exists()).toBe(false)
    w2.unmount()
  })

  // ===== P2a 脚本能力：徽标 / 总闸 / 同意卡（2026-09-29）=====

  it('has_script entry shows 「脚本」 badge; script-only package still activatable', async () => {
    requestMock.mockResolvedValue(listResp([
      skin({ id: 'full', has_script: true }),
      skin({
        id: 'scriptonly',
        has_script: true,
        manifest: { id: 'scriptonly', name: 'ScriptOnly', version: '1.0.0', author: '', description: '', type: 'theme', variants: [], entry: null, script: 'skin/main.js' },
      }),
    ]))
    const w = mountPanel()
    await flushPromises()

    const cards = w.findAll('.skin-card')
    expect(cards[1].find('.skin-script-tag').exists()).toBe(true)
    expect(cards[1].text()).toContain('脚本')
    // script-only（无 CSS entry）= canActivate 走 script 载荷分支
    const so = cards[2]
    expect(so.find('.skin-script-tag').exists()).toBe(true)
    expect(so.find('button.btn-primary').exists()).toBe(true)
    w.unmount()
  })

  it('script switch on dispatches config.set_field then progressive refresh', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'list') return Promise.resolve(listResp([]))
      if (cmd === 'get') {
        expect(data).toBeUndefined()
        return Promise.resolve({ ui: { skins: { allow_scripts: false } } })
      }
      if (cmd === 'set_field') {
        expect(data).toEqual({ path: 'ui.skins.allow_scripts', value: true })
        return Promise.resolve({ ok: true })
      }
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })
    refreshMock.mockResolvedValue(undefined)
    const w = mountPanel()
    await flushPromises()
    expect((w.find('[data-test="script-switch"] input').element as HTMLInputElement).checked).toBe(false)

    await w.find('[data-test="script-switch"] input').setValue(true)
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'ui.skins.allow_scripts', value: true })
    expect(refreshMock).toHaveBeenCalledTimes(1) // 开闸 = 渐进注入
    expect(toastMock.success).toHaveBeenCalled()
    w.unmount()
  })

  it('set_field failure surfaces error toast; switch state follows config truth', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(listResp([]))
      if (cmd === 'get') return Promise.resolve({ ui: { skins: { allow_scripts: false } } })
      if (cmd === 'set_field') return Promise.reject(new Error('config 写入失败'))
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })
    const w = mountPanel()
    await flushPromises()

    await w.find('[data-test="script-switch"] input').setValue(true)
    await flushPromises()

    // 失败=未生效：错误浮出 + 不做渐进刷新（闸仍关，无脚本可注入）
    expect(toastMock.error).toHaveBeenCalledWith(expect.stringContaining('开关保存失败'))
    expect(refreshMock).not.toHaveBeenCalled()
    w.unmount()

    // 保存失败后开关态跟随 config 真相源（fresh mount 重读）
    const w2 = mountPanel()
    await flushPromises()
    expect((w2.find('[data-test="script-switch"] input').element as HTMLInputElement).checked).toBe(false)
    w2.unmount()
  })

  it('consent card appears on scriptPending and dispatches allow/deny', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') {
        return Promise.resolve(listResp([
          skin({ id: 'scripty', has_script: true, signature: 'verified' }),
        ]))
      }
      if (cmd === 'get') return Promise.resolve({})
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })
    const w = mountPanel()
    await flushPromises()
    expect(w.find('[data-test="script-consent"]').exists()).toBe(false)

    skinStateMock.scriptPending = 'scripty'
    await flushPromises()
    const card = w.find('[data-test="script-consent"]')
    expect(card.exists()).toBe(true)
    expect(card.text()).toContain('Alpha Skin') // displayName 来自列表行
    expect(card.text()).toContain('✅ 已验证') // 签名如实展示（不拦截）
    expect(card.text()).toContain('abababab') // shortSha(整包 sha)

    await card.find('[data-test="script-allow"]').trigger('click')
    expect(consentMock).toHaveBeenLastCalledWith(true)
    await card.find('[data-test="script-deny"]').trigger('click')
    expect(consentMock).toHaveBeenLastCalledWith(false)
    w.unmount()
  })

  it('缺陷 #9：switch off discards pending consent card（闸关 = JS 不执行，卡不留）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'list') return Promise.resolve(listResp([skin({ id: 'scripty', has_script: true })]))
      if (cmd === 'get') return Promise.resolve({ ui: { skins: { allow_scripts: true } } })
      if (cmd === 'set_field') {
        expect(data).toEqual({ path: 'ui.skins.allow_scripts', value: false })
        return Promise.resolve({ ok: true })
      }
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })
    const w = mountPanel()
    await flushPromises()
    skinStateMock.scriptPending = 'scripty'
    await flushPromises()
    expect(w.find('[data-test="script-consent"]').exists()).toBe(true)

    await w.find('[data-test="script-switch"] input').setValue(false)
    await flushPromises()

    // pending 废弃（卡消失）——卡上 bytes-in-hand 不得在闸关后仍可被同意注入
    expect(skinStateMock.scriptPending).toBe('')
    expect(w.find('[data-test="script-consent"]').exists()).toBe(false)
    expect(refreshMock).not.toHaveBeenCalled() // 无 live 脚本 → 无需整页刷新
    w.unmount()
  })

  // 以下测试替换 window.location（jsdom [LegacyUnforgeable]，仅 defineProperty
  // 可换；替换后不可恢复）——保持文件内最后执行。
  it('switch off with live script running → config saved then whole-page reload', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'list') return Promise.resolve(listResp([]))
      if (cmd === 'get') return Promise.resolve({ ui: { skins: { allow_scripts: true } } })
      if (cmd === 'set_field') {
        expect(data).toEqual({ path: 'ui.skins.allow_scripts', value: false })
        return Promise.resolve({ ok: true })
      }
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })
    needsReloadMock.mockReturnValue(true) // live 脚本在场
    const reloadMock = vi.fn()
    Object.defineProperty(window, 'location', {
      value: { href: 'http://localhost/', reload: reloadMock },
      writable: true,
    })
    const w = mountPanel()
    await flushPromises()

    await w.find('[data-test="script-switch"] input').setValue(false)
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('config', 'set_field', { path: 'ui.skins.allow_scripts', value: false })
    expect(reloadMock).toHaveBeenCalledTimes(1) // 裁定 2：关闸必刷新卸载 JS
    expect(refreshMock).not.toHaveBeenCalled()
    w.unmount()
  })

  it('set_active with live script running → reload instead of hot refresh', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(listResp([skin()]))
      if (cmd === 'set_active') return Promise.resolve({ active: 'alpha' })
      return Promise.reject(new Error(`unexpected ${cmd}`))
    })
    needsReloadMock.mockReturnValue(true)
    const reloadMock = vi.fn()
    Object.defineProperty(window, 'location', {
      value: { href: 'http://localhost/', reload: reloadMock },
      writable: true,
    })
    const w = mountPanel()
    await flushPromises()

    const card = w.findAll('.skin-card')[1]
    await card.find('button.btn-primary').trigger('click')
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('skins', 'set_active', { id: 'alpha' })
    expect(reloadMock).toHaveBeenCalledTimes(1) // 裁定 2：切向任何目的地都刷新
    expect(refreshMock).not.toHaveBeenCalled()
    w.unmount()
  })
})
