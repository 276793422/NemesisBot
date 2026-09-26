import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// 设置页「皮肤」tab 面板测试（P1）。后端行为由
// crates/nemesis-web/src/handlers/skins/tests.rs 钉住（后端唯一真相源）；
// 这里钉 dispatch 语义与徽标/按钮态呈现。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

const refreshMock = vi.fn()
vi.mock('../../../composables/useSkin', () => ({
  applySkinRefresh: (...args: any[]) => refreshMock(...args),
  applySkinBoot: vi.fn(),
}))

const toastMock = { success: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() }
vi.mock('../../../composables/useToast', () => ({
  useToast: () => toastMock,
}))

import SkinsPanel from '../SkinsPanel.vue'

function skin(over: Record<string, unknown> = {}) {
  return {
    id: 'alpha',
    file: 'alpha.nbskin',
    status: 'ok',
    status_detail: null,
    signature: 'unsigned',
    sig_detail: null,
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
  toastMock.success.mockClear()
  toastMock.error.mockClear()
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
})
