// WorkTimelineBlock spawn 行钻取测试（2026-11-06 B 档）。
//
// 契约：spawn 行显示任务文本（替代原始参数 JSON），可点击；点击按任务
// 文本匹配 subagents.list 台账行 → router.push /executors?tab=subagents&focus=<id>；
// 台账缺席/WS 未就绪回落执行单元页（无 focus）；非 spawn 行点击不导航。
// spawnTaskFromArgs 纯函数（截断 JSON 正则前缀提取 + 转义还原）钉死。
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

const requestMock = vi.fn()
const pushMock = vi.fn()

vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: requestMock, connected: { value: true } }),
}))
vi.mock('vue-router', () => ({
  useRouter: () => ({ push: pushMock }),
}))

import WorkTimelineBlock from '../WorkTimelineBlock.vue'
import { spawnTaskFromArgs } from '../workLabels'

function block(spawnArgs?: string) {
  return mount(WorkTimelineBlock, {
    props: {
      work: {
        started_at_ms: 1000,
        ended_at_ms: 2000,
        rounds: [
          {
            text: '',
            tools: [
              {
                tool: 'spawn',
                call_id: 's1',
                ok: true,
                duration_ms: 4200,
                args_preview: spawnArgs ?? '{"task":"修复登录验证","agent_id":"coder","tools":"full"}',
                result_preview: '',
              },
              {
                tool: 'exec',
                call_id: 'e1',
                ok: true,
                duration_ms: 300,
                args_preview: '{"cmd":"cargo test"}',
                result_preview: 'ok',
              },
            ],
          },
        ],
      },
      live: false,
    },
  })
}

/** 落地态默认折叠——先点开头部才能看到工具行。 */
async function openBlock(spawnArgs?: string) {
  const w = block(spawnArgs)
  await w.find('.wt-header').trigger('click')
  return w
}

beforeEach(() => {
  requestMock.mockReset()
  pushMock.mockReset()
})

describe('spawnTaskFromArgs（纯函数）', () => {
  it('完整 JSON：parse 取 task 字段', () => {
    expect(spawnTaskFromArgs('{"task":"写个报告","tools":"full"}')).toBe('写个报告')
  })

  it('截断 JSON：正则抠出 task 前缀（保留截断伪影 …）', () => {
    const truncated = '{"task":"修复登录验证模块并在完成后运行全量回归测试，包括单元测试与集成测…'
    expect(spawnTaskFromArgs(truncated)).toBe('修复登录验证模块并在完成后运行全量回归测试，包括单元测试与集成测…')
  })

  it('截断 JSON 内含转义字符：捕获组回包 JSON.parse 还原', () => {
    const truncated = '{"task":"第一步 \\"跑测试\\"\\n第二步看结果…'
    expect(spawnTaskFromArgs(truncated)).toBe('第一步 "跑测试"\n第二步看结果…')
  })

  it('无 task 字段返回空串', () => {
    expect(spawnTaskFromArgs('{"tools":"full"}')).toBe('')
    expect(spawnTaskFromArgs('')).toBe('')
  })
})

describe('WorkTimelineBlock spawn 行钻取', () => {
  it('spawn 行显示任务文本（替代参数 JSON），非 spawn 行保持参数原文', async () => {
    const w = await openBlock()
    const tools = w.findAll('.wt-tool')
    expect(tools[0].find('.wt-tool-task').text()).toBe('修复登录验证')
    expect(tools[0].classes()).toContain('wt-tool-spawn')
    // 非 spawn 行：无 wt-tool-task，参数原文走旧通道
    expect(tools[1].find('.wt-tool-task').exists()).toBe(false)
    expect(tools[1].find('.wt-tool-args').text()).toBe('{"cmd":"cargo test"}')
  })

  it('点击 spawn 行：按任务匹配台账行 → 深链 focus', async () => {
    requestMock.mockResolvedValue({
      tasks: [
        { id: 'subagent:hit', task: '修复登录验证' },
        { id: 'subagent:other', task: '别的任务' },
      ],
    })
    const w = await openBlock()
    await w.findAll('.wt-tool')[0].trigger('click')
    await flushPromises()
    expect(pushMock).toHaveBeenCalledWith({ path: '/executors', query: { tab: 'subagents', focus: 'subagent:hit' } })
  })

  it('台账无精确匹配时前缀回落（截断任务文本）', async () => {
    requestMock.mockResolvedValue({
      tasks: [{ id: 'subagent:px', task: '修复登录验证模块并在完成后运行全量回归测试' }],
    })
    const w = await openBlock('{"task":"修复登录验证模块并在完成后运行全量回归…')
    await w.findAll('.wt-tool')[0].trigger('click')
    await flushPromises()
    expect(pushMock).toHaveBeenCalledWith({ path: '/executors', query: { tab: 'subagents', focus: 'subagent:px' } })
  })

  it('台账缺席（过旧被逐出）→ 回落执行单元页无 focus', async () => {
    requestMock.mockResolvedValue({ tasks: [{ id: 'subagent:x', task: '毫不相干' }] })
    const w = await openBlock()
    await w.findAll('.wt-tool')[0].trigger('click')
    await flushPromises()
    expect(pushMock).toHaveBeenCalledWith({ path: '/executors', query: { tab: 'subagents' } })
  })

  it('WS 未就绪 → 回落执行单元页', async () => {
    requestMock.mockRejectedValue(new Error('ws down'))
    const w = await openBlock()
    await w.findAll('.wt-tool')[0].trigger('click')
    await flushPromises()
    expect(pushMock).toHaveBeenCalledWith({ path: '/executors', query: { tab: 'subagents' } })
  })

  it('非 spawn 行点击不导航', async () => {
    requestMock.mockResolvedValue({ tasks: [] })
    const w = await openBlock()
    await w.findAll('.wt-tool')[1].trigger('click')
    await flushPromises()
    expect(pushMock).not.toHaveBeenCalled()
    expect(requestMock).not.toHaveBeenCalled()
  })
})

