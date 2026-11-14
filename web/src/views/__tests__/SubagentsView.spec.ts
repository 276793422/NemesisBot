// SubagentsView 详情请求围栏测试（R575 BUG-20261106-006 + R578 BUG-20261106-009）：
// 加载期 backdrop 尚未显示表格仍可点，两次在飞 openDetail 的响应按完成序覆盖
// ——迟到响应必须被代次围栏整包丢弃；换详情还须拆旧 poll（定时器/在飞响应/
// 载入窗口 WS 帧三臂都不得把旧任务事件灌进新时间线）。WSAPI / WebSocket /
// vue-router / toast 全 mock，不碰真实后端。
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'

const requestMock = vi.fn()

const wsMocks = vi.hoisted(() => ({
  addMessageHandler: vi.fn(),
  removeMessageHandler: vi.fn(),
}))
vi.mock('../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: requestMock, connected: { value: true } }),
}))
vi.mock('../../composables/useWebSocket', () => ({
  addMessageHandler: wsMocks.addMessageHandler,
  removeMessageHandler: wsMocks.removeMessageHandler,
  useWebSocket: () => ({
    addMessageHandler: wsMocks.addMessageHandler,
    removeMessageHandler: wsMocks.removeMessageHandler,
    connect: vi.fn(),
    sendMessage: vi.fn(),
  }),
}))
const toastMocks = vi.hoisted(() => ({
  success: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
}))
vi.mock('../../composables/useToast', () => ({
  useToast: () => toastMocks,
}))
const routerMocks = vi.hoisted(() => ({
  replace: vi.fn(),
  push: vi.fn(),
}))
vi.mock('vue-router', () => ({
  useRoute: () => ({ query: {} }),
  useRouter: () => ({ replace: routerMocks.replace, push: routerMocks.push }),
}))

import SubagentsView from '../SubagentsView.vue'

function deferred<T>() {
  let resolve!: (v: T) => void
  let reject!: (e: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

function row(id: string, task: string, running = false) {
  return {
    id,
    parent_session_key: 'web:sess-1',
    parent_chat_id: 'sess-1',
    origin: 'detached',
    agent_id: 'agent-1',
    task,
    role: '',
    tools_profile: '',
    depth: 0,
    background: false,
    loop_origin: 'main',
    status: 'done',
    started_at_ms: 1,
    running,
  }
}

async function mountView(props?: Record<string, unknown>) {
  const w = mount(SubagentsView, { props })
  await flushPromises()
  return w
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
  wsMocks.addMessageHandler.mockClear()
  wsMocks.removeMessageHandler.mockClear()
  toastMocks.success.mockClear()
  toastMocks.warn.mockClear()
  toastMocks.error.mockClear()
  routerMocks.replace.mockReset()
  requestMock.mockImplementation((_m: string, cmd: string) => {
    if (cmd === 'list') return Promise.resolve({ tasks: [] })
    if (cmd === 'events') return Promise.resolve({ events: [], running: false })
    return Promise.resolve({})
  })
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('SubagentsView 详情请求围栏（R575）', () => {
  it('先点慢任务 A 再点快任务 B：A 迟到响应不把详情换回 A', async () => {
    const slowDetail = deferred<any>()
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('a', '慢任务 AAA'), row('b', '快任务 BBB')] })
      if (cmd === 'detail') {
        return data.id === 'a' ? slowDetail.promise : Promise.resolve(row('b', '快任务 BBB'))
      }
      if (cmd === 'events') return Promise.resolve({ events: [], running: false })
      return Promise.resolve({})
    })
    const w = await mountView()
    const rows = w.findAll('.subagent-row')

    // 点 A（detail 请求挂起——backdrop 未显示，表格仍可点）。
    await rows[0].trigger('click')
    // 点 B（detail/events 立即返回，modal 渲染 B）。
    await w.findAll('.subagent-row')[1].trigger('click')
    await flushPromises()
    expect(w.find('.modal-body').text()).toContain('快任务 BBB')
    expect(w.find('.modal-body').text()).not.toContain('慢任务 AAA')

    // A 迟到返回：围栏丢弃，详情仍是 B。
    slowDetail.resolve(row('a', '慢任务 AAA'))
    await flushPromises()
    expect(w.find('.modal-body').text()).toContain('快任务 BBB')
    expect(w.find('.modal-body').text()).not.toContain('慢任务 AAA')
    w.unmount()
  })

  it('先点慢任务 A 再点快任务 B：A 迟到拒绝不把详情翻成「任务不在册」', async () => {
    const slowDetail = deferred<any>()
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('a', '慢任务 AAA'), row('b', '快任务 BBB')] })
      if (cmd === 'detail') {
        return data.id === 'a' ? slowDetail.promise : Promise.resolve(row('b', '快任务 BBB'))
      }
      if (cmd === 'events') return Promise.resolve({ events: [], running: false })
      return Promise.resolve({})
    })
    const w = await mountView()
    await w.findAll('.subagent-row')[0].trigger('click')
    await w.findAll('.subagent-row')[1].trigger('click')
    await flushPromises()

    slowDetail.reject(new Error('boom'))
    await flushPromises()
    expect(w.find('.modal-body').text()).toContain('快任务 BBB')
    expect(w.text()).not.toContain('任务不在册')
    w.unmount()
  })

  it('无并发时详情正常打开（围栏不误伤正常路径）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('a', '普通任务')] })
      if (cmd === 'detail') return Promise.resolve(row('a', '普通任务'))
      if (cmd === 'events') return Promise.resolve({ events: [], running: false })
      return Promise.resolve({})
    })
    const w = await mountView()
    await w.findAll('.subagent-row')[0].trigger('click')
    await flushPromises()
    expect(w.find('.modal-body').text()).toContain('普通任务')
    // 正常路径关闭后 modal 消失。
    await w.find('.modal-close').trigger('click')
    expect(w.find('.modal-body').exists()).toBe(false)
    w.unmount()
  })
})

