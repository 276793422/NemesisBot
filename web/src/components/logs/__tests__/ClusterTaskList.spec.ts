import { mount } from '@vue/test-utils'
import { describe, it, expect } from 'vitest'
import ClusterTaskList from '../ClusterTaskList.vue'
import { makeTask } from './fixtures'

describe('ClusterTaskList 详情', () => {
  it('渲染视角切换按钮（本机 / 对端）', () => {
    const wrapper = mount(ClusterTaskList, {
      props: { tasks: [makeTask()], selectedId: 'taskAbC123', requests: [] },
    })
    const btns = wrapper.findAll('.perspective-btn')
    expect(btns.length).toBe(2)
    expect(wrapper.text()).toContain('本机视角')
    expect(wrapper.text()).toContain('对端视角')
  })

  it('无 relatedRequestId 时对端视角按钮 disabled', () => {
    const wrapper = mount(ClusterTaskList, {
      props: { tasks: [makeTask({ relatedRequestId: undefined })], selectedId: 'taskAbC123', requests: [] },
    })
    const remoteBtn = wrapper.findAll('.perspective-btn')[1]
    expect(remoteBtn.attributes('disabled')).toBeDefined()
  })

  it('渲染迭代内容', () => {
    const wrapper = mount(ClusterTaskList, {
      props: { tasks: [makeTask()], selectedId: 'taskAbC123', requests: [] },
    })
    expect(wrapper.findAll('.iteration-card').length).toBe(1)
    expect(wrapper.text()).toContain('远端状态正常')
  })
})
