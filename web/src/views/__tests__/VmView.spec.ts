import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// P5 第 4 步 ②：虚拟化（VM）管理页。后端语义由 bot 侧 vm_bridge +
// nemesis-web vm_ws_tests 钉住；本套件钉前端契约面——
// 探针/建议横幅渲染、总开关 confirm 门与默认关、config.set 全量回写、
// image.build update 旗标转发、实例 sleep/wake/kill 命令转发、
// B4 环境安装分步卡（分步态渲染 / 关闭态闸 / 命令展示与复制 / 安装转发）、
// B6 快速试玩卡（四态闸 / 启停转发 / exec 转发与输出渲染）。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import VmView from '../VmView.vue'

const confirmMock = vi.fn(() => true)

// 与后端 overview() 投影同构（vm_bridge.rs）。enabled=false（默认态铁律）。
function overview(over: Record<string, unknown> = {}) {
  return {
    enabled: false,
    config: {
      enabled: false,
      arch: 'x86_64',
      hypervisor: 'auto',
      desktop: 'minimal',
      image: {
        source: 'https://cloud.debian.org/images/cloud/bookworm/latest/debian-12-genericcloud-amd64.qcow2',
        sha256: '',
      },
      resources: { vcpu: 2, ram_mb: 2048, disk_gb: 20 },
      max_instances: 2,
      idle_sleep_secs: 300,
      gc: { max_total_disk_gb: 50, over_limit: 'warn' },
    },
    probe: { platform: 'windows', accel: 'whpx', available: true, layer: 'user', reason: null },
    sleep_line: 'snapshot',
    sleep_line_note: 'WHPX 快照线可用',
    suggestion: { enabled: true, note: 'WHPX 可用，可开启' },
    gc: { used_gb: 0, budget_gb: 50, decision: 'allow' },
    instances: [],
    qemu_binary: 'C:\\qemu\\qemu-system-x86_64.exe',
    images_root: 'C:\\ws\\tools\\vm\\images',
    ...over,
  }
}

function imageList(over: Record<string, unknown> = {}) {
  return {
    images_root: 'C:\\ws\\tools\\vm\\images',
    images: [{ name: 'base.qcow2', size_bytes: 350000000, modified_ms: 1730000000000 }],
    build: { running: false },
    log: { path: 'C:\\ws\\tools\\vm\\image-build.log', size_bytes: 12, tail: ['step1 ok', 'step2 ok'] },
    ...over,
  }
}

function instanceRow(over: Record<string, unknown> = {}) {
  return {
    agent_id: 'abc123def4567890',
    running: true,
    sleep_mode: null,
    asleep_at: null,
    arch: 'x86_64',
    accel: 'whpx',
    vcpu: 2,
    ram_mb: 2048,
    disk_gb: 20,
    has_snapshot: true,
    has_state_file: true,
    ...over,
  }
}

// 与后端 bootstrap_status() 投影同构（vm_bridge.rs）。默认全绿态收窄为
// absent/needs 组合时用 over 覆盖。enabled=false（默认态铁律）。
// image_route 默认 wsl_ready（platform=windows 的全绿路由态，B7）。
function bootstrapStatus(over: Record<string, unknown> = {}) {
  return {
    enabled: false,
    platform: 'windows',
    qemu: { state: 'ready', source: 'C:\\ws\\tools\\vm\\qemu' },
    accel: { state: 'ok' },
    image_ready: true,
    image_route: { state: 'wsl_ready', missing: [], command: '', note: '', guest_ready: true },
    install: { running: false },
    log: { path: 'C:\\ws\\tools\\vm\\bootstrap.log', tail: [] as string[] },
    ...over,
  }
}

// 与后端 try_status() 投影同构（vm_bridge.rs B6+热修）。默认 = 开启 +
// 镜像就绪 + 未运行未在途（试玩卡「就绪未运行」态——带启动按钮）。
function tryStatus(over: Record<string, unknown> = {}) {
  return {
    agent_id: 'try-dashboard',
    enabled: true,
    image_ready: true,
    arch: 'x86_64',
    arch_supported: true,
    running: false,
    starting: false,
    last_error: null,
    ...over,
  }
}