// --- R578（BUG-20261106-009）：换详情围栏的 poll/WS 两臂 ---
// req 闸只挡首载响应；旧任务的 2s 轮询定时器、已在飞的 poll 响应、载入
// 窗口的实时帧都会把 A 的事件灌进 B 已清空的时间线。

describe('SubagentsView 详情 poll 围栏（R578）', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('换详情先拆旧 poll：B 载入窗口不再拉 A 事件，B 时间线不含 A 历史', async () => {
    vi.useFakeTimers()
    const aEventsCalls = () =>
      requestMock.mock.calls.filter((c: any[]) => c[1] === 'events' && c[2]?.id === 'a').length
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('a', '任务甲'), row('b', '任务乙')] })
      if (cmd === 'detail') {
        return Promise.resolve(data.id === 'a' ? row('a', '任务甲', true) : row('b', '任务乙'))
      }
      if (cmd === 'events') {
        if (data.id === 'a') {
          return Promise.resolve({ events: [{ content: '甲事件', role: 'assistant' }], running: true })
        }
        return Promise.resolve({ events: [{ content: '乙事件', role: 'assistant' }], running: false })
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    // 打开 A（running=true → poll 装弹），确认轮询在走。
    await w.findAll('.subagent-row')[0].trigger('click')
    await flushPromises()
    expect(aEventsCalls()).toBe(1)
    vi.advanceTimersByTime(2100)
    expect(aEventsCalls()).toBe(2)

    // 点 B：旧 poll 必须当场拆除——推进 4.5s 不得再产生 A 的 events 调用
    // （B running=false 不装新 poll，任何新增都来自漏拆的旧 poll）。
    await w.findAll('.subagent-row')[1].trigger('click')
    await flushPromises()
    const afterSwitch = aEventsCalls()
    vi.advanceTimersByTime(4500)
    expect(aEventsCalls()).toBe(afterSwitch)
    // B 时间线只含乙事件——甲事件的历史不得回灌。
    const tl = w.find('.subagent-timeline').text()
    expect(tl).toContain('乙事件')
    expect(tl).not.toContain('甲事件')
    w.unmount()
  })

  it('在飞 poll 响应迟到：换代后整包丢弃不入新时间线', async () => {
    vi.useFakeTimers()
    const inFlightA = deferred<any>()
    let aCalls = 0
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('a', '任务甲'), row('b', '任务乙')] })
      if (cmd === 'detail') {
        return Promise.resolve(data.id === 'a' ? row('a', '任务甲', true) : row('b', '任务乙'))
      }
      if (cmd === 'events') {
        if (data.id === 'a') {
          aCalls++
          // 首拉（开详情）即时；后续 tick 挂起制造在飞响应。
          return aCalls === 1
            ? Promise.resolve({ events: [{ content: '甲事件', role: 'assistant' }], running: true })
            : inFlightA.promise
        }
        return Promise.resolve({ events: [{ content: '乙事件', role: 'assistant' }], running: false })
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    await w.findAll('.subagent-row')[0].trigger('click')
    await flushPromises()
    // 制造在飞 poll：tick 已发出、响应挂起。
    vi.advanceTimersByTime(2100)
    await flushPromises()
    // 载入 B（B events 即时返回，running=false 不装新 poll）。
    await w.findAll('.subagent-row')[1].trigger('click')
    await flushPromises()
    // A 的在飞 poll 响应迟到到达：代次围栏丢弃，不得入 B 时间线。
    inFlightA.resolve({ events: [{ content: '甲迟到污染', role: 'assistant' }], running: false })
    await flushPromises()
    const tl = w.find('.subagent-timeline').text()
    expect(tl).toContain('乙事件')
    expect(tl).not.toContain('甲迟到污染')
    w.unmount()
  })

  it('载入窗口的旧任务实时帧不入新时间线（WS 臂）', async () => {
    const bDetailPending = deferred<any>()
    requestMock.mockImplementation((_m: string, cmd: string, data: any) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('a', '任务甲'), row('b', '任务乙')] })
      if (cmd === 'detail') {
        return data.id === 'b' ? bDetailPending.promise : Promise.resolve(row('a', '任务甲'))
      }
      if (cmd === 'events') {
        return Promise.resolve({ events: [{ content: `${data.id}事件`, role: 'assistant' }], running: false })
      }
      return Promise.resolve({})
    })
    const w = await mountView()
    // 打开 A，拿到 wsHandler（addMessageHandler 注册的回调）。
    await w.findAll('.subagent-row')[0].trigger('click')
    await flushPromises()
    const handler = wsMocks.addMessageHandler.mock.calls.at(-1)?.[0] as (f: any) => void
    expect(typeof handler).toBe('function')

    // 点 B：B detail 挂起 → 载入窗口（detail.value 仍是 A）。旧任务实时帧
    // 到达——不得灌进已清空的 timeline。
    await w.findAll('.subagent-row')[1].trigger('click')
    await flushPromises()
    handler({ type: 'push', cmd: 'tool_event', data: { subagent_id: 'a', kind: 'RoundText', seq: 99, data: { content: '甲载入窗帧' } } })
    await flushPromises()

    // B 就位：时间线只含 B 的事件（mock 生成 "b事件"），甲的载入窗帧被闸。
    bDetailPending.resolve(row('b', '任务乙'))
    await flushPromises()
    const tl = w.find('.subagent-timeline').text()
    expect(tl).toContain('b事件')
    expect(tl).not.toContain('甲载入窗帧')
    w.unmount()
  })
})

