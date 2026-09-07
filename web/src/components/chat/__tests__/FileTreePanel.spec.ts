import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises } from '@vue/test-utils'

// M4：FileTreePanel —— 折叠态默认（localStorage 记忆）、展开拉根树、
// 懒展开（children:null 点目录再查一层）、点击文件 @path 进输入框、
// 错误透出、truncated 提示。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import FileTreePanel from '../FileTreePanel.vue'
import { useChatStore } from '../../../stores/chat'

function rootTree() {
  return {
    path: '',
    truncated: false,
    entries: [
      { name: 'src', path: 'src', type: 'dir', children: null },
      { name: 'README.md', path: 'README.md', type: 'file' },
    ],
  }
}

function mountPanel() {
  return mount(FileTreePanel)
}

beforeEach(() => {
  localStorage.clear()
  setActivePinia(createPinia())
  requestMock.mockReset()
})

describe('FileTreePanel', () => {
  it('默认折叠成细条；点开拉 fs.tree 根', async () => {
    requestMock.mockResolvedValue(rootTree())
    const wrapper = mountPanel()
    expect(wrapper.find('.filetree-panel').exists()).toBe(false)
    expect(wrapper.find('.filetree-rail').exists()).toBe(true)
    // 折叠态不请求数据。
    expect(requestMock).not.toHaveBeenCalled()

    await wrapper.find('.filetree-rail').trigger('click')
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('fs', 'tree', {})
    const rows = wrapper.findAll('.filetree-row')
    expect(rows.length).toBe(2)
    expect(rows[0].text()).toContain('src')
    expect(rows[1].text()).toContain('README.md')
  })

  it('children:null 目录点击 → 懒展开查子层并原位填充', async () => {
    requestMock.mockResolvedValue(rootTree())
    const wrapper = mountPanel()
    await wrapper.find('.filetree-rail').trigger('click')
    await flushPromises()

    requestMock.mockResolvedValue({
      path: 'src',
      truncated: false,
      entries: [{ name: 'main.rs', path: 'src/main.rs', type: 'file' }],
    })
    await wrapper.findAll('.filetree-row')[0].trigger('click')
    await flushPromises()

    expect(requestMock).toHaveBeenLastCalledWith('fs', 'tree', { path: 'src', depth: 1 })
    const rows = wrapper.findAll('.filetree-row')
    expect(rows.length).toBe(3)
    expect(rows[1].text()).toContain('main.rs')
  })

  it('已展开的目录再点收起（不再请求）', async () => {
    requestMock.mockResolvedValue(rootTree())
    const wrapper = mountPanel()
    await wrapper.find('.filetree-rail').trigger('click')
    await flushPromises()

    requestMock.mockResolvedValue({
      path: 'src',
      truncated: false,
      entries: [{ name: 'main.rs', path: 'src/main.rs', type: 'file' }],
    })
    const srcRow = wrapper.findAll('.filetree-row')[0]
    await srcRow.trigger('click') // 展开
    await flushPromises()
    await wrapper.findAll('.filetree-row')[0].trigger('click') // 收起
    await flushPromises()

    expect(requestMock).toHaveBeenCalledTimes(2) // 根 + 懒展开一次，无重复
    expect(wrapper.findAll('.filetree-row').length).toBe(2)
  })

  it('点击文件 → 输入框追加 @path（尾带空白）', async () => {
    requestMock.mockResolvedValue(rootTree())
    const wrapper = mountPanel()
    await wrapper.find('.filetree-rail').trigger('click')
    await flushPromises()

    const chat = useChatStore()
    chat.input = '看一下'
    await wrapper.findAll('.filetree-row')[1].trigger('click') // README.md
    expect(chat.input).toBe('看一下 @README.md ')
  })

  it('输入框为空/尾空白时追加不留双空格', async () => {
    requestMock.mockResolvedValue(rootTree())
    const wrapper = mountPanel()
    await wrapper.find('.filetree-rail').trigger('click')
    await flushPromises()

    const chat = useChatStore()
    await wrapper.findAll('.filetree-row')[1].trigger('click')
    expect(chat.input).toBe('@README.md ')
  })

  it('根请求失败 → 错误透出 + 重试按钮', async () => {
    requestMock.mockRejectedValue(new Error('workspace not found: /x'))
    const wrapper = mountPanel()
    await wrapper.find('.filetree-rail').trigger('click')
    await flushPromises()

    expect(wrapper.find('.filetree-error').text()).toContain('workspace not found')
    expect(wrapper.find('.filetree-row').exists()).toBe(false)

    requestMock.mockResolvedValue(rootTree())
    await wrapper.find('.filetree-error .filetree-btn').trigger('click')
    await flushPromises()
    expect(wrapper.findAll('.filetree-row').length).toBe(2)
  })

  it('truncated → 诚实提示行', async () => {
    requestMock.mockResolvedValue({ ...rootTree(), truncated: true })
    const wrapper = mountPanel()
    await wrapper.find('.filetree-rail').trigger('click')
    await flushPromises()

    expect(wrapper.find('.filetree-truncated').text()).toContain('500')
  })
})
