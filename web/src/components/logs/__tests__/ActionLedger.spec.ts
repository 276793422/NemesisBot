import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

// 行动账本页内撤销（E3 收官批 2026-10-15；R5 诚实化 2026-10-16）dispatch 级 spec：
// - 撤销按钮可见性：只对 executed + write 类凭条开放；denied/read/network
//   不显示（无效果可撤 / 无通用撤销语义）；
// - 二次确认模态（带凭条信息；域 C/域 B 说明分支——域 B 明示外部副作用不会被撤销）；
// - 确认 → connectors.undo 带 receipt_id + session_key；undone=true → 成功
//   toast + 凭条重拉；
// - R5 三臂：undone=false + reverse_op → error toast（带 reason）；undone=false
//   + local_rewind → warn toast（外部副作用未被撤销）；绝不在 undone=false 时
//   显示「撤销完成」；
// - 请求失败 → error toast，模态退 busy 可重试/取消。
// 后端分派语义（read 诚实说明 / 域 C 反向 op 结构化判定 / 域 B local_rewind /
// registry 按当下裁决）由 crates/nemesis-web/src/handlers/connectors/tests.rs 钉住。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import ActionLedger from '../ActionLedger.vue'

function receipt(over: Record<string, unknown> = {}) {
  return {
    receipt_id: 'RCPT-1',
    ts: '2026-10-15T08:00:00Z',
    connector: 'paper-broker',
    op: 'place_order',
    risk: 'critical',
    class: 'write',
    outcome: 'executed',
    args_digest: 'd'.repeat(32),
    summary: '买入 100 股 @1500',
    undoable: true,
    session_key: 'web:u1',
    ...over,
  }
}

function defaultImpl() {
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'receipts.list') {
      return Promise.resolve({
        receipts: [
          receipt(), // executed + write + undoable → 撤销按钮（域 C）
          receipt({ receipt_id: 'RCPT-2', undoable: false }), // executed + write 无 undo 声明 → 撤销按钮（域 B）
          receipt({ receipt_id: 'RCPT-3', outcome: 'denied_approval' }), // 未生效 → 无按钮
          receipt({ receipt_id: 'RCPT-4', op: 'quote', class: 'read', undoable: false }), // read → 无按钮
          receipt({ receipt_id: 'RCPT-5', op: 'ping', class: 'network', undoable: false }), // network → 无按钮
        ],
      })
    }
    // R5：撤销成功响应 = undone:true（后端按行动账本新凭条结构化判定）。
    if (cmd === 'undo') {
      return Promise.resolve({ undone: true, via: 'reverse_op', capability: 'reverse_op', undo_op: 'cancel_order' })
    }
    return Promise.resolve({})
  })
}

beforeEach(() => {
  requestMock.mockReset()
  defaultImpl()
  useToast().toasts.splice(0)
})

async function mountView() {
  const w = mount(ActionLedger)
  await flushPromises()
  return w
}

