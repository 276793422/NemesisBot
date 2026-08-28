import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// G7 (D2/D3)：UserlandSandbox 直挂测试（不经 SandboxView 门控 —— 组件源码
// 始终参与 vitest，构建期 tree-shake 只影响产物）。覆盖：
// 后端探测表渲染、五个向上 emit、自检三形态（结果表判定语义 / 不支持 /
// 请求失败）、D3 安装指引折叠行。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import UserlandSandbox from '../UserlandSandbox.vue'

const baseProps = {
  busy: null as string | null,
  executorOn: true,
  strictOn: false,
  allowNetwork: false,
  sandboxReady: true,
  backends: [
    { name: 'landlock', form: 'SelfApply', availability: 'full', detail: [] },
    { name: 'bwrap', form: 'WrapCommand', availability: 'none', detail: ['未安装'] },
  ],
  selectedBackend: 'landlock' as string | null,
  strictHint: '当前后端：✅ landlock 可用 — 严格模式可兑现',
}

function threeChecks(over: Array<Record<string, unknown>> = []) {
  const seed = [
    { name: 'workspace 外写入（系统临时目录）', blocked: true, evidence: '写入被拒: EPERM' },
    { name: '网络出站（TCP 1.1.1.1:80）', blocked: false, evidence: '连接成功（landlock 不覆盖网络）' },
    { name: 'workspace 内写入（对照组）', blocked: false, evidence: '写入成功（工作区可写 — 符合预期）' },
  ]
  return seed.map((c, i) => ({ ...c, ...over[i] }))
}

beforeEach(() => {
  requestMock.mockReset()
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'self_test') {
      return Promise.resolve({
        supported: true,
        backend: 'landlock',
        form: 'self_apply',
        allow_network: false,
        probe_ok: true,
        checks: threeChecks(),
      })
    }
    return Promise.resolve({})
  })
})

async function mountComp(props: Record<string, unknown> = {}) {
  const w = mount(UserlandSandbox, { props: { ...baseProps, ...props } })
  await flushPromises()
  return w
}

describe('UserlandSandbox 探测表与事件', () => {
  it('后端探测表：名称/形态/可用性/detail/已选用标记', async () => {
    const w = await mountComp()
    expect(w.text()).toContain('landlock')
    expect(w.text()).toContain('进程内自装')
    expect(w.text()).toContain('包装命令')
    expect(w.text()).toContain('可用')
    expect(w.text()).toContain('不可用')
    expect(w.text()).toContain('未安装')
    expect(w.text()).toContain('✓ 已选用')
    expect(w.text()).toContain('实际选用：')
    // 空探测列表 → 诚实空态
    const empty = await mountComp({ backends: [], selectedBackend: null })
    expect(empty.text()).toContain('本平台无用户态沙盒后端')
  })

  it('五个 emit：refresh / enable-exec / disable-exec / toggle-network / toggle-strict', async () => {
    const w = await mountComp()
    await w.find('[data-test="userland-refresh"]').trigger('click')
    await w.findAll('button').find(b => b.text() === '停用沙盒执行')!.trigger('click')
    await w.findAll('button').find(b => b.text() === '已关闭')!.trigger('click')
    const strictBtn = w.findAll('button').filter(b => b.text() === '已关闭')[1]!
    await strictBtn.trigger('click')
    expect(w.emitted('refresh')).toHaveLength(1)
    expect(w.emitted('disable-exec')).toHaveLength(1)
    expect(w.emitted('toggle-network')).toHaveLength(1)
    expect(w.emitted('toggle-strict')).toHaveLength(1)

    const off = await mountComp({ executorOn: false })
    await off.findAll('button').find(b => b.text() === '启用沙盒执行')!.trigger('click')
    expect(off.emitted('enable-exec')).toHaveLength(1)
  })
})

