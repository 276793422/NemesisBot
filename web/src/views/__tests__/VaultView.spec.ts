import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// L7c（2026-10-13）：凭据 Vault 页 —— 四操作（列表/新增/删除/解锁测试）
// + secret 全程脱敏（值永不进 DOM/请求日志面）+ 覆盖轮换 confirm 门。
// 后端语义由 crates/nemesis-web/src/handlers/vault/{vault.rs,tests.rs} 钉住。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import VaultView from '../VaultView.vue'

const confirmMock = vi.fn(() => true)
vi.stubGlobal('confirm', confirmMock)

function okStatus(over: Record<string, unknown> = {}) {
  return { exists: true, mode: 'dpapi', unlocked: true, alias_count: 1, path: 'C:\\ws\\config\\vault.enc', ...over }
}
function okList(over: Record<string, unknown> = {}) {
  return {
    exists: true,
    unlocked: true,
    entries: [
      { alias: 'openai/prod-key', domain: 'openai', description: '生产 key', created_at: '2026-10-13T00:00:00Z', rotated_at: null },
    ],
    ...over,
  }
}

beforeEach(() => {
  requestMock.mockReset()
  confirmMock.mockClear().mockReturnValue(true)
  requestMock.mockImplementation((module: string, cmd: string) => {
    if (module === 'vault' && cmd === 'status') return Promise.resolve(okStatus())
    if (module === 'vault' && cmd === 'list') return Promise.resolve(okList())
    return Promise.resolve({})
  })
  useToast().toasts.splice(0)
})

async function mountView() {
  const w = mount(VaultView)
  await flushPromises()
  return w
}

describe('VaultView 加载', () => {
  it('onMounted 拉 status+list，状态徽章与条目渲染', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('vault', 'status', {}, 8000)
    expect(requestMock).toHaveBeenCalledWith('vault', 'list', {}, 8000)
    expect(w.text()).toContain('已解锁')
    expect(w.text()).toContain('openai/prod-key')
    expect(w.text()).toContain('生产 key')
  })

  it('vault 缺失：诚实显示未创建，不炸', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve({ exists: false })
      if (cmd === 'list') return Promise.resolve({ exists: false, unlocked: false, entries: [] })
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(w.text()).toContain('尚未创建 vault 文件')
  })

  it('锁定态：横幅提示解锁路径', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve(okStatus({ unlocked: false }))
      if (cmd === 'list') return Promise.resolve(okList({ unlocked: false }))
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(w.text()).toContain('已锁定')
    expect(w.text()).toContain('NEMESISBOT_VAULT_PASSPHRASE')
  })
})

describe('VaultView 新增（vault.set）', () => {
  async function fillAndSubmit(w: Awaited<ReturnType<typeof mountView>>) {
    // 新增卡输入按 placeholder 锚定（ABCD 批 2026-11-23 起页面还有域绑定卡
    // 三个 form-input，索引定位脆弱——placeholder 前缀唯一区分四输入）。
    async function setByPlaceholder(prefix: string, val: string) {
      const el = w.findAll('input.form-input').find(i => (i.attributes('placeholder') || '').startsWith(prefix))
      await el!.setValue(val)
    }
    await setByPlaceholder('别名（必填', 'openai/prod-key')
    await setByPlaceholder('secret 值', 'sk-NEW-SECRET')
    await setByPlaceholder('域（可选', 'openai')
    await setByPlaceholder('备注', '备注')
    const btn = w.findAll('button').find(b => b.text() === '写入')!
    await btn.trigger('click')
    await flushPromises()
  }

  it('带 alias/value/domain/description 调 vault.set，成功后刷新', async () => {
    const w = await mountView()
    await fillAndSubmit(w)
    expect(requestMock).toHaveBeenCalledWith('vault', 'set', {
      alias: 'openai/prod-key', value: 'sk-NEW-SECRET', domain: 'openai', description: '备注', force: false,
    }, 8000)
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('已写入'))).toBe(true)
  })

  it('secret 输入框是 password 型（不回显）', async () => {
    const w = await mountView()
    const pwInput = w.findAll('input[type="password"]')
    expect(pwInput.length).toBeGreaterThanOrEqual(1)
  })

  it('空别名/空值：本地拒绝，不发请求', async () => {
    const w = await mountView()
    const before = requestMock.mock.calls.filter((c: any[]) => c[1] === 'set').length
    const btn = w.findAll('button').find(b => b.text() === '写入')!
    await btn.trigger('click')
    await flushPromises()
    const after = requestMock.mock.calls.filter((c: any[]) => c[1] === 'set').length
    expect(after).toBe(before)
    expect(useToast().toasts.some(t => t.type === 'error')).toBe(true)
  })

  it('已存在别名：后端 force:true 文案触发 confirm，确认后带 force 重发', async () => {
    const w = await mountView()
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'set' && !data.force) {
        return Promise.reject('别名「openai/prod-key」已存在。覆盖（轮换）不可撤销，请确认后带 force:true 重发')
      }
      return Promise.resolve({ alias: data.alias, rotated: true })
    })
    await fillAndSubmit(w)
    expect(confirmMock).toHaveBeenCalled()
    expect(requestMock).toHaveBeenCalledWith('vault', 'set', expect.objectContaining({ force: true }), 8000)
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('已轮换'))).toBe(true)
  })

  it('已存在别名：取消 confirm 不重发', async () => {
    confirmMock.mockReturnValue(false)
    const w = await mountView()
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'set' && !data.force) {
        return Promise.reject('别名「openai/prod-key」已存在。覆盖（轮换）不可撤销，请确认后带 force:true 重发')
      }
      return Promise.resolve({})
    })
    await fillAndSubmit(w)
    const forced = requestMock.mock.calls.filter((c: any[]) => c[1] === 'set' && c[2]?.force)
    expect(forced.length).toBe(0)
  })
})

