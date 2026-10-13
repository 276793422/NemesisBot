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
    const inputs = w.findAll('input.form-input')
    // 新增卡：别名 / value / 域 / 备注 四个输入（解锁卡还有 1 个 password）。
    await inputs.at(1)!.setValue('openai/prod-key')
    await inputs.at(2)!.setValue('sk-NEW-SECRET')
    await inputs.at(3)!.setValue('openai')
    await inputs.at(4)!.setValue('备注')
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