describe('ActionLedger 页内撤销（E3）', () => {
  it('撤销按钮只出现在 executed+write 行；denied/read/network 行不显示', async () => {
    const w = await mountView()
    const rows = w.findAll('[data-test="ledger-row"]')
    expect(rows.length).toBe(5)
    const btns = w.findAll('[data-test="ledger-undo-btn"]')
    expect(btns.length).toBe(2)
    // 按钮落在前两行（域 C / 域 B）
    expect(rows[0].find('[data-test="ledger-undo-btn"]').exists()).toBe(true)
    expect(rows[1].find('[data-test="ledger-undo-btn"]').exists()).toBe(true)
    expect(rows[2].find('[data-test="ledger-undo-btn"]').exists()).toBe(false)
    expect(rows[3].find('[data-test="ledger-undo-btn"]').exists()).toBe(false)
    expect(rows[4].find('[data-test="ledger-undo-btn"]').exists()).toBe(false)
    w.unmount()
  })

  it('点击撤销 → 二次确认模态带凭条信息；undoable=true 显示反向 op 说明；取消关闭且不发 undo 请求', async () => {
    const w = await mountView()
    await w.findAll('[data-test="ledger-undo-btn"]')[0].trigger('click')
    const modal = w.find('[data-test="ledger-undo-modal"]')
    expect(modal.exists()).toBe(true)
    expect(modal.text()).toContain('paper-broker.place_order')
    expect(modal.text()).toContain('反向')
    await modal.find('[data-test="ledger-undo-cancel"]').trigger('click')
    expect(w.find('[data-test="ledger-undo-modal"]').exists()).toBe(false)
    expect(requestMock.mock.calls.map(c => c[1])).not.toContain('undo')
    w.unmount()
  })

  it('域 B（undoable=false）模态明示：回退本地会话，外部副作用不会被撤销（R5 诚实文案）', async () => {
    const w = await mountView()
    await w.findAll('[data-test="ledger-undo-btn"]')[1].trigger('click')
    const modal = w.find('[data-test="ledger-undo-modal"]')
    expect(modal.text()).toContain('回退本地会话最后一轮')
    expect(modal.text()).toContain('外部副作用不会被撤销')
    w.unmount()
  })

  it('确认 → connectors.undo 带 receipt_id + session_key；undone=true → 成功 toast + 凭条重拉', async () => {
    const w = await mountView()
    const listCallsBefore = requestMock.mock.calls.filter(c => c[1] === 'receipts.list').length
    await w.findAll('[data-test="ledger-undo-btn"]')[0].trigger('click')
    await w.find('[data-test="ledger-undo-confirm"]').trigger('click')
    await flushPromises()

    const undoCall = requestMock.mock.calls.find(c => c[1] === 'undo')
    expect(undoCall).toBeTruthy()
    expect(undoCall![0]).toBe('connectors')
    expect(undoCall![2]).toEqual({ receipt_id: 'RCPT-1', session_key: 'web:u1' })

    const toasts = useToast().toasts
    expect(toasts.some(t => t.type === 'success' && t.message.includes('撤销完成'))).toBe(true)
    expect(w.find('[data-test="ledger-undo-modal"]').exists()).toBe(false)

    const listCallsAfter = requestMock.mock.calls.filter(c => c[1] === 'receipts.list').length
    expect(listCallsAfter).toBe(listCallsBefore + 1)
    w.unmount()
  })

  it('undo 失败 → error toast；模态退 busy 可取消', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'receipts.list') return Promise.resolve({ receipts: [receipt()] })
      if (cmd === 'undo') return Promise.reject(new Error('op `cancel_order` 已从 connector `paper-broker` 清单移除'))
      return Promise.resolve({})
    })
    const w = await mountView()
    await w.find('[data-test="ledger-undo-btn"]').trigger('click')
    await w.find('[data-test="ledger-undo-confirm"]').trigger('click')
    await flushPromises()

    const toasts = useToast().toasts
    expect(toasts.some(t => t.type === 'error' && t.message.includes('清单移除'))).toBe(true)
    // 模态保留（busy 已退），可取消退出
    const modal = w.find('[data-test="ledger-undo-modal"]')
    expect(modal.exists()).toBe(true)
    expect((modal.find('[data-test="ledger-undo-confirm"]').element as HTMLButtonElement).disabled).toBe(false)
    await modal.find('[data-test="ledger-undo-cancel"]').trigger('click')
    expect(w.find('[data-test="ledger-undo-modal"]').exists()).toBe(false)
    w.unmount()
  })

  // R5 三臂：undone=false 绝不显示「撤销完成」。
  it('R5: undone=false + reverse_op → error toast 带 reason，绝无「撤销完成」', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'receipts.list') return Promise.resolve({ receipts: [receipt()] })
      if (cmd === 'undo') {
        return Promise.resolve({
          undone: false, via: 'reverse_op', capability: 'reverse_op', undo_op: 'cancel_order',
          reason: '反向 op 凭条 outcome=`error`（未真实生效，外部状态未撤销）',
        })
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    await w.find('[data-test="ledger-undo-btn"]').trigger('click')
    await w.find('[data-test="ledger-undo-confirm"]').trigger('click')
    await flushPromises()

    const toasts = useToast().toasts
    expect(toasts.some(t => t.type === 'error' && t.message.includes('未真实生效'))).toBe(true)
    expect(toasts.some(t => t.message.includes('撤销完成'))).toBe(false)
    w.unmount()
  })

  it('R5: undone=false + local_rewind → warn toast（外部副作用未被撤销），绝无「撤销完成」', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'receipts.list') return Promise.resolve({ receipts: [receipt({ undoable: false })] })
      if (cmd === 'undo') {
        return Promise.resolve({
          undone: false, via: 'local_rewind', capability: 'local_rewind',
          reason: '外部副作用未被撤销，仅本地会话回退',
        })
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    await w.find('[data-test="ledger-undo-btn"]').trigger('click')
    await w.find('[data-test="ledger-undo-confirm"]').trigger('click')
    await flushPromises()

    const toasts = useToast().toasts
    expect(toasts.some(t => t.type === 'warn' && t.message.includes('外部副作用未被撤销'))).toBe(true)
    expect(toasts.some(t => t.message.includes('撤销完成'))).toBe(false)
    w.unmount()
  })
})