function mountView(
  ov: unknown = overview(),
  img: unknown = imageList(),
  bs: unknown = bootstrapStatus(),
  ts: unknown = tryStatus(),
) {
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'overview') return Promise.resolve(ov)
    if (cmd === 'image.list') return Promise.resolve(img)
    if (cmd === 'bootstrap.status') return Promise.resolve(bs)
    if (cmd === 'try.status') return Promise.resolve(ts)
    return Promise.resolve({})
  })
  const w = mount(VmView)
  // 两跳：flushPromises 的 scheduler 只让出一层微任务，refreshAll 的
  // overview→bootstrap.status→image.list 链式 await 需要多次才落定（SandboxView 套件同款）。
  return flushPromises().then(() => flushPromises()).then(() => flushPromises()).then(() => w)
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  vi.stubGlobal('confirm', confirmMock)
  confirmMock.mockReset().mockReturnValue(true)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('VmView 加载与探针横幅', () => {
  it('挂载拉 overview + image.list；探针/建议/睡眠线渲染；默认关', async () => {
    const w = await mountView()
    const cmds = requestMock.mock.calls.map((c) => c[1])
    expect(cmds).toContain('overview')
    expect(cmds).toContain('image.list')
    // 探针横幅（available → 绿点 + 加速器名；建议卡 + 睡眠线）
    expect(w.find('[data-test="probe-banner"]').exists()).toBe(true)
    expect(w.text()).toContain('加速器 WHPX 可用')
    expect(w.text()).toContain('WHPX 快照线可用')
    expect(w.find('[data-test="suggestion"]').text()).toContain('可开启')
    // 总开关默认态 = 已关闭（enabled=false）
    expect(w.find('[data-test="master-toggle"]').text()).toBe('已关闭')
    w.unmount()
  })

  it('加速器缺席：原因 + 建议关渲染', async () => {
    const w = await mountView(overview({
      probe: { platform: 'windows', accel: 'none', available: false, layer: null, reason: '未启用 WHPX 功能' },
      sleep_line: null,
      sleep_line_note: '无硬件加速，无快照线',
      suggestion: { enabled: false, note: '缺 WHPX，实例无法启动' },
    }))
    expect(w.text()).toContain('加速器 NONE 不可用')
    expect(w.text()).toContain('未启用 WHPX 功能')
    expect(w.find('[data-test="suggestion"]').text()).toContain('建议保持关闭')
    w.unmount()
  })

  it('overview 失败 → 诚实降级卡（不崩）', async () => {
    const w = await mountView(null, null)
    expect(w.text()).toContain('无法获取 VM 总览')
    w.unmount()
  })
})

