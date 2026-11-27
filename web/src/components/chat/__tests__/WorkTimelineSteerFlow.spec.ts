// 插话指示唯一性 · store→组件集成测试（2026-11-27 修复回归锁·第二层）。
//
// 与 WorkTimelineBlock.spec.ts 手搓 fixture 的差别：这里**不手搓 work prop**，
// 而是按后端真实帧序驱动 chat store 的摄取函数（ingestWorkRoundText /
// ingestWorkToolStart / ingestWorkToolFinish / ingestSteerNote /
// ingestSteerResponse——与 gateway WorkRecorder 事件同构），再用 store 构建
// 的 pendingWork（真实数据形状）渲染真实组件，数 DOM。
// 「底层的数据上报全部模拟、只测 UI 现象」——本文件即该主张的落地。
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount } from '@vue/test-utils'

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>()
  return { ...actual, useWSAPI: () => ({ request: requestMock, connected: { value: true } }) }
})
vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn() }),
}))

import { useChatStore, type WorkTimeline } from '../../../stores/chat'
import WorkTimelineBlock from '../WorkTimelineBlock.vue'

/** 按生产帧序构造一个多轮时间线（注记前 3 个「叙述+工具」轮），停在
 *  「注记已入列、回应未落地」的未收口窗口——用户截图的缺陷现场形状。 */
function steerLiveWork() {
  const store = useChatStore()
  store.reset()
  for (let i = 1; i <= 3; i++) {
    store.ingestWorkRoundText(`第${i}轮叙述`)
    store.ingestWorkToolStart(`call-${i}`, 'exec', `{"cmd":"check-${i}"}`)
    store.ingestWorkToolFinish(`call-${i}`, true, 100, 'ok')
  }
  // 轮边界认领：⚡ 注记帧到场（live 载体帧，ChatPanel 会话过滤后摄取）
  store.ingestSteerNote('⚡ 插话已注入本轮思考：先确认进度')
  return store
}

function mountLive(work: WorkTimeline | null) {
  return mount(WorkTimelineBlock, {
    props: { work: work!, live: true, steerLiveText: '' },
  })
}

beforeEach(() => {
  setActivePinia(createPinia())
  requestMock.mockReset()
})

describe('插话指示唯一性 · store 摄取帧序 → 组件渲染（2026-11-27）', () => {
  it('未收口窗口：注记 1 条、指示行 1 条且紧跟注记（3 历史轮不复制）', () => {
    const store = steerLiveWork()
    const w = mountLive(store.pendingWork)
    // 注记恰好 1 条（3 轮历史 + 注记轮，缺陷态注记仍 1 条——注记本身不受
    // 缺陷影响，受影响的是指示行）
    expect(w.findAll('.wt-steer-note')).toHaveLength(1)
    // 指示行恰好 1 条——缺陷态这里是 4（每轮一条）
    const indicators = w.findAll('.wt-steer-working')
    expect(indicators).toHaveLength(1)
    // 位置：紧跟注记行（注记 → 指示 → 后续内容）
    const children = Array.from(w.find('.wt-body').element.children)
    const noteIdx = children.findIndex((el) => el.classList.contains('wt-steer-note'))
    const indIdx = children.findIndex((el) => el.classList.contains('wt-steer-working'))
    expect(noteIdx).toBeGreaterThanOrEqual(0)
    expect(indIdx).toBe(noteIdx + 1)
    // 历史轮零污染：指示行之前恰好只有 1 条注记行（工具行 + 叙述 + 注记）
    const before = children.slice(0, indIdx).filter((el) => el.classList.contains('wt-steer-note'))
    expect(before).toHaveLength(1)
    w.unmount()
  })

  it('收口回应落地：指示行消失、回应段恰 1 条（同一 store 对象连续摄取）', async () => {
    const store = steerLiveWork()
    const w = mountLive(store.pendingWork)
    expect(w.findAll('.wt-steer-working')).toHaveLength(1)
    // steer_response 瞬态帧到场 → store 反查注记轮落回应段 → 组件热更新
    store.ingestSteerResponse('插话已处理：进度正常')
    await w.vm.$nextTick()
    expect(w.findAll('.wt-steer-working')).toHaveLength(0)
    expect(w.findAll('.wt-steer-response')).toHaveLength(1)
    expect(w.find('.wt-steer-response-body').text()).toBe('插话已处理：进度正常')
    w.unmount()
  })

  it('流式正文预览卡同族恰 1 条（steerLiveText 非空窗口）', () => {
    const store = steerLiveWork()
    const w = mount(WorkTimelineBlock, {
      props: { work: store.pendingWork!, live: true, steerLiveText: '生成中的正文…' },
    })
    expect(w.findAll('.wt-steer-live-text')).toHaveLength(1)
    expect(w.findAll('.wt-steer-working')).toHaveLength(1)
    w.unmount()
  })

  it('无插话的多轮时间线零指示行（窗口条件不得被无关轮误开）', () => {
    const store = useChatStore()
    store.reset()
    for (let i = 1; i <= 4; i++) {
      store.ingestWorkRoundText(`第${i}轮叙述`)
      store.ingestWorkToolStart(`call-${i}`, 'exec', `{"cmd":"x-${i}"}`)
      store.ingestWorkToolFinish(`call-${i}`, true, 50, 'ok')
    }
    const w = mountLive(store.pendingWork)
    expect(w.findAll('.wt-steer-working')).toHaveLength(0)
    expect(w.findAll('.wt-steer-note')).toHaveLength(0)
    w.unmount()
  })
})
