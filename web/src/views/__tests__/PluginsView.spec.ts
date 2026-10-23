import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useToast } from '../../composables/useToast'

// 三 Tab 重构（2026-09-29）+ 本地插件库升级（2026-10-21：目录全量四成员 +
// 用户开关（native_set_enabled，长期记忆）+ 签名状态徽标 + 未知库清单——
// 只列出，不装载）。契约（request('plugins','list') 精确形态）与错误降级
// 不变。后端行为由 handlers/plugins/tests.rs（含 dispatch 级）钉住。
// 2026-10-22 WASM 加载纪律反转（安装 ≠ 加载）：行恒带开关（加载意愿，
// config.plugin.wasm.json 记录位），未加载行也有配置钮、无日志钮；
// 引导文案提 plugins_wasm/ 目录与默认不加载。

const requestMock = vi.fn()
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import PluginsView from '../PluginsView.vue'

const LIST = {
  pipeline_plugins: [
    { name: 'metrics-pipeline', scope: null, enabled: true, description: '每工具调用计时（around 段参考实现）' },
  ],
  plugins: [
    {
      id: 'plugin_onnx',
      label: 'ONNX 嵌入推理',
      used_by: '强化记忆 / 自动记忆注入',
      found: true,
      filename: 'plugin_onnx.dll',
      path: 'C:\\bot\\plugins\\plugin_onnx.dll',
      enabled: true,
      immediate_effect: false,
      signature: { mode: 'dormant', state: null, detail: '签名状态探测未接入（gateway 未注入 probe）' },
      capabilities: ['embedding 推理'],
      detail: {
        enhanced_memory_enabled: true,
        active_tier: 'medium',
        active_model: 'all-MiniLM-L6-v2',
        model_ready: true,
      },
    },
    {
      id: 'plugin_ui',
      label: 'WebView UI / 系统托盘',
      used_by: 'desktop 集成',
      found: false,
      filename: 'plugin_ui.dll',
      enabled: true,
      immediate_effect: false,
      signature: { mode: 'dormant', state: null, detail: '签名状态探测未接入（gateway 未注入 probe）' },
    },
    {
      id: 'eval_monitor_dll',
      label: 'eval 沙盒监控',
      used_by: 'eval 行为评估',
      found: true,
      filename: 'eval_monitor_dll.dll',
      enabled: true,
      immediate_effect: true,
      signature: { mode: 'dormant', state: null, detail: '签名状态探测未接入（gateway 未注入 probe）' },
    },
    {
      id: 'plugin_wasm',
      label: 'WASM 插件宿主壳',
      used_by: 'WASM 组件插件子系统',
      found: true,
      filename: 'plugin_wasm.dll',
      enabled: false,
      immediate_effect: false,
      signature: { mode: 'dormant', state: null, detail: '签名状态探测未接入（gateway 未注入 probe）' },
    },
    {
      id: 'plugin_vm',
      label: 'VM 显示通道',
      used_by: 'sandbox VM 画面中继',
      found: true,
      filename: 'plugin_vm.dll',
      enabled: true,
      immediate_effect: false,
      signature: { mode: 'dormant', state: null, detail: '签名状态探测未接入（gateway 未注入 probe）' },
    },
  ],
  unrecognized_libraries: [
    { name: 'stray_helper.dll', path: 'C:\\bot\\plugins\\stray_helper.dll' },
  ],
}

beforeEach(() => {
  requestMock.mockReset()
  useToast().toasts.splice(0)
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'list') return Promise.resolve(LIST)
    if (cmd === 'wasm.list') return Promise.resolve({ plugins: [] })
    return Promise.resolve({})
  })
})

async function mountView() {
  const w = mount(PluginsView)
  await flushPromises()
  return w
}

async function switchTab(w: ReturnType<typeof mount>, label: string) {
  const tab = w.findAll('.tab').find(b => b.text() === label)
  expect(tab, `找不到 Tab「${label}」`).toBeTruthy()
  await tab!.trigger('click')
  await flushPromises()
}

