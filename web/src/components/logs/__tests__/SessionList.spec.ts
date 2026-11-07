import { mount, type VueWrapper } from '@vue/test-utils'
import { describe, it, expect } from 'vitest'
import SessionList from '../SessionList.vue'
import { makeSession } from './fixtures'

describe('SessionList 详情面板', () => {
  it('渲染对话气泡：user 右、assistant 左，内容正确', () => {
    const wrapper = mount(SessionList, {
      props: { sessions: [makeSession()], selectedId: 'web_chat1', requests: [] },
    })
    const msgs = wrapper.findAll('.chat-msg')
    expect(msgs.length).toBe(2)
    expect(msgs[0].classes()).toContain('user')
    expect(msgs[1].classes()).toContain('assistant')
    expect(wrapper.text()).toContain('你好')
    expect(wrapper.text()).toContain('有什么可以帮你')
  })

  it('assistant 消息 toolCalls>0 时显示"查看 N 次调用"按钮', () => {
    const session = makeSession({
      messages: [
        { role: 'user', content: '读文件', timestamp: '2026-06-27T10:00:00+08:00' },
        { role: 'assistant', content: '好的', timestamp: '2026-06-27T10:00:01+08:00', toolCalls: 3 },
      ],
    })
    const wrapper = mount(SessionList, {
      props: { sessions: [session], selectedId: 'web_chat1', requests: [{ id: 'r1' }] as any },
    })
    const jumpBtns = wrapper.findAll('.msg-jump')
    expect(jumpBtns.length).toBe(1) // 仅 assistant 那条
    expect(jumpBtns[0].text()).toContain('3')
  })

  it('选中 messages 为空时自动 emit select 触发加载', () => {
    const session = makeSession({ messages: [] })
    const wrapper = mount(SessionList, {
      props: { sessions: [session], selectedId: null, requests: [] },
    })
    expect(wrapper.emitted('select')).toBeTruthy()
    expect(wrapper.emitted('select')![0]).toEqual(['web_chat1'])
  })
})

describe('SessionList 时间过滤器（BUG-20261106-028）', () => {
  const listItemIds = (wrapper: VueWrapper<any>) =>
    wrapper.findAll('.list-item .item-id').map(n => n.text())

  it('默认 all 显示全部；选「今天」隐藏非今日会话', async () => {
    const stale = makeSession({ id: 'web_stale', messages: [] }) // fixture 固定 2026-06-27，恒为非今日
    const fresh = makeSession({ id: 'web_fresh', lastTime: new Date().toISOString(), messages: [] })
    const wrapper = mount(SessionList, {
      props: { sessions: [stale, fresh], selectedId: null, requests: [] },
    })
    expect(listItemIds(wrapper)).toEqual(['web_stale', 'web_fresh'])
    await wrapper.find('select.form-select').setValue('today')
    expect(listItemIds(wrapper)).toEqual(['web_fresh'])
  })

  it('「今天」窗口用 48h 前时间戳隐藏（与运行日期无关的确定性断言）', async () => {
    const twoDaysAgo = makeSession({
      id: 'web_2d',
      lastTime: new Date(Date.now() - 2 * 86_400_000).toISOString(),
      messages: [],
    })
    const wrapper = mount(SessionList, {
      props: { sessions: [twoDaysAgo], selectedId: null, requests: [] },
    })
    await wrapper.find('select.form-select').setValue('today')
    expect(listItemIds(wrapper)).toEqual([])
  })

  it('选「本周」：本周时间戳可见、fixture 旧日期（恒上上周以前）隐藏', async () => {
    const fresh = makeSession({ id: 'web_fresh', lastTime: new Date().toISOString(), messages: [] })
    const stale = makeSession({ id: 'web_stale', messages: [] })
    const wrapper = mount(SessionList, {
      props: { sessions: [fresh, stale], selectedId: null, requests: [] },
    })
    await wrapper.find('select.form-select').setValue('week')
    expect(listItemIds(wrapper)).toEqual(['web_fresh'])
  })

  it('不可解析 lastTime 不过滤（诚实降级，不因格式意外隐藏条目）', async () => {
    const bad = makeSession({ id: 'web_bad', lastTime: 'not-a-timestamp', messages: [] })
    const wrapper = mount(SessionList, {
      props: { sessions: [bad], selectedId: null, requests: [] },
    })
    await wrapper.find('select.form-select').setValue('today')
    expect(listItemIds(wrapper)).toEqual(['web_bad'])
  })
})
