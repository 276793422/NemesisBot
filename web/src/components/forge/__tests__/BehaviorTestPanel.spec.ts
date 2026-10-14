// A 线行为回归面板契约（自进化升级 2026-10-14）：
// - forge.test 结果逐用例渲染（结论/失败明细/judge 分数）；
// - gate 布尔 → 部署闸徽标两态；
// - skipped（无 tests 段）→ 诚实跳过徽标，不渲染结果表。
import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../../composables/useToast'

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))

import BehaviorTestPanel from '../BehaviorTestPanel.vue'

function caseResult(over: Record<string, unknown> = {}) {
  return {
    name: '计划先行',
    passed: true,
    failures: [],
    transcript: {},
    ...over,
  }
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation(async (_m: string, cmd: string) => {
    if (cmd === 'registry.list') {
      return {
        skill_directories: [
          { name: 'weather', has_skill_md: true },
          { name: 'broken', has_skill_md: false },
        ],
      }
    }
    return {}
  })
})

describe('BehaviorTestPanel（A 线回归结果表）', () => {
  it('skill 下拉只列有 SKILL.md 的目录', async () => {
    const w = mount(BehaviorTestPanel)
    await flushPromises()
    const options = w.findAll('select option').map((o) => o.text())
    expect(options).toContain('weather')
    expect(options).not.toContain('broken')
    w.unmount()
  })

  it('gate=true → 部署闸通过徽标 + 逐用例结论行', async () => {
    const w = mount(BehaviorTestPanel)
    await flushPromises()
    requestMock.mockImplementation(async (_m: string, cmd: string, data: any) => {
      if (cmd === 'test') {
        expect(data.name).toBe('weather')
        return {
          skill: 'weather',
          passed: 1,
          failed: 0,
          gate: true,
          results: [caseResult()],
        }
      }
      return {}
    })
    await w.find('[data-testid="run-btn"]').trigger('click')
    await flushPromises()
    const gate = w.find('[data-testid="gate-badge"]')
    expect(gate.exists()).toBe(true)
    expect(gate.text()).toContain('部署闸通过')
    expect(w.find('[data-testid="case-row-计划先行"]').exists()).toBe(true)
    w.unmount()
  })

  it('gate=false → 部署闸不通过徽标 + 失败明细可见', async () => {
    const w = mount(BehaviorTestPanel)
    await flushPromises()
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'test') {
        return {
          skill: 'weather',
          passed: 0,
          failed: 1,
          gate: false,
          results: [
            caseResult({
              passed: false,
              failures: ['tools_called: expected tool \'present_plan\' was not called'],
            }),
          ],
        }
      }
      return {}
    })
    await w.find('[data-testid="run-btn"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="gate-badge"]').text()).toContain('部署闸不通过')
    expect(w.text()).toContain("tools_called: expected tool 'present_plan' was not called")
    w.unmount()
  })

  it('skipped（无 tests 段）→ 跳过徽标、不渲染结果表', async () => {
    const w = mount(BehaviorTestPanel)
    await flushPromises()
    requestMock.mockImplementation(async (_m: string, cmd: string) => {
      if (cmd === 'test') return { skipped: true, reason: 'no behavior tests declared' }
      return {}
    })
    await w.find('[data-testid="run-btn"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="skipped-badge"]').exists()).toBe(true)
    expect(w.find('[data-testid="gate-badge"]').exists()).toBe(false)
    w.unmount()
  })
})
