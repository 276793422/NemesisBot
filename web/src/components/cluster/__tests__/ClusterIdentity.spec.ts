// 职能框架 M6：ClusterIdentity 的职能/档位编辑契约。
// - nodes.list 本机行预填 professions/tier；
// - 目录按钮 toggle 选中态（btn-primary）；
// - 保存 payload 携带 professions 数组 + tier（空 = null，auto）。
// 后端归一/校验/peers.toml 持久化由 handlers/cluster.rs node.update_identity
// 钉住；本 spec 只钉编辑面。
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import ClusterIdentity from '../ClusterIdentity.vue'

const LOCAL_NODE = {
  id: 'node-self',
  name: 'Self',
  role: 'worker',
  isLocal: true,
  professions: ['dev'],
  tier: 'normal',
}

function mockBackend() {
  requestMock.mockImplementation(async (_m: string, cmd: string) => {
    if (cmd === 'config.get')
      return {
        node_id: 'node-self',
        name: 'Self',
        role: 'worker',
        category: 'development',
        tags: [],
        capabilities: [],
      }
    if (cmd === 'nodes.list') return { nodes: [LOCAL_NODE] }
    return {}
  })
}

async function mountEditor() {
  mockBackend()
  const w = mount(ClusterIdentity)
  await flushPromises()
  return w
}

function profButton(w: ReturnType<typeof mount>, slug: string) {
  return w.findAll('button').find((b) => b.text().includes(`（${slug}）`))!
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
})

describe('ClusterIdentity 职能/档位编辑（M6）', () => {
  it('nodes.list 本机行预填：已声明职能按钮选中 + 当前声明徽标 + 档位回显', async () => {
    const w = await mountEditor()
    const devBtn = profButton(w, 'dev')
    expect(devBtn.classes()).toContain('btn-primary')
    // 未声明的职能不选中。
    expect(profButton(w, 'architecture').classes()).not.toContain('btn-primary')
    // 当前声明徽标。
    expect(w.text()).toContain('当前声明')
    // 档位回显 normal（select value）。
    const tierSelect = w.findAll('select').find((s) => (s.element as HTMLSelectElement).value === 'normal')
    expect(tierSelect).toBeDefined()
    w.unmount()
  })

  it('toggle 职能 + 改档位后保存：node.update_identity payload 携带 professions + tier', async () => {
    const w = await mountEditor()
    // 加选 architecture（big 档职能）。
    await profButton(w, 'architecture').trigger('click')
    // 档位改 big。
    const tierSelect = w.findAll('select').find((s) => (s.element as HTMLSelectElement).value === 'normal')!
    await tierSelect.setValue('big')
    // 保存。
    const saveBtn = w.findAll('button').find((b) => b.text().includes('更新身份'))!
    await saveBtn.trigger('click')
    await flushPromises()

    const call = requestMock.mock.calls.find((c) => c[1] === 'node.update_identity')
    expect(call).toBeDefined()
    const payload = call![2]
    // 原有 dev 保留 + 新增 architecture（预填不被覆盖）。
    expect(payload.professions).toEqual(['dev', 'architecture'])
    expect(payload.tier).toBe('big')
    // 身份字段照常携带。
    expect(payload.name).toBe('Self')
    expect(payload.role).toBe('worker')
    w.unmount()
  })

  it('档位留空 = auto：payload tier 为 null', async () => {
    const w = await mountEditor()
    const tierSelect = w.findAll('select').find((s) => (s.element as HTMLSelectElement).value === 'normal')!
    await tierSelect.setValue('')
    const saveBtn = w.findAll('button').find((b) => b.text().includes('更新身份'))!
    await saveBtn.trigger('click')
    await flushPromises()

    const call = requestMock.mock.calls.find((c) => c[1] === 'node.update_identity')!
    expect(call[2].tier).toBeNull()
    w.unmount()
  })
})
