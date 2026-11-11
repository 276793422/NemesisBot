import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
  initWSAPI: vi.fn(),
}))
vi.mock('../../composables/useSSE', () => ({ on: vi.fn(), off: vi.fn() }))

const authedFetchMock = vi.fn()
vi.mock('../../lib/authFetch', () => ({
  authedFetch: (...args: any[]) => authedFetchMock(...args),
}))

import AboutView from '../AboutView.vue'

const FEATURES = [
  { id: 'voice', label: '语音', desc: 'STT/TTS', category: 'subsystems', default: false, enabled: true },
  { id: 'channels-web', label: 'Web 通道', desc: '', category: 'channels', default: true, enabled: true },
  { id: 'cluster', label: '集群', desc: '', category: 'subsystems', default: false, enabled: false },
]

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation(() => Promise.resolve({}))
  authedFetchMock.mockReset()
})

async function mountView() {
  const w = mount(AboutView)
  await flushPromises()
  return w
}

async function goto(w: ReturnType<typeof mount>, tab: string) {
  await w.findAll('button.tab').find(b => b.text() === tab)!.trigger('click')
  await flushPromises()
}

describe('AboutView 关于页签', () => {
  it('加载版本号；失败保留 --', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'version' ? Promise.resolve({ version: '9.9.9' }) : Promise.resolve({}))
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('system', 'version')
    expect(w.text()).toContain('9.9.9')

    requestMock.mockImplementation((_m, cmd) => cmd === 'version' ? Promise.reject(new Error('x')) : Promise.resolve({}))
    const w2 = await mountView()
    expect(w2.text()).toContain('--')
  })

  it('许可链接存在', async () => {
    const w = await mountView()
    expect(w.text()).toContain('查看许可与使用限制')
  })
})

describe('AboutView 构建形态页签', () => {
  it('加载 features 并按 channels/subsystems 分组渲染', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'features' ? Promise.resolve({ features: FEATURES }) : Promise.resolve({}))
    const w = await mountView()
    await goto(w, '构建形态')
    expect(requestMock).toHaveBeenCalledWith('system', 'features')
    expect(w.text()).toContain('语音')
    expect(w.text()).toContain('Web 通道')
    const vm = w.vm as any
    expect(vm.channelFeatures.length).toBe(1)
    expect(vm.subsystemFeatures.length).toBe(2)
    expect(vm.enabledCount).toBe(2)
  })

  it('features 空列表与失败分支；失败后可重试', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'features' ? Promise.reject(new Error('offline')) : Promise.resolve({}))
    const w = await mountView()
    await goto(w, '构建形态')
    expect((w.vm as any).buildFeaturesError).toContain('offline')
    expect((w.vm as any).buildFeaturesLoaded).toBe(false)

    requestMock.mockImplementation((_m, cmd) => cmd === 'features' ? Promise.resolve({ features: FEATURES }) : Promise.resolve({}))
    await (w.vm as any).loadBuildFeatures()
    await flushPromises()
    expect((w.vm as any).buildFeaturesLoaded).toBe(true)
    expect((w.vm as any).buildFeatures.length).toBe(3)
  })

  it('已加载后切页签不重复拉取', async () => {
    requestMock.mockImplementation((_m, cmd) => cmd === 'features' ? Promise.resolve({ features: FEATURES }) : Promise.resolve({}))
    const w = await mountView()
    await goto(w, '构建形态')
    const calls = requestMock.mock.calls.filter(c => c[1] === 'features').length
    await goto(w, '关于')
    await goto(w, '构建形态')
    expect(requestMock.mock.calls.filter(c => c[1] === 'features').length).toBe(calls)
  })
})

describe('AboutView Readme 页签', () => {
  it('authedFetch 成功渲染 markdown；HTTP 错误提示；缓存守卫', async () => {
    authedFetchMock.mockResolvedValue({ ok: true, json: () => Promise.resolve({ content: '# Hello' }) })
    const w = await mountView()
    await goto(w, 'Readme')
    expect(authedFetchMock).toHaveBeenCalledWith('/api/system/readme')
    expect(w.text()).toContain('Hello')

    // 已有内容不再拉取
    const calls = authedFetchMock.mock.calls.length
    await (w.vm as any).loadReadme()
    expect(authedFetchMock.mock.calls.length).toBe(calls)
  })

  it('readme 加载失败显示错误', async () => {
    authedFetchMock.mockResolvedValue({ ok: false, status: 500 })
    const w = await mountView()
    await goto(w, 'Readme')
    expect((w.vm as any).readmeError).toBe('HTTP 500')

    authedFetchMock.mockRejectedValue(new Error('net down'))
    ;(w.vm as any).readmeContent = ''
    await (w.vm as any).loadReadme()
    expect((w.vm as any).readmeError).toBe('net down')
  })
})