// ===========================================================================
// 归档生命周期（2026-11-15）：archived prop 双模式 / 工作面归档面 / 归档
// TAB 恢复·删除·清空 / confirm 纪律（归档恢复不弹、删除清空必弹）/
// archived-count emit / 删除后详情模态显式关闭。
// ===========================================================================

function findBtn(w: any, text: string) {
  return w.findAll('button').find((b: any) => b.text().trim() === text)
}

describe('SubagentsView 归档模式', () => {
  it('归档模式：list 请求带 archived=only；emit archived-count（全库投影）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('a', '归档任务')], archived_count: 5 })
      return Promise.resolve({})
    })
    const w = await mountView({ archived: true })
    expect(requestMock).toHaveBeenCalledWith('subagents', 'list', { archived: 'only' })
    expect(w.emitted('archived-count')?.at(-1)).toEqual([5])
    w.unmount()
  })

  it('工作面：缺省请求不带 archived；也 emit archived-count', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [], archived_count: 3 })
      return Promise.resolve({})
    })
    const w = await mountView()
    expect(requestMock).toHaveBeenCalledWith('subagents', 'list', {})
    expect(w.emitted('archived-count')?.at(-1)).toEqual([3])
    w.unmount()
  })

  it('工作面：终态行「归档」钮不弹确认；运行中行是「终止」钮', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('a', '终态任务'), row('b', '运行任务', true)] })
      return Promise.resolve({})
    })
    const confirmSpy = vi.fn(() => true)
    vi.stubGlobal('confirm', confirmSpy)
    const w = await mountView()
    const rows = w.findAll('.subagent-row')
    const btns0 = rows[0].findAll('button').map((b: any) => b.text().trim())
    expect(btns0).toContain('归档')
    expect(btns0).not.toContain('终止')
    const btns1 = rows[1].findAll('button').map((b: any) => b.text().trim())
    expect(btns1).toContain('终止')
    expect(btns1).not.toContain('归档')
    await rows[0].findAll('button').find((b: any) => b.text().trim() === '归档')!.trigger('click')
    await flushPromises()
    expect(confirmSpy).not.toHaveBeenCalled()
    expect(requestMock).toHaveBeenCalledWith('subagents', 'archive', { ids: ['a'] })
    w.unmount()
  })

  it('工作面工具行「归档全部终态」：只批量归档终态行（运行中行不进 ids）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('a', '终态'), row('b', '运行中', true)] })
      return Promise.resolve({})
    })
    const w = await mountView()
    await findBtn(w, '归档全部终态')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('subagents', 'archive', { ids: ['a'] })
    w.unmount()
  })

  it('归档模式：恢复不弹确认；删除必弹确认（delegate 行附 B 端回灌注记）', async () => {
    const del = row('d', '委派任务')
    del.origin = 'delegate'
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('u', '普通归档'), del] })
      return Promise.resolve({})
    })
    const confirmSpy = vi.fn((_note?: string) => true)
    vi.stubGlobal('confirm', confirmSpy)
    const w = await mountView({ archived: true })
    const rows = w.findAll('.subagent-row')
    const btns0 = rows[0].findAll('button').map((b: any) => b.text().trim())
    expect(btns0).toContain('恢复')
    expect(btns0).toContain('删除')
    expect(btns0).not.toContain('归档')

    await rows[0].findAll('button').find((b: any) => b.text().trim() === '恢复')!.trigger('click')
    await flushPromises()
    expect(confirmSpy).not.toHaveBeenCalled()
    expect(requestMock).toHaveBeenCalledWith('subagents', 'unarchive', { ids: ['u'] })

    await rows[1].findAll('button').find((b: any) => b.text().trim() === '删除')!.trigger('click')
    await flushPromises()
    expect(confirmSpy).toHaveBeenCalledTimes(1)
    expect(String(confirmSpy.mock.calls[0][0])).toContain('B 端结果回灌将无处落')
    expect(requestMock).toHaveBeenCalledWith('subagents', 'remove', { ids: ['d'] })
    w.unmount()
  })

  it('归档模式工具行「清空已归档」：confirm 拒绝不发请求；确认则批量 remove 全部行', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('x', '甲'), row('y', '乙')] })
      return Promise.resolve({})
    })
    vi.stubGlobal('confirm', vi.fn(() => false))
    const w = await mountView({ archived: true })
    await findBtn(w, '清空已归档')!.trigger('click')
    await flushPromises()
    expect(requestMock.mock.calls.filter((c: any[]) => c[1] === 'remove').length).toBe(0)

    vi.stubGlobal('confirm', vi.fn(() => true))
    await findBtn(w, '清空已归档')!.trigger('click')
    await flushPromises()
    expect(requestMock).toHaveBeenCalledWith('subagents', 'remove', { ids: ['x', 'y'] })
    w.unmount()
  })

  it('删除正查看的行后详情模态显式关闭（不悬空轮询 404）', async () => {
    requestMock.mockImplementation((_m: string, cmd: string) => {
      if (cmd === 'list') return Promise.resolve({ tasks: [row('a', '任务甲')] })
      if (cmd === 'detail') return Promise.resolve(row('a', '任务甲'))
      if (cmd === 'events') return Promise.resolve({ events: [], running: false })
      return Promise.resolve({})
    })
    vi.stubGlobal('confirm', vi.fn(() => true))
    const w = await mountView({ archived: true })
    await w.findAll('.subagent-row')[0].trigger('click')
    await flushPromises()
    expect(w.find('.modal-body').exists()).toBe(true)
    await w.findAll('.subagent-row')[0].findAll('button').find((b: any) => b.text().trim() === '删除')!.trigger('click')
    await flushPromises()
    expect(w.find('.modal-body').exists()).toBe(false)
    w.unmount()
  })
})
