import { mount } from '@vue/test-utils'
import { describe, it, expect } from 'vitest'
import RequestList from '../RequestList.vue'
import { makeRequest } from './fixtures'
import { parseDirTimestamp } from '../mockData'

describe('RequestList 迭代详情', () => {
  it('渲染迭代卡片，数量 = iterations，显示迭代序号', () => {
    const wrapper = mount(RequestList, {
      props: { requests: [makeRequest()], selectedId: '2026-06-27_10-00-00_r1', sessions: [], tasks: [] },
    })
    expect(wrapper.findAll('.iteration-card').length).toBe(1)
    expect(wrapper.text()).toContain('迭代 0')
  })

  it('点击 header 展开/折叠 iteration-body，展开后渲染工具调用', async () => {
    const wrapper = mount(RequestList, {
      props: { requests: [makeRequest()], selectedId: '2026-06-27_10-00-00_r1', sessions: [], tasks: [] },
    })
    expect(wrapper.find('.iteration-body').exists()).toBe(false)
    await wrapper.find('.iteration-header').trigger('click')
    expect(wrapper.find('.iteration-body').exists()).toBe(true)
    expect(wrapper.find('.iteration-body').text()).toContain('read_file')
    expect(wrapper.find('.iteration-body').text()).toContain('file body')
  })

  it('关联按钮按条件出现（有所属会话 + 关联集群任务）', () => {
    const wrapper = mount(RequestList, {
      props: {
        requests: [makeRequest({ sessionId: 'web_chat1', clusterTaskId: 'task1' })],
        selectedId: '2026-06-27_10-00-00_r1',
        sessions: [{ id: 'web_chat1' } as any],
        tasks: [{ id: 'task1' } as any],
      },
    })
    expect(wrapper.text()).toContain('所属会话')
    expect(wrapper.text()).toContain('关联集群任务')
  })
})

describe('RequestList 模型过滤（BUG-20261106-028 横向）', () => {
  it('过滤选项从数据派生：含 provider 前缀的真实 model 串，无硬编码字面量', () => {
    const wrapper = mount(RequestList, {
      props: {
        requests: [
          makeRequest({ id: 'r1', model: 'zhipu/glm-4.7' }),
          makeRequest({ id: 'r2', model: 'claude-opus-4-8' }),
        ],
        selectedId: 'r1',
        sessions: [],
        tasks: [],
      },
    })
    const values = wrapper.findAll('select.form-select option').map((o: any) => o.element.value as string)
    expect(values).toContain('all')
    expect(values).toContain('zhipu/glm-4.7')
    expect(values).toContain('claude-opus-4-8')
    // 硬编码字面量不得回流（旧实现的两枚固定选项对带前缀的实际 model 串恒不命中）
    expect(values).not.toContain('glm-4.7')
    expect(values).not.toContain('claude-sonnet-4-5')
  })

  it('选中某模型后列表只剩该模型的条目', async () => {
    const wrapper = mount(RequestList, {
      props: {
        requests: [
          makeRequest({ id: 'r1', model: 'zhipu/glm-4.7' }),
          makeRequest({ id: 'r2', model: 'claude-opus-4-8' }),
        ],
        selectedId: 'r1',
        sessions: [],
        tasks: [],
      },
    })
    await wrapper.find('select.form-select').setValue('claude-opus-4-8')
    const ids = wrapper.findAll('.list-item .item-id').map(n => n.text())
    expect(ids).toEqual(['r2'])
  })
})

describe('RequestList 杂务徽标 + 用户消息全文（本地记录不截断）', () => {
  it('isAux 条目打「杂务」徽标，普通条目不打', () => {
    const wrapper = mount(RequestList, {
      props: {
        requests: [
          makeRequest({ id: 'r1' }),
          makeRequest({ id: 'r2', isAux: true }),
        ],
        selectedId: 'r1',
        sessions: [],
        tasks: [],
      },
    })
    const badges = wrapper.findAll('.item-aux-badge')
    expect(badges.length).toBe(1)
    expect(badges[0].text()).toBe('杂务')
  })

  it('fullMessage 渲染「用户消息全文」卡（默认展开，可折叠），与迭代卡片分开', async () => {
    const wrapper = mount(RequestList, {
      props: {
        requests: [makeRequest({ fullMessage: '全文第一行\n全文第二行' })],
        selectedId: '2026-06-27_10-00-00_r1',
        sessions: [],
        tasks: [],
      },
    })
    expect(wrapper.find('.fullmsg-card').exists()).toBe(true)
    expect(wrapper.find('.fullmsg-pre').text()).toContain('全文第一行')
    // 折叠切换不打扰迭代卡
    await wrapper.find('.fullmsg-card .iteration-header').trigger('click')
    expect(wrapper.find('.fullmsg-pre').exists()).toBe(false)
    expect(wrapper.findAll('.iteration-card').length).toBe(1)
  })

  it('无 fullMessage 不渲染全文卡（列表态/旧数据兼容）', () => {
    const wrapper = mount(RequestList, {
      props: { requests: [makeRequest()], selectedId: '2026-06-27_10-00-00_r1', sessions: [], tasks: [] },
    })
    expect(wrapper.find('.fullmsg-card').exists()).toBe(false)
    expect(wrapper.findAll('.iteration-card').length).toBe(1)
  })
})

describe('parseDirTimestamp 目录名时间戳解析', () => {
  it('下划线目录名（含/不含微秒）→ 有效 Date；此前 new Date 恒 Invalid（NaN天前）', () => {
    const d1 = parseDirTimestamp('2026-06-27_10-00-00')
    expect(d1.getTime()).not.toBeNaN()
    expect(d1.getHours()).toBe(10)
    const d2 = parseDirTimestamp('2026-06-27_10-00-05.123456')
    expect(d2.getTime()).not.toBeNaN()
    expect(d2.getSeconds()).toBe(5)
    expect(d2.getMilliseconds()).toBe(123)
    // ISO 串回落 new Date 原行为
    expect(parseDirTimestamp('2026-06-27T10:00:00+08:00').getTime()).not.toBeNaN()
  })
})
