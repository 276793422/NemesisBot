import { describe, it, expect, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useChatStore } from '../chat'

// 2026-11-08（子进程实时输出与完成唤醒）：exec 执行中输出尾窗摄取——
// ingestWorkToolOutput 倒序挂最后一个 running 条目（call_id 置空语义）、
// 无 running 条目诚实忽略、Finish 清 live_tail（终态以 result_preview 接棒）。

beforeEach(() => {
  setActivePinia(createPinia())
})

describe('chat store ingestWorkToolOutput (2026-11-08)', () => {
  it('挂到最后一个 running 条目（call_id 置空语义：倒序找 running）', () => {
    const store = useChatStore()
    // 分组规则（后端 ensure_tool_round 同构）：叙述轮只吃第一个工具批次，
    // 第二个 Start 开纯工具轮 → c1 在叙述轮、c2 在新工具轮。
    store.ingestWorkRoundText('看看输出')
    store.ingestWorkToolStart('c1', 'exec', 'ping a')
    store.ingestWorkToolStart('c2', 'grep', 'pat')
    store.ingestWorkToolOutput('PING a 64 bytes\n', 18)
    const rounds = store.pendingWork?.rounds ?? []
    expect(rounds).toHaveLength(2)
    // c1（先启动）不带尾窗，c2（最新 running）携带。
    expect(rounds[0].tools[0].live_tail).toBeUndefined()
    expect(rounds[1].tools[0].live_tail).toBe('PING a 64 bytes\n')
    expect(rounds[1].tools[0].live_total_bytes).toBe(18)
  })

  it('replacement 窗整体替换（同条目重复摄取 = 覆盖不叠加）', () => {
    const store = useChatStore()
    store.ingestWorkToolStart('c1', 'exec', '')
    store.ingestWorkToolOutput('chunk one\n', 10)
    store.ingestWorkToolOutput('chunk one\nchunk two\n', 20)
    const t = store.pendingWork?.rounds.at(-1)?.tools[0]
    expect(t?.live_tail).toBe('chunk one\nchunk two\n')
    expect(t?.live_total_bytes).toBe(20)
  })

  it('无 running 条目（全部已 Finish / 无 Started）诚实忽略', () => {
    const store = useChatStore()
    store.ingestWorkToolStart('c1', 'exec', '')
    store.ingestWorkToolFinish('c1', true, 5, 'done')
    store.ingestWorkToolOutput('stray\n', 6)
    const t = store.pendingWork?.rounds.at(-1)?.tools[0]
    expect(t?.live_tail).toBeUndefined()
    expect(t?.result_preview).toBe('done')
  })

  it('无 pendingWork（直答零开销）不炸', () => {
    const store = useChatStore()
    expect(() => store.ingestWorkToolOutput('x\n', 2)).not.toThrow()
  })

  it('Finish 清 live_tail / live_total_bytes（终态以 result_preview 接棒）', () => {
    const store = useChatStore()
    store.ingestWorkToolStart('c1', 'exec', '')
    store.ingestWorkToolOutput('running tail\n', 13)
    store.ingestWorkToolFinish('c1', true, 1500, 'full result here')
    const t = store.pendingWork?.rounds.at(-1)?.tools[0]
    expect(t?.live_tail).toBeUndefined()
    expect(t?.live_total_bytes).toBeUndefined()
    expect(t?.result_preview).toBe('full result here')
    expect(t?.running).toBeUndefined()
  })

  it('多轮时间线：只在最新轮的 running 条目挂载（跨轮不误挂）', () => {
    const store = useChatStore()
    // 第一轮：叙述 + 已完成工具。
    store.ingestWorkRoundText('先跑个命令')
    store.ingestWorkToolStart('c1', 'exec', '')
    store.ingestWorkToolFinish('c1', true, 5, 'r1')
    // 第二轮：新 running。
    store.ingestWorkToolStart('c2', 'exec', 'ping b')
    store.ingestWorkToolOutput('reply 2\n', 8)
    const rounds = store.pendingWork?.rounds ?? []
    expect(rounds).toHaveLength(2)
    expect(rounds[0].tools[0].live_tail).toBeUndefined()
    expect(rounds[1].tools[0].live_tail).toBe('reply 2\n')
  })
})