describe('VaultView 删除（vault.remove）', () => {
  it('confirm 门 + 按 alias 删除 + 成功刷新', async () => {
    const w = await mountView()
    const btn = w.findAll('button').find(b => b.text() === '删除')!
    await btn.trigger('click')
    await flushPromises()
    expect(confirmMock).toHaveBeenCalled()
    expect(requestMock).toHaveBeenCalledWith('vault', 'remove', { alias: 'openai/prod-key' }, 8000)
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('已删除'))).toBe(true)
  })

  it('取消 confirm：不发 remove', async () => {
    confirmMock.mockReturnValue(false)
    const w = await mountView()
    const btn = w.findAll('button').find(b => b.text() === '删除')!
    await btn.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter((c: any[]) => c[1] === 'remove').length).toBe(0)
  })
})

describe('VaultView 解锁测试（vault.unlock）', () => {
  async function submitUnlock(w: Awaited<ReturnType<typeof mountView>>, pw: string) {
    // 解锁卡是第一个 password 输入。
    const input = w.findAll('input[type="password"]')[0]!
    await input.setValue(pw)
    const btn = w.findAll('button').find(b => b.text() === '测试解锁')!
    await btn.trigger('click')
    await flushPromises()
  }

  it('正确口令：unlock 成功 → 成功横幅 + 刷新', async () => {
    const w = await mountView()
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'unlock') return Promise.resolve({ unlocked: true, mode: 'argon2id' })
      return Promise.resolve({})
    })
    await submitUnlock(w, 'right-pass')
    expect(requestMock).toHaveBeenCalledWith('vault', 'unlock', { passphrase: 'right-pass' }, 8000)
    expect(w.text()).toContain('解锁成功（模式 argon2id）')
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('解锁测试通过'))).toBe(true)
  })

  it('错误口令：诚实失败横幅，口令不回显到页面', async () => {
    const w = await mountView()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'unlock') return Promise.reject('vault 解锁失败: bad passphrase')
      return Promise.resolve({})
    })
    await submitUnlock(w, 'wrong-pass')
    expect(w.text()).toContain('✗ vault 解锁失败')
    expect(w.text()).not.toContain('wrong-pass')
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('解锁测试失败'))).toBe(true)
  })

  it('空口令：本地拒绝不发请求', async () => {
    const w = await mountView()
    const before = requestMock.mock.calls.filter((c: any[]) => c[1] === 'unlock').length
    const btn = w.findAll('button').find(b => b.text() === '测试解锁')!
    await btn.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter((c: any[]) => c[1] === 'unlock').length).toBe(before)
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('请先输入'))).toBe(true)
  })
})

describe('VaultView 凭据免打扰（ABCD 批 A，vault.credential_status/set）', () => {
  function okCredAuto(over: Record<string, unknown> = {}) {
    return { injected: true, enabled: false, auto_count: 0, preauth_count: 0, denied_count: 0, ...over }
  }

  it('加载拉 credential_status；injected 时渲染开关与状态行', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'credential_status') return Promise.resolve(okCredAuto({ enabled: true, auto_count: 3 }))
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('vault', 'credential_status', {}, 8000)
    const toggle = w.find('input[data-test="credential-auto-toggle"]')
    expect(toggle.exists()).toBe(true)
    expect((toggle.element as HTMLInputElement).checked).toBe(true)
    expect(w.text()).toContain('免打扰中：凭据引用将自动放行（本次运行 3 次免卡）')
  })

  it('未注入（injected:false）：开关禁用 + 诚实降级提示', async () => {
    const w = await mountView()
    const toggle = w.find('input[data-test="credential-auto-toggle"]')
    expect((toggle.element as HTMLInputElement).disabled).toBe(true)
    expect(w.text()).toContain('统计载体未注入')
  })

  it('切换调用 credential_set 并回读状态', async () => {
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'credential_status') return Promise.resolve(okCredAuto({ enabled: data?.enabled ?? false }))
      if (cmd === 'credential_set') return Promise.resolve({ saved: true })
      return Promise.resolve({})
    })
    const w = await mountView()
    const toggle = w.find('input[data-test="credential-auto-toggle"]')
    expect((toggle.element as HTMLInputElement).disabled).toBe(false)
    await toggle.setValue(true)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('vault', 'credential_set', { enabled: true }, 8000)
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('凭据免打扰已开启'))).toBe(true)
    expect((w.find('input[data-test="credential-auto-toggle"]').element as HTMLInputElement).checked).toBe(true)
  })

  it('切换失败：回滚勾选 + 错误 toast', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'credential_status') return Promise.resolve(okCredAuto())
      if (cmd === 'credential_set') return Promise.reject('config 写盘失败')
      return Promise.resolve({})
    })
    const w = await mountView()
    const toggle = w.find('input[data-test="credential-auto-toggle"]')
    expect((toggle.element as HTMLInputElement).disabled).toBe(false)
    await toggle.setValue(true)
    await flushPromises()
    expect((w.find('input[data-test="credential-auto-toggle"]').element as HTMLInputElement).checked).toBe(false)
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('保存失败'))).toBe(true)
  })
})

