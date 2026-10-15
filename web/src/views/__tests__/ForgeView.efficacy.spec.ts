// F 线实效展示契约（自进化升级 2026-10-14）：
// registry artifacts 携带 last_efficacy（monitor 评估写回）→ Artifacts 表
// 「实效（部署前→后）」列渲染 verdict 文案 + before→after 百分比；
// 未评估（None）= '--' 诚实缺省。
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import ForgeView from '../ForgeView.vue'

function artifact(over: Record<string, unknown> = {}) {
  return {
    id: 'a1',
    name: 'skill-demo',
    kind: 'Skill',
    status: 'Active',
    version: '1',
    usage_count: 5,
    success_rate: 0.8,
    updated_at: '2026-10-14T00:00:00+08:00',
    ...over,
  }
}

function mountForge() {
  return mount(ForgeView, {
    global: {
      stubs: {
        // A/C 线子面板不在本 spec 范围（各自有 spec）。
        BehaviorTestPanel: { template: '<div />' },
        ReplayPanel: { template: '<div />' },
      },
    },
  })
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation(async (_m: string, cmd: string) => {
    if (cmd === 'status') return { enabled: true, running: true }
    if (cmd === 'stats') return { enabled: true }
    if (cmd === 'registry.list') {
      return {
        artifacts: [
          artifact({
            last_efficacy: {
              measured_at: '2026-10-14T00:00:00+08:00',
              success_before: 0.5,
              success_after: 0.8,
              samples_before: 10,
              samples_after: 10,
              verdict: 'positive',
            },
          }),
          artifact({ id: 'a2', name: 'skill-unevaluated' }),
        ],
        skill_directories: [],
      }
    }
    return {}
  })
})

async function openArtifactsTab(w: ReturnType<typeof mount>) {
  const tab = w.findAll('.tab').find((t) => t.text() === 'Artifacts')
  expect(tab, 'Artifacts tab exists').toBeTruthy()
  await tab!.trigger('click')
  await flushPromises()
}

describe('ForgeView 实效展示（F 线）', () => {
  it('切到 Artifacts 表：有 last_efficacy 渲染 verdict + before→after', async () => {
    const w = mountForge()
    await flushPromises()
    await openArtifactsTab(w)
    const eff0 = w.find('[data-testid="efficacy-0"]')
    expect(eff0.exists()).toBe(true)
    expect(eff0.text()).toContain('正向')
    expect(eff0.text()).toContain('50% → 80%')
    w.unmount()
  })

  it('未评估 artifact → -- 诚实缺省', async () => {
    const w = mountForge()
    await flushPromises()
    await openArtifactsTab(w)
    expect(w.find('[data-testid="efficacy-1"]').text()).toContain('--')
    w.unmount()
  })
})