describe('UserlandSandbox 自检（D2）', () => {
  it('supported:true → 元信息 + 三探针判定语义（隔离以拦截为目标，对照组以允许为目标）', async () => {
    const w = await mountComp()
    await w.find('[data-test="run-selftest"]').trigger('click')
    await flushPromises()

    expect(requestMock.mock.calls.find(c => c[1] === 'self_test')).toBeTruthy()
    expect(w.find('[data-test="selftest-checks"]').exists()).toBe(true)
    const rows = w.findAll('[data-test="selftest-checks"] tbody tr')
    expect(rows).toHaveLength(3)
    // 隔离探针 blocked=true → ✅ 已拦截
    expect(rows[0].text()).toContain('✅ 已拦截')
    // 隔离探针 blocked=false（landlock 网络缺口）→ ⚠️ 允许
    expect(rows[1].text()).toContain('⚠️ 允许')
    // 对照组 blocked=false → ✅ 正常
    expect(rows[2].text()).toContain('✅ 正常')
    // 元信息
    expect(w.text()).toContain('landlock')
    expect(w.text()).toContain('子进程内自装')
    expect(w.text()).toContain('探针进程正常退出')
    // 证据原文如实展示
    expect(w.text()).toContain('EPERM')
  })

  it('对照组被拦 = 异常（红）；隔离未生效 = 警告（amber）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'self_test') {
        return Promise.resolve({
          supported: true,
          backend: 'bwrap',
          form: 'wrap_command',
          allow_network: true,
          probe_ok: true,
          checks: threeChecks([
            { blocked: false, evidence: '写入成功（未沙盒 / 隔离未生效）' },
            { blocked: false, evidence: '连接成功' },
            { blocked: true, evidence: '写入失败（异常）' },
          ]),
        })
      }
      return Promise.resolve({})
    })
    const w = await mountComp()
    await w.find('[data-test="run-selftest"]').trigger('click')
    await flushPromises()
    const rows = w.findAll('[data-test="selftest-checks"] tbody tr')
    expect(rows[0].text()).toContain('⚠️ 允许')
    expect(rows[2].text()).toContain('❌ 异常')
    expect(w.text()).toContain('包装命令')
  })

  it('supported:false → 不支持说明（Windows / 无后端），无结果表', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'self_test') {
        return Promise.resolve({ supported: false, backend: null, checks: [], note: 'Windows 由 Sandboxie（内核强制）承担沙盒，无用户态后端可自检' })
      }
      return Promise.resolve({})
    })
    const w = await mountComp()
    await w.find('[data-test="run-selftest"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-test="selftest-unsupported"]').exists()).toBe(true)
    expect(w.text()).toContain('Sandboxie')
    expect(w.find('[data-test="selftest-checks"]').exists()).toBe(false)
  })

  it('probe_ok:false（探针进程异常）→ 报错行展示', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'self_test') {
        return Promise.resolve({ supported: true, backend: 'bwrap', form: 'wrap_command', allow_network: false, probe_ok: false, error: 'no JSON verdict (exit 127)', checks: [] })
      }
      return Promise.resolve({})
    })
    const w = await mountComp()
    await w.find('[data-test="run-selftest"]').trigger('click')
    await flushPromises()
    expect(w.text()).toContain('探针进程异常')
    expect(w.text()).toContain('exit 127')
    expect(w.find('[data-test="selftest-checks"]').exists()).toBe(false)
  })

  it('self_test 请求失败 → 错误文案，不崩', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'self_test') return Promise.reject(new Error('网关超时'))
      return Promise.resolve({})
    })
    const w = await mountComp()
    await w.find('[data-test="run-selftest"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-test="selftest-error"]').exists()).toBe(true)
    expect(w.text()).toContain('网关超时')
  })
})

describe('UserlandSandbox 安装指引（D3）', () => {
  it('折叠行常驻：bwrap / landlock / seatbelt 三条指引', async () => {
    const w = await mountComp()
    const guide = w.find('[data-test="install-guide"]')
    expect(guide.exists()).toBe(true)
    expect(guide.text()).toContain('sudo apt install bubblewrap')
    expect(guide.text()).toContain('内核 ≥ 5.13')
    expect(guide.text()).toContain('macOS 内置')
    // details 默认折叠（内容在 summary 之外但仍渲染于 DOM——text() 可断言内容存在）
  })
})