describe('VmView 配置页', () => {
  it('config.set 全量回写 executor.vm 段（总开关开启走 confirm 门）', async () => {
    const w = await mountView()
    await w.find('[data-test="master-toggle"]').trigger('click')
    expect(confirmMock).toHaveBeenCalledTimes(1)
    await flushPromises()

    const call = requestMock.mock.calls.find((c) => c[1] === 'config.set')!
    expect(call[0]).toBe('vm')
    const vm = call[2].vm
    expect(vm.enabled).toBe(true)
    // 全字段回写（config.set 是整段写，不丢未编辑字段）
    expect(vm.arch).toBe('x86_64')
    expect(vm.hypervisor).toBe('auto')
    expect(vm.desktop).toBe('minimal')
    expect(vm.image.source).toContain('debian-12-genericcloud-amd64.qcow2')
    expect(vm.image.sha256).toBe('')
    expect(vm.resources).toEqual({ vcpu: 2, ram_mb: 2048, disk_gb: 20 })
    expect(vm.max_instances).toBe(2)
    expect(vm.idle_sleep_secs).toBe(300)
    expect(vm.gc).toEqual({ max_total_disk_gb: 50, over_limit: 'warn' })
    // 写后刷新（overview 重新拉取）
    expect(requestMock.mock.calls.filter((c) => c[1] === 'overview').length).toBe(2)
    expect(useToast().toasts.some((t) => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('开启被取消（confirm false）→ 不发 config.set', async () => {
    const w = await mountView()
    confirmMock.mockReturnValueOnce(false)
    await w.find('[data-test="master-toggle"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.find((c) => c[1] === 'config.set')).toBeUndefined()
    w.unmount()
  })

  it('config 段缺席 → 表单回落 JS 默认（编辑缓冲与后端 serde default 同构）', async () => {
    const w = await mountView(overview({ config: null }))
    await flushPromises()
    const arch = w.find('[data-test="arch"]').element as HTMLSelectElement
    expect(arch.value).toBe('x86_64')
    const vcpu = w.find('[data-test="vcpu"]').element as HTMLInputElement
    expect(vcpu.value).toBe('2')
    // 直接保存：payload 是默认值全集
    await w.find('[data-test="save"]').trigger('click')
    await flushPromises()
    const vm = requestMock.mock.calls.find((c) => c[1] === 'config.set')![2].vm
    expect(vm.enabled).toBe(false)
    expect(vm.resources).toEqual({ vcpu: 2, ram_mb: 2048, disk_gb: 20 })
    w.unmount()
  })

  it('修改资源字段后保存：修改进 payload，脏提示出现', async () => {
    const w = await mountView()
    const vcpu = w.find('[data-test="vcpu"]')
    await vcpu.setValue(4)
    expect(w.find('[data-test="dirty-hint"]').exists()).toBe(true)
    await w.find('[data-test="save"]').trigger('click')
    await flushPromises()
    const vm = requestMock.mock.calls.find((c) => c[1] === 'config.set')![2].vm
    expect(vm.resources.vcpu).toBe(4)
    // 原有 enabled=false 保持（不因编辑被翻转）
    expect(vm.enabled).toBe(false)
    w.unmount()
  })

  it('预设下拉切 aarch64 预设：URL 进编辑缓冲（接口预留档可看可选可配）', async () => {
    const w = await mountView()
    const preset = w.find('[data-test="preset"]')
    await preset.setValue('2') // Debian 12 arm64 预设
    await w.find('[data-test="save"]').trigger('click')
    await flushPromises()
    const vm = requestMock.mock.calls.find((c) => c[1] === 'config.set')![2].vm
    expect(vm.image.source).toContain('debian-12-genericcloud-arm64.qcow2')
    w.unmount()
  })
})

describe('VmView 镜像页', () => {
  async function toImagesTab(w: ReturnType<typeof mount>) {
    await w.findAll('button').find((b) => b.text() === '镜像')!.trigger('click')
    await flushPromises()
  }

  it('镜像列表 + 日志尾部渲染；GC 状态与预算', async () => {
    const w = await mountView()
    await toImagesTab(w)
    expect(w.findAll('[data-test="image-row"]').length).toBe(1)
    expect(w.text()).toContain('base.qcow2')
    expect(w.text()).toContain('333.8M')
    expect(w.find('[data-test="build-log"]').text()).toContain('step1 ok')
    expect(w.find('[data-test="gc-status"]').text()).toContain('磁盘用量 0GB')
    expect(w.find('[data-test="gc-status"]').text()).toContain('预算 50GB')
    w.unmount()
  })

  it('首次构建发 update:false；更新镜像过 confirm 发 update:true', async () => {
    const w = await mountView()
    await toImagesTab(w)
    await w.find('[data-test="build-first"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.find((c) => c[1] === 'image.build')![2]).toEqual({ update: false })

    confirmMock.mockReturnValueOnce(true)
    await w.find('[data-test="build-update"]').trigger('click')
    expect(confirmMock).toHaveBeenCalledTimes(1)
    await flushPromises()
    expect(requestMock.mock.calls.filter((c) => c[1] === 'image.build')[1]![2]).toEqual({ update: true })
    w.unmount()
  })

  it('更新镜像被取消 → 不发 image.build', async () => {
    const w = await mountView()
    await toImagesTab(w)
    confirmMock.mockReturnValueOnce(false)
    await w.find('[data-test="build-update"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.find((c) => c[1] === 'image.build')).toBeUndefined()
    w.unmount()
  })

  it('构建进行中：运行横幅渲染 + 构建按钮禁用', async () => {
    const w = await mountView(overview(), imageList({ build: { running: true, started_ms: 1730000000000, update: false } }))
    await toImagesTab(w)
    expect(w.find('[data-test="build-running"]').exists()).toBe(true)
    expect((w.find('[data-test="build-first"]').element as HTMLButtonElement).disabled).toBe(true)
    expect((w.find('[data-test="build-update"]').element as HTMLButtonElement).disabled).toBe(true)
    w.unmount()
  })

  it('gc.run 转发 + 成功 toast', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'overview') return Promise.resolve(overview())
      if (cmd === 'image.list') return Promise.resolve(imageList())
      if (cmd === 'gc.run') return Promise.resolve({ swept_orphans: 2, used_gb: 12, decision: null })
      return Promise.resolve({})
    })
    const w = mount(VmView)
    await flushPromises()
    await toImagesTab(w)
    await w.find('[data-test="gc-run"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.find((c) => c[1] === 'gc.run')).toBeTruthy()
    expect(useToast().toasts.some((t) => t.message.includes('清理 2 个'))).toBe(true)
    w.unmount()
  })
})

describe('VmView 实例页', () => {
  function mountWithInstances(rows: unknown[]) {
    return mountView(overview({ instances: rows }), imageList())
  }

  async function toInstancesTab(w: ReturnType<typeof mount>) {
    await w.findAll('button').find((b) => b.text() === '实例')!.trigger('click')
    await flushPromises()
  }

  it('实例行渲染：运行态/睡眠态标签与规格', async () => {
    const w = await mountWithInstances([
      instanceRow(),
      instanceRow({ agent_id: 'deadbeef00000000', running: false, asleep_at: 1730000000000, has_snapshot: true }),
    ])
    await toInstancesTab(w)
    const rows = w.findAll('[data-test="instance-row"]')
    expect(rows.length).toBe(2)
    expect(w.text()).toContain('运行中')
    expect(w.text()).toContain('睡眠（snapshot）')
    expect(w.text()).toContain('x86_64 · whpx · 2C/2048M/20G')
    w.unmount()
  })

  it('睡眠/唤醒/冷停命令转发 agent_id；冷停过 confirm 门', async () => {
    const w = await mountWithInstances([
      instanceRow(),
      instanceRow({ agent_id: 'deadbeef00000000', running: false, asleep_at: 1730000000000 }),
    ])
    await toInstancesTab(w)

    await w.find('[data-test="inst-sleep"]').trigger('click')
    await flushPromises()
    const sleepCall = requestMock.mock.calls.find((c) => c[1] === 'instance.sleep')!
    expect(sleepCall[2]).toEqual({ agent_id: 'abc123def4567890' })

    await w.find('[data-test="inst-wake"]').trigger('click')
    await flushPromises()
    const wakeCall = requestMock.mock.calls.find((c) => c[1] === 'instance.wake')!
    expect(wakeCall[2]).toEqual({ agent_id: 'deadbeef00000000' })

    confirmMock.mockReturnValueOnce(false)
    await w.find('[data-test="inst-kill"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.find((c) => c[1] === 'instance.kill')).toBeUndefined()

    confirmMock.mockReturnValueOnce(true)
    await w.find('[data-test="inst-kill"]').trigger('click')
    await flushPromises()
    const killCall = requestMock.mock.calls.find((c) => c[1] === 'instance.kill')!
    expect(killCall[2]).toEqual({ agent_id: 'abc123def4567890' })
    w.unmount()
  })

  it('实例操作失败 → 错误 toast', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'overview') return Promise.resolve(overview({ instances: [instanceRow()] }))
      if (cmd === 'image.list') return Promise.resolve(imageList())
      if (cmd === 'instance.sleep') return Promise.reject(new Error('QEMU 未运行'))
      return Promise.resolve({})
    })
    const w = mount(VmView)
    await flushPromises()
    await toInstancesTab(w)
    await w.find('[data-test="inst-sleep"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('QEMU 未运行'))).toBe(true)
    w.unmount()
  })
})

describe('VmView 环境安装分步卡（B4）', () => {
  it('全绿态：三步就绪渲染；安装按钮「已就绪」禁用', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus())
    expect(w.find('[data-test="env-card"]').exists()).toBe(true)
    expect(w.find('[data-test="env-gate-hint"]').exists()).toBe(false)
    expect(w.find('[data-test="qemu-state"]').text()).toContain('齐备')
    expect(w.find('[data-test="accel-state"]').text()).toContain('硬件加速可用')
    expect(w.find('[data-test="image-state"]').text()).toContain('已就绪')
    const btn = w.find('[data-test="qemu-install"]')
    expect(btn.text()).toBe('已就绪')
    expect((btn.element as HTMLButtonElement).disabled).toBe(true)
    // image_ready=true → 构建按钮不渲染（只留查看日志）
    expect(w.find('[data-test="image-build"]').exists()).toBe(false)
    w.unmount()
  })

  it('未就绪态渲染：absent/needs_kmod 命令展示 + 复制按钮 + 镜像构建按钮在场', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus({
      qemu: { state: 'absent' },
      accel: { state: 'needs_kmod', command: 'sudo modprobe kvm_intel', note: 'KVM 内核模块未加载——终端执行后复检' },
      image_ready: false,
    }))
    expect(w.find('[data-test="qemu-state"]').text()).toContain('未安装')
    expect(w.find('[data-test="accel-state"]').text()).toContain('KVM 内核模块未加载')
    expect(w.find('[data-test="accel-command"]').text()).toContain('sudo modprobe kvm_intel')
    expect(w.find('[data-test="accel-copy"]').exists()).toBe(true)
    expect(w.find('[data-test="accel-enable"]').exists()).toBe(false) // needs_kmod 无一键按钮（半自动诚实纪律）
    expect(w.find('[data-test="image-build"]').exists()).toBe(true)
    w.unmount()
  })

  it('关闭态闸：提示横幅在场，三个动作按钮全部禁用', async () => {
    const w = await mountView(overview({ enabled: false }), imageList(), bootstrapStatus({
      qemu: { state: 'absent' },
      accel: { state: 'needs_enable_windows', action: 'button', command: 'DISM /Online /Enable-Feature /FeatureName:HypervisorPlatform /All /NoRestart', note: 'WHPX 功能未启用' },
      image_ready: false,
    }))
    expect(w.find('[data-test="env-gate-hint"]').exists()).toBe(true)
    expect((w.find('[data-test="qemu-install"]').element as HTMLButtonElement).disabled).toBe(true)
    expect((w.find('[data-test="accel-enable"]').element as HTMLButtonElement).disabled).toBe(true)
    expect((w.find('[data-test="image-build"]').element as HTMLButtonElement).disabled).toBe(true)
    w.unmount()
  })

  // ── B7 构建路由（bootstrap.image_route）各态渲染 ──

  it('wsl_ready：构建按钮可用，状态行注明自动经 WSL', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus({
      image_ready: false,
      image_route: { state: 'wsl_ready', missing: [], command: '', note: 'WSL 环境就绪——「首次构建」自动经 WSL 跑同一管线，产物直落镜像目录', guest_ready: true },
    }))
    const btn = w.find('[data-test="image-build"]')
    expect(btn.exists()).toBe(true)
    expect((btn.element as HTMLButtonElement).disabled).toBe(false)
    expect(w.find('[data-test="image-state"]').text()).toContain('经 WSL')
    expect(w.find('[data-test="image-route-guide"]').exists()).toBe(false)
    w.unmount()
  })

  it('wsl_missing_tools：黄条缺清单 + 复制命令 + 构建按钮禁用', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus({
      image_ready: false,
      image_route: { state: 'wsl_missing_tools', missing: ['qemu-img（qemu-utils 包）', 'file'], command: 'wsl -e bash -c "sudo apt install -y qemu-utils file curl"', note: '装完后点「复检」，转就绪即可构建', guest_ready: true },
    }))
    expect((w.find('[data-test="image-build"]').element as HTMLButtonElement).disabled).toBe(true)
    expect(w.find('[data-test="image-route-missing"]').text()).toContain('qemu-img')
    expect(w.find('[data-test="image-route-command"]').text()).toContain('sudo apt install')
    expect(w.find('[data-test="image-route-copy"]').exists()).toBe(true)
    expect(w.find('[data-test="image-route-note"]').text()).toContain('复检')
    w.unmount()
  })

  it('wsl_no_distro：指路 wsl --install 命令 + 诚实注记，构建按钮禁用', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus({
      image_ready: false,
      image_route: { state: 'wsl_no_distro', missing: [], command: 'wsl --install -d Ubuntu', note: '未检测到 WSL 发行版——终端执行后（可能需要管理员与重启）点「复检」', guest_ready: true },
    }))
    expect((w.find('[data-test="image-build"]').element as HTMLButtonElement).disabled).toBe(true)
    expect(w.find('[data-test="image-route-command"]').text()).toContain('wsl --install -d Ubuntu')
    expect(w.find('[data-test="image-route-note"]').text()).toContain('管理员与重启')
    w.unmount()
  })

  it('guest 未就绪：提示行给 build-guest 复制命令，构建按钮禁用', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus({
      image_ready: false,
      image_route: { state: 'wsl_ready', missing: [], command: '', note: '', guest_ready: false },
    }))
    expect((w.find('[data-test="image-build"]').element as HTMLButtonElement).disabled).toBe(true)
    expect(w.find('[data-test="guest-missing-hint"]').exists()).toBe(true)
    expect(w.find('[data-test="guest-missing-hint"]').text()).toContain('bash scripts/build-guest.sh')
    w.unmount()
  })

  it('no_script：脚本缺席态渲染后端 note 指路，构建按钮禁用', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus({
      image_ready: false,
      image_route: { state: 'no_script', missing: [], command: '', note: '构建脚本不在场——安装 plugin-vm 插件包（plugins/plugin_vm.dll，镜像脚本与 guest 双架构 ELF 已内嵌、首次构建自动释放），或开发形态从仓库根运行网关，或在 Linux 宿主构建后把 base.qcow2 落到镜像目录回传', guest_ready: true },
    }))
    expect((w.find('[data-test="image-build"]').element as HTMLButtonElement).disabled).toBe(true)
    expect(w.find('[data-test="image-state"]').text()).toContain('构建脚本不在场')
    w.unmount()
  })

  it('image_route 键缺席（旧后端形态）：闸默认放行，构建按钮仍可用', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus({
      image_ready: false,
      image_route: undefined,
    }))
    expect(w.find('[data-test="image-route-guide"]').exists()).toBe(false)
    expect((w.find('[data-test="image-build"]').element as HTMLButtonElement).disabled).toBe(false)
    w.unmount()
  })

  it('一键安装转发 bootstrap.install_qemu；成功后立即复检 bootstrap.status', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus({ qemu: { state: 'absent' } }))
    const before = requestMock.mock.calls.filter((c) => c[1] === 'bootstrap.status').length
    await w.find('[data-test="qemu-install"]').trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'bootstrap.install_qemu')!
    expect(call[0]).toBe('vm')
    expect(call[2]).toEqual({})
    expect(requestMock.mock.calls.filter((c) => c[1] === 'bootstrap.status').length).toBeGreaterThan(before)
    expect(useToast().toasts.some((t) => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('一键安装失败（后端拒绝）→ 错误 toast，不轮询', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'overview') return Promise.resolve(overview({ enabled: true }))
      if (cmd === 'image.list') return Promise.resolve(imageList())
      if (cmd === 'bootstrap.status') return Promise.resolve(bootstrapStatus({ qemu: { state: 'absent' } }))
      if (cmd === 'bootstrap.install_qemu') return Promise.reject(new Error('已有 QEMU 安装任务进行中'))
      return Promise.resolve({})
    })
    const w = mount(VmView)
    await flushPromises().then(() => flushPromises()).then(() => flushPromises())
    await w.find('[data-test="qemu-install"]').trigger('click')
    await flushPromises()
    expect(useToast().toasts.some((t) => t.type === 'error' && t.message.includes('已有 QEMU 安装任务进行中'))).toBe(true)
    w.unmount()
  })

  it('加速一键走 confirm 门：取消不发；确认转发 bootstrap.enable_accel', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus({
      accel: { state: 'needs_enable_windows', action: 'button', command: 'DISM /Online /Enable-Feature /FeatureName:HypervisorPlatform /All /NoRestart', note: 'WHPX 功能未启用——页面一键发起（UAC 提权，用户确认）' },
    }))
    confirmMock.mockReturnValueOnce(false)
    await w.find('[data-test="accel-enable"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.find((c) => c[1] === 'bootstrap.enable_accel')).toBeUndefined()

    confirmMock.mockReturnValueOnce(true)
    await w.find('[data-test="accel-enable"]').trigger('click')
    await flushPromises()
    expect(confirmMock).toHaveBeenCalledTimes(2)
    expect(requestMock.mock.calls.find((c) => c[1] === 'bootstrap.enable_accel')![0]).toBe('vm')
    expect(useToast().toasts.some((t) => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('复制命令：clipboard.writeText 收到命令原文 + 成功 toast', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('navigator', { clipboard: { writeText } })
    const cmd = 'sudo modprobe kvm_amd'
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus({
      accel: { state: 'needs_group', command: cmd, note: '/dev/kvm 当前用户不可写' },
    }))
    await w.find('[data-test="accel-copy"]').trigger('click')
    await flushPromises()
    expect(writeText).toHaveBeenCalledWith(cmd)
    expect(useToast().toasts.some((t) => t.message.includes('命令已复制'))).toBe(true)
    w.unmount()
  })

  it('安装进行中：运行指示渲染 + 安装按钮禁用 + 日志尾展示', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus({
      qemu: { state: 'absent' },
      install: { running: true, started_ms: 1730000000000 },
      log: { path: 'C:\\ws\\tools\\vm\\bootstrap.log', tail: ['[1/4] 下载 qemu.zip', '[2/4] 校验 SHA-256'] },
    }))
    expect(w.find('[data-test="qemu-running"]').exists()).toBe(true)
    expect(w.find('[data-test="qemu-running"]').text()).toContain('安装进行中')
    expect((w.find('[data-test="qemu-install"]').element as HTMLButtonElement).disabled).toBe(true)
    expect(w.find('[data-test="bs-log"]').text()).toContain('校验 SHA-256')
    w.unmount()
  })

  it('bootstrap.status 失败 → 卡内诚实降级（重试按钮），页面其余功能不受影响', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'overview') return Promise.resolve(overview())
      if (cmd === 'image.list') return Promise.resolve(imageList())
      if (cmd === 'bootstrap.status') return Promise.reject(new Error('no such command'))
      return Promise.resolve({})
    })
    const w = mount(VmView)
    await flushPromises().then(() => flushPromises()).then(() => flushPromises())
    expect(w.find('[data-test="env-card"]').exists()).toBe(true)
    expect(w.text()).toContain('环境状态获取失败')
    expect(w.find('[data-test="probe-banner"]').exists()).toBe(true)
    w.unmount()
  })
})