// ---------------------------------------------------------------------------
// B4（2026-11-09）轮内时序：思考 → 应答 → 操作（真实时间流顺序——思考先
// 于叙述产生，渲染序必须一致）。
// ---------------------------------------------------------------------------

describe('WorkTimelineBlock 轮内时序（B4）', () => {
  function blockWith(text: string, reasoning: string) {
    return mount(WorkTimelineBlock, {
      props: {
        work: {
          started_at_ms: 1000,
          ended_at_ms: 2000,
          rounds: [
            {
              text,
              reasoning,
              tools: [
                {
                  tool: 'exec',
                  call_id: 'e1',
                  ok: true,
                  duration_ms: 300,
                  args_preview: '{"cmd":"ls"}',
                },
              ],
            },
          ],
        },
        live: false,
      },
    })
  }

  it('轮内思考渲染在叙述之前（思考 → 应答 → 操作）', async () => {
    const w = blockWith('我先看下目录', '先想清楚改哪里再动手')
    await w.find('.wt-header').trigger('click')
    const body = w.find('.wt-body')
    const children = body.element.children
    expect(children.length).toBeGreaterThanOrEqual(3)
    expect(children[0].classList.contains('wt-think')).toBe(true)
    expect(children[1].classList.contains('wt-round-text')).toBe(true)
    expect(children[2].classList.contains('wt-tool-group')).toBe(true)
    expect(w.find('details.wt-think .wt-think-body').text()).toBe('先想清楚改哪里再动手')
    expect(w.find('.wt-round-text').text()).toBe('我先看下目录')
  })

  it('只有叙述的轮无思考卡；只有思考的轮无叙述行', async () => {
    const w = blockWith('', '只想不说')
    await w.find('.wt-header').trigger('click')
    expect(w.find('.wt-round-text').exists()).toBe(false)
    expect(w.find('details.wt-think').exists()).toBe(true)
    const w2 = blockWith('只有叙述', '')
    await w2.find('.wt-header').trigger('click')
    expect(w2.find('details.wt-think').exists()).toBe(false)
    expect(w2.find('.wt-round-text').exists()).toBe(true)
  })
})

// ---------------------------------------------------------------------------
// B3（2026-11-09）流式思考卡：liveThinking 非空 = 渲染增长中思考卡（默认
// 展开）；空 = 不渲染；live 态增长发 grown 供宿主贴底。
// ---------------------------------------------------------------------------

describe('WorkTimelineBlock 流式思考卡（B3）', () => {
  function liveBlock(liveThinking: string) {
    return mount(WorkTimelineBlock, {
      props: {
        work: { started_at_ms: 1000, ended_at_ms: 2000, rounds: [] },
        live: true,
        liveThinking,
      },
    })
  }

  it('非空渲染思考中卡（默认展开），空不渲染', () => {
    const w = liveBlock('第一段思考')
    const card = w.find('details.wt-think-live')
    expect(card.exists()).toBe(true)
    expect(card.attributes('open')).toBeDefined()
    expect(card.find('.wt-think-body').text()).toBe('第一段思考')
    expect(card.find('summary').text()).toContain('思考中')
    const w2 = liveBlock('')
    expect(w2.find('details.wt-think-live').exists()).toBe(false)
  })

  it('live 态思考增长发 grown 事件（贴底信号）', async () => {
    const w = liveBlock('一')
    expect(w.emitted('grown')).toBeUndefined()
    await w.setProps({ liveThinking: '一' + '二'.repeat(64) })
    expect(w.emitted('grown')?.length).toBe(1)
  })

  it('落地态（live=false）思考增长不发 grown', async () => {
    const w = mount(WorkTimelineBlock, {
      props: {
        work: { started_at_ms: 1000, ended_at_ms: 2000, rounds: [] },
        live: false,
        liveThinking: '一',
      },
    })
    await w.setProps({ liveThinking: '一二三' })
    expect(w.emitted('grown')).toBeUndefined()
  })
})
