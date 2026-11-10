import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { createRouter, createMemoryHistory } from 'vue-router'
import { useToast } from '../../composables/useToast'

// ③ ABAC 访问控制专页 spec（十层页面化 2026-11-10）：六族规则表行级编辑
// ——族/子类目 tab 切换、行增删、整档 config.save 写回（comment 保留）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import SecurityAbacView from '../SecurityAbacView.vue'

function configResult() {
  return {
    default_action: 'deny',
    file_rules: {
      read: [{ pattern: 'C:\\Projects\\**', action: 'allow', comment: '项目目录可读' }],
      write: [],
      delete: [],
    },
    process_rules: {
      exec: [
        { pattern: 'cargo *', action: 'ask', comment: '' },
        { pattern: 'format', action: 'deny', comment: '格式化' },
      ],
      spawn: [],
      kill: [],
      suspend: [],
    },
  }
}

function makeRouter() {
  return createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', component: { template: '<div />' } },
      { path: '/settings', component: { template: '<div />' } },
    ],
  })
}

async function mountView(cfg: any = configResult()) {
  requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
    if (cmd === 'config.get') return Promise.resolve(cfg)
    if (cmd === 'config.save') return Promise.resolve({ saved: true })
    if (cmd === 'overview') return Promise.resolve({ editor: { injected: true, full_access: true, external_write: true } })
    return Promise.resolve({})
  })
  const w = mount(SecurityAbacView, { global: { plugins: [makeRouter()] } })
  await flushPromises()
  return w
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
})

describe('SecurityAbacView 六族规则表', () => {
  it('默认渲染 file·read 桶：1 条规则 + 计数徽标；默认姿态 deny 显示', async () => {
    const w = await mountView()
    const rows = w.find('[data-test="abac-rules-table"]').findAll('tbody tr')
    expect(rows).toHaveLength(1)
    expect((rows[0].find('input').element as HTMLInputElement).value).toBe('C:\\Projects\\**')
    expect(w.find('[data-test="abac-tab-file_rules-read"]').classes()).toContain('active')
    expect(w.find('[data-test="abac-tab-file_rules-read"]').text()).toContain('1')
    expect(w.text()).toContain('默认姿态 deny')
    w.unmount()
  })

  it('tab 切换到 process·exec：2 条规则 + action 归一显示（ask）', async () => {
    const w = await mountView()
    await w.find('[data-test="abac-tab-process_rules-exec"]').trigger('click')
    await flushPromises()
    const rows = w.find('[data-test="abac-rules-table"]').findAll('tbody tr')
    expect(rows).toHaveLength(2)
    const action = rows[0].find('select')
    expect((action.element as HTMLSelectElement).value).toBe('ask')
    w.unmount()
  })

  it('+ 加规则 → 本地行出现；保存 → 整档 config.save（含新行 + comment 原样）', async () => {
    const w = await mountView()
    await w.find('[data-test="abac-rule-add"]').trigger('click')
    await flushPromises()
    let rows = w.find('[data-test="abac-rules-table"]').findAll('tbody tr')
    expect(rows).toHaveLength(2)
    // 填新行 pattern
    await rows[1].findAll('input')[0].setValue('D:\\Docs\\**')
    requestMock.mockClear()
    await w.find('[data-test="abac-save"]').trigger('click')
    await flushPromises()
    const saveCalls = requestMock.mock.calls.filter(c => c[1] === 'config.save')
    expect(saveCalls.length).toBe(1)
    const payload = saveCalls[0][2]
    expect(payload.file_rules.read).toHaveLength(2)
    expect(payload.file_rules.read[0].comment).toBe('项目目录可读') // comment 不丢
    expect(payload.file_rules.read[1].pattern).toBe('D:\\Docs\\**')
    expect(useToast().toasts.some(t => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('删行 → 本地移除；保存载荷不含该行', async () => {
    const w = await mountView()
    await w.find('[data-test="abac-tab-process_rules-exec"]').trigger('click')
    await flushPromises()
    const removeBtns = w.find('[data-test="abac-rules-table"]').findAll('[data-test="abac-rule-remove"]')
    await removeBtns[1].trigger('click')
    await flushPromises()
    requestMock.mockClear()
    await w.find('[data-test="abac-save"]').trigger('click')
    await flushPromises()
    const payload = requestMock.mock.calls.find(c => c[1] === 'config.save')![2]
    expect(payload.process_rules.exec).toHaveLength(1)
    expect(payload.process_rules.exec[0].pattern).toBe('cargo *')
    w.unmount()
  })

  it('保存失败 → error toast + 重新拉取 config', async () => {
    const w = await mountView()
    requestMock.mockImplementation((_m: string, cmd: string, _d: any) => {
      if (cmd === 'config.save') return Promise.reject(new Error('ws down'))
      if (cmd === 'config.get') return Promise.resolve(configResult())
      return Promise.resolve({})
    })
    await w.find('[data-test="abac-save"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some(t => t.type === 'error')).toBe(true)
    // 行数回真（重拉后 file·read 仍 1 行）
    const rows = w.find('[data-test="abac-rules-table"]').findAll('tbody tr')
    expect(rows).toHaveLength(1)
    w.unmount()
  })

  it('Full Access 只读卡呈现 overview editor 快照', async () => {
    const w = await mountView()
    const card = w.find('[data-test="editor-flags-card"]')
    expect(card.exists()).toBe(true)
    expect(card.text()).toContain('已开启')
    w.unmount()
  })
})
