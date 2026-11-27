// 地址注册表单 busy 闸回归（BUG-20261127-014）：输入框 @keyup.enter 不经
// 按钮 :disabled——此前 addAddress 无函数内 busy 闸，快速连按 Enter 并发
// 双发 addr_add（VaultView runUnlockTest 同形态先例有闸）。loaded:true
// 在场形态（空态卡形态归 MailboxView.emptystate.spec.ts 专属）。
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))
vi.mock('../../composables/useSSE', () => ({
  on: vi.fn(),
  off: vi.fn(),
}))

import MailboxView from '../MailboxView.vue'

beforeEach(() => {
  requestMock.mockReset()
})

function loadedMock(addDeferred?: { resolve: (v: any) => void; calls: any[] }) {
  requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
    if (cmd === 'status') return Promise.resolve({ loaded: true, smtp: { running: false }, pop3: { running: false } })
    if (cmd === 'stats') return Promise.resolve({ mailboxes: [] })
    if (cmd === 'addr_list') return Promise.resolve({ addresses: [], default: '' })
    if (cmd === 'list') return Promise.resolve({ items: [] })
    if (cmd === 'addr_add') {
      addDeferred?.calls.push(data)
      if (addDeferred) return new Promise((res) => { addDeferred.resolve = res })
      return Promise.resolve({})
    }
    return Promise.resolve({})
  })
}

async function mountLoaded(mock = loadedMock) {
  mock()
  const w = mount(MailboxView)
  await flushPromises()
  return w
}

describe('MailboxView 地址注册表单（BUG-20261127-014）', () => {
  it('Enter 双发受 busy 闸：在飞期间再按 Enter 不重复发 addr_add', async () => {
    const d = { resolve: (_: any) => {}, calls: [] as any[] }
    const w = await mountLoaded(() => loadedMock(d))
    // 打开注册地址模态（头部「＋ 注册地址」按钮）。
    const openBtn = w.findAll('button').find((b) => b.text().includes('注册地址'))!
    await openBtn.trigger('click')
    await flushPromises()
    const input = w.find('.addr-modal input')
    expect(input.exists()).toBe(true)
    await input.setValue('a@corp.example')
    // 第一发在飞；紧接着第二发 Enter——撞函数内 busy 闸。
    await input.trigger('keyup.enter')
    await input.trigger('keyup.enter')
    expect(d.calls, '连按 Enter 只发一次 addr_add').toHaveLength(1)
    expect(d.calls[0]).toEqual({ address: 'a@corp.example' })
    d.resolve({ ok: true })
    await flushPromises()
    expect(d.calls).toHaveLength(1)
    // 成功后模态关闭（既有行为 sanity：addrInput 清空随 addrOpen=false 收场）。
    expect(w.find('.addr-modal').exists()).toBe(false)
    w.unmount()
  })
})
