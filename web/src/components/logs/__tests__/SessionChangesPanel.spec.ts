import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

// M3：SessionChangesPanel —— file_changes 聚合（文件→次数/kind）、点击
// 拉取 diff、无注记 diff 渲染、诚实注记（无差异/文件缺失）、错误透出、
// 会话切换清态、无变更不渲染。

const requestMock = vi.fn()
vi.mock('../../../composables/useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

import SessionChangesPanel from '../SessionChangesPanel.vue'

const messagesWithChanges = [
  { role: 'assistant', content: 'r1', file_changes: [{ path: 'src/a.rs', kind: 'Modify' }] },
  { role: 'user', content: 'q2' },
  {
    role: 'assistant',
    content: 'r2',
    file_changes: [
      { path: 'src/a.rs', kind: 'Modify' },
      { path: 'src/b.rs', kind: 'Create' },
    ],
  },
]

function fileDiff(over: Partial<any> = {}) {
  return {
    path: 'src/a.rs',
    backend: 'git',
    base_turn: 1,
    diff: '--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,1 +1,1 @@\n-old line\n+new line\n',
    head_on_disk: true,
    note: '',
    session_id: 's1',
    ...over,
  }
}

beforeEach(() => {
  requestMock.mockReset()
})

describe('SessionChangesPanel', () => {
  it('无 file_changes 时整个面板不渲染', () => {
    const wrapper = mount(SessionChangesPanel, {
      props: { session: 's1', messages: [{ role: 'user', content: 'hi' }] },
    })
    expect(wrapper.find('.changes-panel').exists()).toBe(false)
  })

  it('默认折叠；展开后按文件聚合并显示次数（跨行去重合并 kind）', async () => {
    const wrapper = mount(SessionChangesPanel, {
      props: { session: 's1', messages: messagesWithChanges },
    })
    expect(wrapper.find('.changes-body').exists()).toBe(false)

    await wrapper.find('.changes-toggle').trigger('click')
    const files = wrapper.findAll('.change-file')
    expect(files.length).toBe(2)
    // 按次数降序：a.rs ×2 在前。
    expect(files[0].text()).toContain('src/a.rs')
    expect(files[0].text()).toContain('×2')
    expect(files[1].text()).toContain('src/b.rs')
    expect(files[1].text()).toContain('×1')
    expect(wrapper.find('.changes-toggle').text()).toContain('2 文件 / 3 次')
  })

  it('点击文件拉 sessions.file_diff 并渲染 diff；note 空不显示提示行', async () => {
    requestMock.mockResolvedValue(fileDiff())
    const wrapper = mount(SessionChangesPanel, {
      props: { session: 's1', messages: messagesWithChanges },
    })
    await wrapper.find('.changes-toggle').trigger('click')
    await wrapper.findAll('.change-file')[0].trigger('click')
    await flushPromises()

    expect(requestMock).toHaveBeenCalledWith('sessions', 'file_diff', {
      session_id: 's1',
      path: 'src/a.rs',
    })
    const code = wrapper.find('.diff-code code')
    expect(code.exists()).toBe(true)
    expect(code.html()).toContain('hljs-deletion')
    expect(code.html()).toContain('hljs-addition')
    expect(wrapper.find('.diff-hint').exists()).toBe(false)
  })

  it('诚实注记（无差异/文件不在盘）作为提示行显示', async () => {
    requestMock.mockResolvedValue(
      fileDiff({ diff: '', note: '当前内容与基线无差异（可能已被回退或手动恢复）' }),
    )
    const wrapper = mount(SessionChangesPanel, {
      props: { session: 's1', messages: messagesWithChanges },
    })
    await wrapper.find('.changes-toggle').trigger('click')
    await wrapper.findAll('.change-file')[0].trigger('click')
    await flushPromises()

    expect(wrapper.find('.diff-hint').text()).toContain('无差异')
    expect(wrapper.find('.diff-code').exists()).toBe(false)
  })

  it('后端错误透出到面板（不静默）', async () => {
    requestMock.mockRejectedValue(new Error('该文件无 checkpoint 基线'))
    const wrapper = mount(SessionChangesPanel, {
      props: { session: 's1', messages: messagesWithChanges },
    })
    await wrapper.find('.changes-toggle').trigger('click')
    await wrapper.findAll('.change-file')[0].trigger('click')
    await flushPromises()

    expect(wrapper.find('.diff-error').text()).toContain('无 checkpoint 基线')
  })

  it('会话切换清选中态与 diff 内容', async () => {
    requestMock.mockResolvedValue(fileDiff())
    const wrapper = mount(SessionChangesPanel, {
      props: { session: 's1', messages: messagesWithChanges },
    })
    await wrapper.find('.changes-toggle').trigger('click')
    await wrapper.findAll('.change-file')[0].trigger('click')
    await flushPromises()
    expect(wrapper.find('.diff-code').exists()).toBe(true)

    await wrapper.setProps({ session: 's2' })
    expect(wrapper.find('.diff-code').exists()).toBe(false)
    expect(wrapper.find('.change-file.active').exists()).toBe(false)
  })
})