describe('PluginsView 四 Tab 重构（WASM 主角 + 本地库 + 管线 + 说明）', () => {
  it('契约：挂载即 request("plugins","list")；默认 Tab = WASM 纯列表（说明卡片移入【说明】Tab）', async () => {
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('plugins', 'list')
    expect(requestMock).toHaveBeenCalledWith('plugins', 'wasm.list')
    // 默认 Tab 是 WASM：只剩插件列表（说明卡片 2026-10-23 移入【说明】Tab）
    expect(w.text()).not.toContain('WASM 插件是什么')
    expect(w.text()).not.toContain('怎么开发一个插件')
    expect(w.text()).not.toContain('下载插件开发包')
    expect(w.text()).toContain('尚未安装任何 WASM 插件')
    // 本地库内容在另一 Tab，默认不渲染
    expect(w.text()).not.toContain('4/5 已就绪')
    // 【说明】Tab：两张说明卡片 + devkit 下载钮在场
    await switchTab(w as any, '说明')
    expect(w.text()).toContain('WASM 插件是什么')
    expect(w.text()).toContain('怎么开发一个插件')
    expect(w.text()).toContain('下载插件开发包')
    w.unmount()
  })

  it('说明 Tab devkit 下载：点击走 wasm.devkit_download（overwrite=false），结果面板展示解压根', async () => {
    const w = await mountView()
    await switchTab(w as any, '说明')
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(LIST)
      if (cmd === 'wasm.list') return Promise.resolve({ plugins: [] })
      if (cmd === 'wasm.devkit_download')
        return Promise.resolve({ path: 'C:\\ws\\wasm-plugin-devkit\\nightly-wasm-devkit.zip', dir: 'C:\\ws\\wasm-plugin-devkit\\devkit', size: 2048, files: 42 })
      return Promise.resolve({})
    })
    const btn = w.findAll('button').find(b => b.text().includes('下载插件开发包'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('plugins', 'wasm.devkit_download', { overwrite: false })
    expect(w.text()).toContain('C:\\ws\\wasm-plugin-devkit\\devkit')
    expect(w.text()).toContain('2.0 KB')
    // 二次下载带覆盖复选
    const box = w.find('input[type="checkbox"]')
    await box.setValue(true)
    await btn!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenLastCalledWith('plugins', 'wasm.devkit_download', { overwrite: true })
    w.unmount()
  })

  it('本地插件库 Tab：目录全量四成员 + 就绪状态 + 开关 + 签名徽标 + 生效语义', async () => {
    const w = await mountView()
    await switchTab(w as any, '本地插件库')
    expect(w.text()).toContain('4/5 已就绪')
    expect(w.text()).toContain('plugin_onnx')
    expect(w.text()).toContain('已就绪')
    expect(w.text()).toContain('C:\\bot\\plugins\\plugin_onnx.dll')
    expect(w.text()).toContain('embedding 推理')
    expect(w.text()).toContain('模型就绪')
    // 目录全量（2026-10-21：不再只列 onnx+ui；2026-10-23：+plugin_vm 花名册补登记）
    expect(w.text()).toContain('eval_monitor_dll')
    expect(w.text()).toContain('plugin_wasm')
    expect(w.text()).toContain('plugin_vm')
    expect(w.text()).toContain('未找到')
    // 开关（checkbox 在场）+ 生效语义注记
    const boxes = w.findAll('input[type="checkbox"]')
    expect(boxes.length).toBe(5)
    expect(w.text()).toContain('开关即时生效')
    expect(w.text()).toContain('改动重启后生效')
    // 签名徽标（dormant 态）
    expect(w.text()).toContain('签名策略休眠')
    // 停用条目：置灰（opacity）+ 已停用 badge
    const off = w.findAll('.plugin-card').find(c => c.text().includes('plugin_wasm'))
    expect(off!.attributes('style')).toContain('opacity: 0.55')
    expect(off!.text()).toContain('已停用')
    w.unmount()
  })

  it('本地插件库开关：toggle 走 native_set_enabled，toast 带生效语义', async () => {
    const w = await mountView()
    await switchTab(w as any, '本地插件库')
    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'native_set_enabled')
        return Promise.resolve({ id: data.id, enabled: data.enabled, immediate_effect: false, note: '重启 gateway 后生效' })
      if (cmd === 'list') return Promise.resolve(LIST)
      return Promise.resolve({})
    })
    // 点 plugin_onnx 的开关（关）。
    const card = w.findAll('.plugin-card').find(c => c.text().includes('plugin_onnx'))
    const box = card!.find('input[type="checkbox"]')
    await box.setValue(false)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('plugins', 'native_set_enabled', { id: 'plugin_onnx', enabled: false })
    expect(useToast().toasts.some(t => t.message.includes('已停用') && t.message.includes('重启 gateway 后生效'))).toBe(true)
    w.unmount()
  })

  it('未知库清单：只列出，不装载', async () => {
    const w = await mountView()
    await switchTab(w as any, '本地插件库')
    expect(w.text()).toContain('未识别的库')
    expect(w.text()).toContain('只列出，不装载')
    expect(w.text()).toContain('stray_helper.dll')
    w.unmount()
  })

  it('签名 enforced 拦截态：红字签名拦截徽标', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') {
        return Promise.resolve({
          ...LIST,
          plugins: LIST.plugins.map(p =>
            p.id === 'plugin_ui'
              ? { ...p, signature: { mode: 'enforced', state: 'Tampered', detail: 'file hash mismatch' } }
              : p
          ),
        })
      }
      if (cmd === 'wasm.list') return Promise.resolve({ plugins: [] })
      return Promise.resolve({})
    })
    const w = await mountView()
    await switchTab(w as any, '本地插件库')
    expect(w.text()).toContain('签名拦截：Tampered')
    expect(w.text()).toContain('file hash mismatch')
    w.unmount()
  })

  it('签名 warn 态：未验签放行（橙）+ 已验签（绿）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') {
        return Promise.resolve({
          ...LIST,
          plugins: LIST.plugins.map(p =>
            p.id === 'plugin_ui'
              ? { ...p, signature: { mode: 'warn', state: 'NoSignature', detail: '无主签名（开发构建 / 未签产物）' } }
              : p.id === 'plugin_onnx'
                ? { ...p, signature: { mode: 'warn', state: 'Valid', detail: 'signed_at=1' } }
                : p
          ),
        })
      }
      if (cmd === 'wasm.list') return Promise.resolve({ plugins: [] })
      return Promise.resolve({})
    })
    const w = await mountView()
    await switchTab(w as any, '本地插件库')
    expect(w.text()).toContain('未验签（warn 放行）')
    expect(w.text()).toContain('无主签名（开发构建 / 未签产物）')
    w.unmount()
  })

  it('onnx detail：模型未安装 → 红字提示', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') {
        return Promise.resolve({
          ...LIST,
          plugins: LIST.plugins.map(p =>
            p.id === 'plugin_onnx'
              ? { ...p, detail: { enhanced_memory_enabled: true, active_tier: 'medium', active_model: 'all-MiniLM-L6-v2', model_ready: false } }
              : p
          ),
        })
      }
      if (cmd === 'wasm.list') return Promise.resolve({ plugins: [] })
      return Promise.resolve({})
    })
    const w = await mountView()
    await switchTab(w as any, '本地插件库')
    expect(w.text()).toContain('模型未安装')
    w.unmount()
  })

  it('list 失败 → 错误 toast、不崩溃', async () => {
    requestMock.mockRejectedValue(new Error('ws down'))
    const w = await mountView()
    expect(useToast().toasts.some(t => t.type === 'error' && t.message.includes('加载插件状态失败'))).toBe(true)
    w.unmount()
  })

  it('管线插件 Tab：切换后渲染启停开关；切换走 set_metrics_enabled', async () => {
    const w = await mountView()
    await switchTab(w as any, '管线插件')
    expect(w.text()).toContain('管线插件')
    expect(w.text()).toContain('metrics-pipeline')

    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(LIST)
      if (cmd === 'set_metrics_enabled') return Promise.resolve({ name: 'metrics-pipeline', enabled: false })
      return Promise.resolve({})
    })
    const card = w.findAll('.card').find(c => c.text().includes('管线插件'))
    expect(card).toBeTruthy()
    const box = card!.find('input[type="checkbox"]')
    expect(box.exists()).toBe(true)
    await box.setValue(false)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('plugins', 'set_metrics_enabled', { enabled: false })
    expect(useToast().toasts.some(t => t.message.includes('已停用'))).toBe(true)
    w.unmount()
  })
})

