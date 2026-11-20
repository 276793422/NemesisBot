import { mount, flushPromises } from '@vue/test-utils'
import { describe, it, expect, vi, beforeEach } from 'vitest'

// 邮箱页空态卡 spec（插件缺席引导层，2026-11-19）：loaded:false 时中央空态卡
// （标题/原因 hint/四条检查清单/重试按钮）顶替整个操作面——header 动作组、
// 分库统计条、地址 chips、列表主区全部不渲染；重试点击触发 mail.status 重拉。
// 在场形态（loaded:true）主区恢复渲染。三层语义的中间层：编译态（feature 裁掉）
// 整页不存在（router 门控），运行时缺席 = 此卡，在场 = 完整邮箱。

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

async function mountView() {
  const w = mount(MailboxView)
  await flushPromises()
  return w
}

describe('MailboxView 插件缺席空态卡', () => {
  it('loaded:false：空态卡渲染（标题/hint/检查清单/重试），操作面全部不渲染；只发 status 一个请求（缺席短路）', async () => {
    const calls: string[] = []
    requestMock.mockImplementation((_m: string, cmd: string) => {
      calls.push(cmd)
      if (cmd === 'status') {
        return Promise.resolve({ loaded: false, hint: 'mail 插件未装载（DLL 缺席或 config.mail.json enabled 未开启）' })
      }
      // stats/list/addr_list 走 catch → notLoaded 兜底
      return Promise.reject(new Error('mail 插件未装载'))
    })
    const w = await mountView()
    const empty = w.find('.mail-empty')
    expect(empty.exists()).toBe(true)
    expect(empty.text()).toContain('邮箱插件未装载')
    expect(empty.text()).toContain('mail 插件未装载（DLL 缺席或 config.mail.json enabled 未开启）')
    // 四条检查清单
    expect(empty.findAll('.empty-checklist li').length).toBe(4)
    expect(empty.text()).toContain('plugins/plugin_mail.dll')
    expect(empty.text()).toContain('config.mail.json')
    expect(empty.text()).toContain('config.plugin.native.json')
    expect(empty.text()).toContain('重启网关')
    // 重试按钮在场
    expect(empty.find('button').text()).toContain('重试')
    // 操作面全部不渲染
    expect(w.find('.header-actions').exists()).toBe(false)
    expect(w.find('.stat-strip').exists()).toBe(false)
    expect(w.find('.addr-chips').exists()).toBe(false)
    expect(w.find('.mail-main').exists()).toBe(false)
    expect(w.find('.srv-strip').exists()).toBe(false)
    // 缺席短路：loadAll 只发 status + 空态卡开关态预取（config_get），
    // 不发注定失败的 stats/addr_list/list
    expect(calls).toEqual(['status', 'config_get'])
    w.unmount()
  })

  it('缺省 hint：空态卡显示兜底文案（不带 status.hint）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve({ loaded: false })
      return Promise.reject(new Error('x'))
    })
    const w = await mountView()
    expect(w.find('.mail-empty').text()).toContain('plugin_mail DLL 缺席、config.mail.json 总闸未开或插件名册未启用')
    w.unmount()
  })

  it('重试按钮：点击后重拉 mail.status——装载恢复后操作面回归', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve({ loaded: false })
      return Promise.reject(new Error('x'))
    })
    const w = await mountView()
    expect(w.find('.mail-empty').exists()).toBe(true)
    // 装载恢复：status → loaded:true，stats/addr_list/list 正常返回
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'status') return Promise.resolve({ loaded: true, smtp: { running: true, bind: '0.0.0.0', port: 2525, tls: true }, pop3: { running: true, bind: '0.0.0.0', port: 995, tls: true } })
      if (cmd === 'stats') return Promise.resolve({ mailboxes: [{ mailbox: 'a@x', total: 1, unread: 0, bytes: 10 }] })
      if (cmd === 'addr_list') return Promise.resolve({ addresses: ['a@x'], default: 'a@x' })
      if (cmd === 'list') return Promise.resolve({ items: [] })
      return Promise.resolve({})
    })
    await w.find('.mail-empty button').trigger('click')
    await flushPromises()
    expect(w.find('.mail-empty').exists()).toBe(false)
    expect(w.find('.header-actions').exists()).toBe(true)
    expect(w.find('.stat-strip').exists()).toBe(true)
    expect(w.find('.mail-main').exists()).toBe(true)
    // 服务端状态卡 + TLS 徽标（在场形态）
    expect(w.find('.srv-strip').text()).toContain('🔒STARTTLS')
    expect(w.find('.srv-strip').text()).toContain('🔒POP3S')
    w.unmount()
  })

  it('分库卡点击：默认地址归一化为 \'\'（匹配下拉默认库 option）并触发 list 刷新', async () => {
    const listArgs: any[] = []
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'status') return Promise.resolve({ loaded: true, smtp: { running: true, bind: '127.0.0.1', port: 2525 }, pop3: { running: false } })
      if (cmd === 'stats') return Promise.resolve({ mailboxes: [
        { mailbox: 'a@x', total: 1, unread: 0, bytes: 10 },
        { mailbox: 'b@x', total: 2, unread: 1, bytes: 20 },
      ] })
      if (cmd === 'addr_list') return Promise.resolve({ addresses: ['a@x', 'b@x'], default: 'a@x' })
      if (cmd === 'list') { listArgs.push(data?.mailbox); return Promise.resolve({ items: [] }) }
      return Promise.resolve({})
    })
    const w = await mountView()
    // 初次加载：currentMailbox=''，list mailbox=''
    expect(listArgs).toEqual([''])
    const cards = w.findAll('.stat-card')
    // 点默认地址 a@x 的分库卡 → 归一化 ''，list 重拉
    await cards[0].trigger('click')
    expect(listArgs).toEqual(['', ''])
    // 点非默认地址 b@x 的分库卡 → 传地址本身，list 重拉
    await cards[1].trigger('click')
    expect(listArgs).toEqual(['', '', 'b@x'])
    // 下拉 value 跟随（b@x 在 options 中有对应项）
    expect((w.find('.sel').element as HTMLSelectElement).value).toBe('b@x')
    w.unmount()
  })

  it('空态卡配置入口：打开配置模态——插件缺席也可配（config_get 先于句柄闸），保存回写保留未知键', async () => {
    let setArgs: any = null
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'status') return Promise.resolve({ loaded: false })
      if (cmd === 'config_get') return Promise.resolve({
        config: {
          enabled: false,
          // 表单外键：保存后必须原样保留（notify 段不被表单落盘抹掉）
          notify: { on: ['estop'], to: 'ops@x' },
          client: { smtp_host: 'smtp.qq.com', smtp_port: 465 },
        },
        path: 'C:/ws/config/config.mail.json',
      })
      if (cmd === 'config_set') { setArgs = data; return Promise.resolve({ ok: true, note: '已保存；重启网关后生效' }) }
      return Promise.reject(new Error('unexpected: ' + cmd))
    })
    const w = await mountView()
    // 空态卡内配置入口（双入口之一）
    const cfgBtn = w.findAll('.mail-empty button').find(b => b.text().includes('配置'))
    expect(cfgBtn).toBeTruthy()
    await cfgBtn!.trigger('click')
    await flushPromises()
    expect(w.find('.cfg-modal').exists()).toBe(true)
    expect(w.find('.cfg-modal').text()).toContain('config.mail.json')
    // 表单回显：client.smtp_host 从 config_get 映射进扁平编辑态
    const hostInput = w.findAll('.cfg-modal input')
      .find(i => (i.element as HTMLInputElement).value === 'smtp.qq.com')
    expect(hostInput).toBeTruthy()
    // 改总闸并保存
    await w.findAll('.cfg-modal input[type=checkbox]')[0].setValue(true)
    await w.find('.cfg-modal .modal-actions .btn.primary').trigger('click')
    await flushPromises()
    // config_set 载荷：改过的键生效 + 表单外键保留 + client 段原样
    expect(setArgs?.config?.enabled).toBe(true)
    expect(setArgs?.config?.notify).toEqual({ on: ['estop'], to: 'ops@x' })
    expect(setArgs?.config?.client?.smtp_host).toBe('smtp.qq.com')
    // 保存成功模态关闭
    expect(w.find('.cfg-modal').exists()).toBe(false)
    w.unmount()
  })
})