describe('VmView 快速试玩卡（B6）', () => {
  async function toInstancesTab(w: ReturnType<typeof mount>) {
    await w.findAll('button').find((b) => b.text() === '实例')!.trigger('click')
    await flushPromises()
  }

  it('关闭态闸：try-off 横幅渲染、启动按钮缺席；就绪态启动转发 try.start + 复检', async () => {
    const w = await mountView(overview({ enabled: false }), imageList(), bootstrapStatus(), tryStatus({ enabled: false }))
    await toInstancesTab(w)
    expect(w.find('[data-test="try-card"]').exists()).toBe(true)
    expect(w.find('[data-test="try-off"]').exists()).toBe(true)
    expect(w.find('[data-test="try-start"]').exists()).toBe(false)
    w.unmount()

    // 就绪态（默认 fixture）：启动按钮在场，点击转发 vm.try.start（无参
    // data）；后端发起即返回 + 首次轮询即见运行 → 成功 toast（冷启动
    // 后台化契约：前端不等冷启，轮询转绿）。
    const w2 = await mountView()
    await toInstancesTab(w2)
    expect(w2.find('[data-test="try-off"]').exists()).toBe(false)
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'overview') return Promise.resolve(overview())
      if (cmd === 'image.list') return Promise.resolve(imageList())
      if (cmd === 'bootstrap.status') return Promise.resolve(bootstrapStatus())
      if (cmd === 'try.start') return Promise.resolve({ running: false, starting: true, note: '冷启动中' })
      if (cmd === 'try.status') return Promise.resolve(tryStatus({ running: true }))
      return Promise.resolve({})
    })
    await w2.find('[data-test="try-start"]').trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'try.start')!
    expect(call[0]).toBe('vm')
    expect(call[2]).toEqual({})
    expect(useToast().toasts.some((t) => t.type === 'success')).toBe(true)
    w2.unmount()
  })

  it('启动在途渲染：状态标签「启动中」、按钮转「启动中…」禁用；失败透传 last_error', async () => {
    // 在途态 fixture（轮询窗口内的真实形态）：标签 + 按钮文案/禁用；
    // 停止按钮在途可见（「启动卡死了想停」的中止路径）。
    const w = await mountView(overview(), imageList(), bootstrapStatus(), tryStatus({ starting: true }))
    await toInstancesTab(w)
    expect(w.find('[data-test="try-state-label"]').text()).toContain('启动中')
    const btn = w.find('[data-test="try-start"]')
    expect(btn.text()).toBe('启动中…')
    expect((btn.element as HTMLButtonElement).disabled).toBe(true)
    expect(w.find('[data-test="try-stop"]').exists()).toBe(true)
    w.unmount()

    // 启动失败：try.status 回落 starting=false + last_error → 错误 toast 带原因。
    const w2 = await mountView()
    await toInstancesTab(w2)
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'overview') return Promise.resolve(overview())
      if (cmd === 'image.list') return Promise.resolve(imageList())
      if (cmd === 'bootstrap.status') return Promise.resolve(bootstrapStatus())
      if (cmd === 'try.start') return Promise.resolve({ running: false, starting: true, note: '冷启动中' })
      if (cmd === 'try.status')
        return Promise.resolve(tryStatus({ starting: false, last_error: '不可用: 加速器缺席' }))
      return Promise.resolve({})
    })
    await w2.find('[data-test="try-start"]').trigger('click')
    await flushPromises()
    const errs = useToast().toasts.filter((t) => t.type === 'error')
    expect(errs.length).toBeGreaterThan(0)
    expect(errs.some((t) => t.message.includes('加速器缺席'))).toBe(true)
    w2.unmount()
  })

  it('镜像未就绪 / arch 不支持：对应指引横幅渲染，启动按钮缺席', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus(), tryStatus({ image_ready: false }))
    await toInstancesTab(w)
    expect(w.find('[data-test="try-noimage"]').exists()).toBe(true)
    expect(w.find('[data-test="try-start"]').exists()).toBe(false)
    w.unmount()

    const w2 = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus(), tryStatus({ arch: 'aarch64', arch_supported: false }))
    await toInstancesTab(w2)
    expect(w2.find('[data-test="try-arch"]').exists()).toBe(true)
    expect(w2.text()).toContain('aarch64')
    expect(w2.find('[data-test="try-start"]').exists()).toBe(false)
    w2.unmount()
  })

  it('运行态：停止按钮 + 输入框 + 预设按钮组渲染；启动按钮缺席', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus(), tryStatus({ running: true }))
    await toInstancesTab(w)
    expect(w.find('[data-test="try-state-label"]').text()).toContain('运行中')
    expect(w.find('[data-test="try-start"]').exists()).toBe(false)
    expect(w.find('[data-test="try-stop"]').exists()).toBe(true)
    expect(w.find('[data-test="try-input"]').exists()).toBe(true)
    expect(w.findAll('[data-test="try-preset"]').length).toBe(5)
    w.unmount()
  })

  it('预设命令转发 try.exec（payload command）；输出区累积渲染', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'overview') return Promise.resolve(overview({ enabled: true }))
      if (cmd === 'image.list') return Promise.resolve(imageList())
      if (cmd === 'bootstrap.status') return Promise.resolve(bootstrapStatus())
      if (cmd === 'try.status') return Promise.resolve(tryStatus({ running: true }))
      if (cmd === 'try.exec') return Promise.resolve({ command: 'uname -a', output: 'Linux try 6.1.0 x86_64' })
      return Promise.resolve({})
    })
    const w = mount(VmView)
    await flushPromises()
    await toInstancesTab(w)
    await w.findAll('[data-test="try-preset"]')[0].trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'try.exec')!
    expect(call[0]).toBe('vm')
    expect(call[2]).toEqual({ command: 'uname -a' })
    const out = w.find('[data-test="try-output"]')
    expect(out.exists()).toBe(true)
    expect(out.text()).toContain('$ uname -a')
    expect(out.text()).toContain('Linux try 6.1.0 x86_64')
    w.unmount()
  })

  it('自由输入 @keydown.enter 触发执行；错误行进输出区且输入保留（可改可重试）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'overview') return Promise.resolve(overview({ enabled: true }))
      if (cmd === 'image.list') return Promise.resolve(imageList())
      if (cmd === 'bootstrap.status') return Promise.resolve(bootstrapStatus())
      if (cmd === 'try.status') return Promise.resolve(tryStatus({ running: true }))
      if (cmd === 'try.exec') return Promise.reject(new Error('VM 内命令执行出错'))
      return Promise.resolve({})
    })
    const w = mount(VmView)
    await flushPromises()
    await toInstancesTab(w)
    await w.find('[data-test="try-input"]').setValue('ls /')
    await w.find('[data-test="try-input"]').trigger('keydown.enter')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'try.exec')!
    expect(call[2]).toEqual({ command: 'ls /' })
    // 失败不清输入——用户改完直接重试（tryCommand 清空只发生在成功路径）
    expect((w.find('[data-test="try-input"]').element as HTMLInputElement).value).toBe('ls /')
    expect(w.find('[data-test="try-output"]').text()).toContain('$ ls /')
    expect(w.find('[data-test="try-output"]').text()).toContain('[出错] VM 内命令执行出错')
    w.unmount()
  })

  it('超时档透传（B8c）：缺省不传 timeout 且预算 120s；选档转发 timeout 且预算=档位+90s', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'overview') return Promise.resolve(overview({ enabled: true }))
      if (cmd === 'image.list') return Promise.resolve(imageList())
      if (cmd === 'bootstrap.status') return Promise.resolve(bootstrapStatus())
      if (cmd === 'try.status') return Promise.resolve(tryStatus({ running: true }))
      if (cmd === 'try.exec') return Promise.resolve({ command: 'x', output: 'ok' })
      return Promise.resolve({})
    })
    const w = mount(VmView)
    await flushPromises()
    await toInstancesTab(w)
    // 缺省档：payload 不带 timeout，请求预算维持 120s 下限。
    await w.find('[data-test="try-input"]').setValue('uname -a')
    await w.find('[data-test="try-run"]').trigger('click')
    await flushPromises()
    let call = requestMock.mock.calls.find((c) => c[1] === 'try.exec')!
    expect(call[2]).toEqual({ command: 'uname -a' })
    expect(call[3]).toBe(120_000)
    // 选 300s：payload 带 timeout=300，预算 390s（档位+90s 余量）。
    await w.find('[data-test="try-timeout"]').setValue('300')
    await w.find('[data-test="try-input"]').setValue('apt update')
    await w.find('[data-test="try-run"]').trigger('click')
    await flushPromises()
    call = requestMock.mock.calls.filter((c) => c[1] === 'try.exec')[1]!
    expect(call[2]).toEqual({ command: 'apt update', timeout: 300 })
    expect(call[3]).toBe(390_000)
    w.unmount()
  })

  it('停止过 confirm 门：取消不发；确认转发 try.stop', async () => {
    const w = await mountView(overview({ enabled: true }), imageList(), bootstrapStatus(), tryStatus({ running: true }))
    await toInstancesTab(w)

    confirmMock.mockReturnValueOnce(false)
    await w.find('[data-test="try-stop"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.find((c) => c[1] === 'try.stop')).toBeUndefined()

    confirmMock.mockReturnValueOnce(true)
    await w.find('[data-test="try-stop"]').trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'try.stop')!
    expect(call[0]).toBe('vm')
    expect(call[2]).toEqual({})
    expect(useToast().toasts.some((t) => t.type === 'success')).toBe(true)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// B9-3 项目工作副本同步卡
// ---------------------------------------------------------------------------

// 与后端 projects.list / vm.sync.status 投影同构（handlers/projects.rs +
// vm_sync.rs SyncStatusReport）。
function projectsList(over: Record<string, unknown>[] = []) {
  return {
    projects: over.length
      ? over
      : [{ id: 'proj-1', name: 'demo-app', path: 'C:\ws\projects\demo-app', created_at: '2026-10-19T00:00:00Z' }],
    count: over.length || 1,
  }
}

function syncStatus(over: Record<string, unknown> = {}) {
  return {
    project_id: 'proj-1',
    has_baseline: true,
    synced_at_ms: 1730000000000,
    selection: 'git',
    baseline_files: 42,
    host_pending: ['src/a.rs'],
    host_pending_deletes: [],
    last_commit: { project_id: 'proj-1', committed_at_ms: 1730001000000, changed: ['x.txt'], deleted: [], bytes: 5 },
    ...over,
  }
}

function mountWithSync(pl: unknown = projectsList(), ss: unknown = syncStatus(), extra: Record<string, unknown> = {}) {
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'overview') return Promise.resolve(overview({ enabled: true }))
    if (cmd === 'image.list') return Promise.resolve(imageList())
    if (cmd === 'bootstrap.status') return Promise.resolve(bootstrapStatus())
    if (cmd === 'try.status') return Promise.resolve(tryStatus())
    if (cmd === 'list') return Promise.resolve(pl)
    if (cmd === 'sync.status') return Promise.resolve(ss)
    if (cmd in extra) return Promise.resolve(extra[cmd])
    return Promise.resolve({})
  })
  const w = mount(VmView)
  return flushPromises().then(() => flushPromises()).then(() => flushPromises()).then(() => w)
}

