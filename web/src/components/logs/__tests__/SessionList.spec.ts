import { mount } from '@vue/test-utils'
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