describe('MailboxView 空态卡总闸直置开关（2026-11-20 热装载通路）', () => {
  function baseMock(setResult: any) {
    return (_m: string, cmd: string, data?: any) => {
      if (cmd === 'status') return Promise.resolve({ loaded: false, hint: 'mail 插件未装载：总闸未开' })
      if (cmd === 'config_get') return Promise.resolve({ config: { enabled: false, notify: { on: ['estop'] } }, path: 'C:/ws/config/config.mail.json' })
      if (cmd === 'config_set') return Promise.resolve(setResult)
      return Promise.reject(new Error('unexpected: ' + cmd))
    }
  }

  it('开关态预取自盘上 config_get（enabled 真值）', async () => {
    requestMock.mockImplementation(baseMock({ ok: true, loaded: false }))
    const w = await mountView()
    const box = w.find('.empty-switch-row input')
    expect(box.exists()).toBe(true)
    expect((box.element as HTMLInputElement).checked).toBe(false)
    w.unmount()
  })

  it('勾选 → config_set 以盘上 config 为底只覆写 enabled:true → loaded:true → 整页即时活', async () => {
    let setArgs: any = null
    let hotLoaded = false // config_set 之后 status 才恢复 loaded:true
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'status') {
        return hotLoaded
          ? Promise.resolve({ loaded: true, smtp: { running: true, bind: '127.0.0.1', port: 2525 }, pop3: { running: false } })
          : Promise.resolve({ loaded: false, hint: 'mail 插件未装载：总闸未开' })
      }
      if (cmd === 'config_get') return Promise.resolve({ config: { enabled: false, notify: { on: ['estop'], to: 'ops@x' } } })
      if (cmd === 'config_set') { setArgs = data; hotLoaded = true; return Promise.resolve({ ok: true, loaded: true, note: '已保存并即时装载（热生效，无需重启网关）' }) }
      if (cmd === 'stats') return Promise.resolve({ mailboxes: [] })
      if (cmd === 'addr_list') return Promise.resolve({ addresses: [], default: '' })
      if (cmd === 'list') return Promise.resolve({ items: [] })
      return Promise.reject(new Error('unexpected: ' + cmd))
    })
    const w = await mountView()
    expect(w.find('.mail-empty').exists()).toBe(true)
    await w.find('.empty-switch-row input').setValue(true)
    await flushPromises()
    // 载荷：enabled 翻 true + 表单外键（notify 段）原样保留
    expect(setArgs?.config?.enabled).toBe(true)
    expect(setArgs?.config?.notify).toEqual({ on: ['estop'], to: 'ops@x' })
    // 整页活：空态卡消失、操作面回归（勾选即用，不等重启）
    expect(w.find('.mail-empty').exists()).toBe(false)
    expect(w.find('.header-actions').exists()).toBe(true)
    expect(w.find('.mail-main').exists()).toBe(true)
    w.unmount()
  })

  it('勾选但装载失败：load_error 透传、空态卡保留、开关保持勾选（盘已保存，原因不在总闸）', async () => {
    requestMock.mockImplementation(baseMock({ ok: true, loaded: false, load_error: 'plugin_mail DLL 缺席（exe 旁 plugins/ 目录无 plugin_mail 库）', note: '已保存；即时装载失败：plugin_mail DLL 缺席（修正后重新保存即可重试）' }))
    const w = await mountView()
    await w.find('.empty-switch-row input').setValue(true)
    await flushPromises()
    // 装载失败 = 空态卡保留（盘已保存，原因不在总闸）
    expect(w.find('.mail-empty').exists()).toBe(true)
    expect((w.find('.empty-switch-row input').element as HTMLInputElement).checked).toBe(true)
    w.unmount()
  })

  it('取消勾选 → config_set enabled:false 落盘（未装载态无运行面可停，纯盘面写）', async () => {
    let setArgs: any = null
    requestMock.mockImplementation((_m: string, cmd: string, data?: any) => {
      if (cmd === 'status') return Promise.resolve({ loaded: false })
      if (cmd === 'config_get') return Promise.resolve({ config: { enabled: true } })
      if (cmd === 'config_set') { setArgs = data; return Promise.resolve({ ok: true, loaded: false, note: '已保存；重启网关后生效' }) }
      return Promise.reject(new Error('unexpected: ' + cmd))
    })
    const w = await mountView()
    // 盘上 enabled:true 预取为勾选态
    expect((w.find('.empty-switch-row input').element as HTMLInputElement).checked).toBe(true)
    await w.find('.empty-switch-row input').setValue(false)
    await flushPromises()
    expect(setArgs?.config?.enabled).toBe(false)
    // 未装载态不变：空态卡仍在
    expect(w.find('.mail-empty').exists()).toBe(true)
    w.unmount()
  })
})