describe('VaultView 凭据使用统计（ABCD 批 B，vault.credential_stats）', () => {
  it('计数与记录表渲染；结果徽章按 outcome 染色文案', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'credential_stats') {
        return Promise.resolve({
          injected: true,
          counters: { auto: 2, preauth: 1, denied: 1 },
          records: [
            { ts_ms: 1763878000000, tool: 'exec', fields: 'env', aliases: 'env.PW104', target_summary: 'exec: deploy.sh', outcome: 'auto', source: 'credential_switch' },
            { ts_ms: 1763878100000, tool: 'web_fetch', fields: 'headers', aliases: 'openai/prod-key', target_summary: 'GET api.example.com', outcome: 'out_of_domain', source: 'domain' },
          ],
        })
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('vault', 'credential_stats', {}, 8000)
    expect(w.text()).toContain('域内预授权')
    expect(w.text()).toContain('拒绝/出域')
    expect(w.text()).toContain('env.PW104')
    expect(w.text()).toContain('exec: deploy.sh')
    expect(w.text()).toContain('开关免卡')
    expect(w.text()).toContain('出域硬拒')
    // 凭据真值面纪律：记录里只有元数据，本测试载荷也全为元数据。
  })

  it('记录为空：显示空态文案', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'credential_stats') return Promise.resolve({ injected: true, counters: { auto: 0, preauth: 0, denied: 0 }, records: [] })
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(w.text()).toContain('本次运行暂无凭据引用记录')
  })
})

describe('VaultView 域绑定管理（ABCD 批 C，vault.bindings/bind/unbind）', () => {
  it('加载拉 bindings 并按别名分组渲染', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'bindings') return Promise.resolve({
        path: 'C:\\ws\\config\\config.vault_domain.json',
        bindings: { 'env.PW104': [{ tool: 'exec', slot: 'env' }] },
      })
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('vault', 'bindings', {}, 8000)
    expect(w.text()).toContain('env.PW104')
    expect(w.text()).toContain('exec : env')
  })

  it('添加绑定：三字段必填校验 + bind 调用 + 刷新', async () => {
    const w = await mountView()
    // 空表单点添加 → 本地拒绝
    const addBtn = w.find('button[data-test="bind-btn"]')
    await addBtn.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter((c: any[]) => c[1] === 'bind').length).toBe(0)
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('必填'))).toBe(true)
    // 填表 → bind → 刷新回读
    const inputs = w.findAll('input.form-input').filter(i => ['别名（如 env.PW104）', '工具（如 exec）', '槽位（如 env）'].includes(i.attributes('placeholder') || ''))
    await inputs[0].setValue('env.PW104')
    await inputs[1].setValue('exec')
    await inputs[2].setValue('env')
    await addBtn.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('vault', 'bind', { alias: 'env.PW104', tool: 'exec', slot: 'env' }, 8000)
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('已绑定'))).toBe(true)
  })

  it('解除绑定：confirm 门 + unbind 调用', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'bindings') return Promise.resolve({ path: '', bindings: { 'env.PW104': [{ tool: 'exec', slot: 'env' }] } })
      return Promise.resolve({})
    })
    const w = await mountView()
    // 取消 confirm：不发 unbind
    confirmMock.mockReturnValue(false)
    await w.find('button[data-test="unbind-btn"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter((c: any[]) => c[1] === 'unbind').length).toBe(0)
    // 确认：发 unbind
    confirmMock.mockReturnValue(true)
    await w.find('button[data-test="unbind-btn"]').trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('vault', 'unbind', { alias: 'env.PW104', tool: 'exec', slot: 'env' }, 8000)
    expect(useToast().toasts.some(t => t.type === 'success' && t.message.includes('已解除绑定'))).toBe(true)
  })
})