describe('WASM 加载纪律反转（2026-10-22：安装 ≠ 加载，开关记录决定装载）', () => {
  // trust 四态字符串（trust.rs：trusted / review-recommended / review-required；
  // handler 对无 lockfile 记录的手工拷入投影 "unknown"）。
  const WASM_ROWS = [
    {
      slug: 'translate', name: '翻译工具', version: '0.1.0', kind: 'tool', trust: 'trusted',
      enabled: true, loaded: true, wasm_sha256: 'a'.repeat(64), min_tier: 'normal',
      egress: ['api.example.com'], x_secret: ['api_key'],
      takes_over: ['web_fetch'],
      tool: { name: 'plugin.translate.translate', description: '文本翻译', operation_type: 'network', min_tier: 'normal' },
    },
    {
      slug: 'activity', name: '活动记录', version: '0.2.0', kind: 'observer', trust: 'unknown',
      enabled: false, loaded: false,
    },
    {
      slug: 'gone', name: 'gone', version: '0.3.0', kind: null, trust: 'review-required',
      enabled: true, loaded: false,
      error: '载荷目录缺失（lockfile 记录仍在；重装或卸载）',
    },
  ]

  function mockWasmRows() {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(LIST)
      if (cmd === 'wasm.list') return Promise.resolve({ plugins: WASM_ROWS })
      return Promise.resolve({})
    })
  }

  it('行语义：●已加载 / ○未加载徽标；开关恒显示；未加载行无日志钮；缺载荷注记在场', async () => {
    mockWasmRows()
    const w = await mountView()
    // 引导文案：目录可见性 + 默认不加载纪律
    expect(w.text()).toContain('plugins_wasm/')
    expect(w.text()).toContain('默认不加载')
    // 已加载行：● + 已加载徽标 + 日志钮在场
    const loadedCard = w.findAll('.plugin-card').find(c => c.text().includes('translate'))
    expect(loadedCard!.text()).toContain('●')
    expect(loadedCard!.text()).toContain('已加载')
    expect(loadedCard!.findAll('button').some(b => b.text() === '日志')).toBe(true)
    // 接管徽标（2026-10-23 工具接管层）：takes_over 声明 → 「接管： <工具>」逐枚展示
    expect(loadedCard!.text()).toContain('接管： web_fetch')
    // 默认未加载行：○ 未加载 + 开关在场（加载意愿恒可操作）+ 无日志钮 + 置灰
    const offCard = w.findAll('.plugin-card').find(c => c.text().includes('activity'))
    expect(offCard!.text()).toContain('未加载')
    // 无接管声明的行不渲染接管徽标
    expect(offCard!.text()).not.toContain('接管：')
    expect(offCard!.find('input[type="checkbox"]').exists()).toBe(true)
    expect(offCard!.findAll('button').some(b => b.text() === '日志')).toBe(false)
    expect(offCard!.attributes('style')).toContain('opacity: 0.6')
    // lockfile 有记录载荷缺席行：error 注记 + 开关照常在场
    const goneCard = w.findAll('.plugin-card').find(c => c.text().includes('gone'))
    expect(goneCard!.text()).toContain('载荷目录缺失')
    expect(goneCard!.find('input[type="checkbox"]').exists()).toBe(true)
    w.unmount()
  })

  it('开关 = 加载意愿：enable 走 wasm.enable + 按后端真相重拉列表', async () => {
    mockWasmRows()
    const w = await mountView()
    requestMock.mockClear()
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'wasm.enable') return Promise.resolve({ slug: data.slug, enabled: true, loaded: true })
      if (cmd === 'list') return Promise.resolve(LIST)
      if (cmd === 'wasm.list')
        return Promise.resolve({
          plugins: WASM_ROWS.map(r => (r.slug === 'activity' ? { ...r, enabled: true, loaded: true } : r)),
        })
      return Promise.resolve({})
    })
    const card = w.findAll('.plugin-card').find(c => c.text().includes('activity'))
    await card!.find('input[type="checkbox"]').setValue(true)
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('plugins', 'wasm.enable', { slug: 'activity' })
    // disable=真摘除 / enable=装配 都会翻 loaded——前端不做乐观翻位，重拉
    expect(requestMock).toHaveBeenCalledWith('plugins', 'wasm.list')
    expect(useToast().toasts.some(t => t.message.includes('已加载'))).toBe(true)
    // 重拉后该行变已加载（●）
    const reloaded = w.findAll('.plugin-card').find(c => c.text().includes('activity'))
    expect(reloaded!.text()).toContain('已加载')
    w.unmount()
  })

  it('安装完成 toast：新装（loaded=false）明示「默认不加载」', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve(LIST)
      if (cmd === 'wasm.list') return Promise.resolve({ plugins: [] })
      if (cmd === 'wasm.install')
        return Promise.resolve({
          installed: true,
          loaded: false,
          plugin: { slug: 'translate', version: '0.1.0', kind: null, trust: 'review-required', enabled: false, loaded: false },
        })
      return Promise.resolve({})
    })
    const w = await mountView()
    await w.find('input[placeholder*="插件源目录"]').setValue('C:\\pkg\\translate')
    const btn = w.findAll('button').find(b => b.text() === '安装')
    await btn!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('plugins', 'wasm.install', {
      source_dir: 'C:\\pkg\\translate',
      allow_unsigned: false,
    })
    expect(useToast().toasts.some(t => t.message.includes('默认不加载'))).toBe(true)
    w.unmount()
  })
})