describe('VmView 项目工作副本同步卡（B9-3）', () => {
  async function toInstancesTab(w: ReturnType<typeof mount>) {
    await w.findAll('button').find((b) => b.text() === '实例')!.trigger('click')
    await flushPromises()
  }

  it('无注册项目：占位文案渲染；不发 sync.status', async () => {
    const w = await mountView()
    await toInstancesTab(w)
    expect(w.find('[data-test="sync-card"]').exists()).toBe(true)
    expect(w.find('[data-test="sync-no-projects"]').exists()).toBe(true)
    expect(requestMock.mock.calls.find((c) => c[1] === 'sync.status')).toBeUndefined()
    w.unmount()
  })

  it('有项目：projects.list + sync.status 拉取；基线/待同步/上次提交渲染', async () => {
    const w = await mountWithSync()
    await toInstancesTab(w)
    expect(w.find('[data-test="sync-project"]').exists()).toBe(true)
    expect((w.find('[data-test="sync-project"]').element as HTMLSelectElement).value).toBe('proj-1')
    expect(w.find('[data-test="sync-baseline"]').text()).toContain('42 个文件')
    expect(w.find('[data-test="sync-pending"]').text()).toContain('1 改 / 0 删')
    expect(w.find('[data-test="sync-last-commit"]').text()).toContain('上次提交')
    w.unmount()
  })

  it('无基线态：sync-no-baseline 渲染（has_baseline=false = 未开工，不是无变更）', async () => {
    const w = await mountWithSync(projectsList(), syncStatus({ has_baseline: false, synced_at_ms: null, selection: null, baseline_files: null, host_pending: [], host_pending_deletes: [], last_commit: null }))
    await toInstancesTab(w)
    expect(w.find('[data-test="sync-no-baseline"]').exists()).toBe(true)
    expect(w.find('[data-test="sync-baseline"]').exists()).toBe(false)
    w.unmount()
  })

  it('一键提交：project id 转发 + 10 分钟预算；no_baseline 走 info 分支', async () => {
    const w = await mountWithSync(projectsList(), syncStatus(), {
      commit: { project_id: 'proj-1', committed_at_ms: 1730002000000, changed: ['a'], deleted: [], bytes: 1 },
    })
    await toInstancesTab(w)
    await w.find('[data-test="sync-commit"]').trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'commit')!
    expect(call[0]).toBe('vm')
    expect(call[2]).toEqual({ project: 'proj-1' })
    expect(call[3]).toBe(600_000)
    w.unmount()
  })

  it('强制重同步过 confirm 门：取消不发；确认转发 resync（project + 预算）', async () => {
    const w = await mountWithSync(projectsList(), syncStatus(), {
      resync: { files: 42, bytes: 12345, selection: 'git', synced_at_ms: 1730003000000 },
    })
    await toInstancesTab(w)

    confirmMock.mockReturnValueOnce(false)
    await w.find('[data-test="sync-resync"]').trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.find((c) => c[1] === 'resync')).toBeUndefined()

    confirmMock.mockReturnValueOnce(true)
    await w.find('[data-test="sync-resync"]').trigger('click')
    await flushPromises()
    const call = requestMock.mock.calls.find((c) => c[1] === 'resync')!
    expect(call[0]).toBe('vm')
    expect(call[2]).toEqual({ project: 'proj-1' })
    expect(call[3]).toBe(600_000)
    expect(useToast().toasts.some((t) => t.type === 'success')).toBe(true)
    w.unmount()
  })

  it('sync.status 请求失败：sync-error 诚实渲染（不崩、按钮仍在）', async () => {
    const w = await mountWithSync(projectsList(), { _error: '项目不存在: proj-1' })
    await toInstancesTab(w)
    expect(w.find('[data-test="sync-error"]').text()).toContain('项目不存在')
    expect(w.find('[data-test="sync-commit"]').exists()).toBe(true)
    w.unmount()
  })
})
