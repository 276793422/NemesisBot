import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount, flushPromises, DOMWrapper } from '@vue/test-utils'

// M6（2026-09-07）：Ctrl+K 命令面板——三源列表渲染（来源徽标）、过滤、
// 键盘导航、执行语义（「插入不发送」：commandDraft 投递 + 关面板 +
// 非聊天视图导航回 /）。数据源 useSlashCommands 打桩；filterSlashCommands
// 用真实现（保证面板过滤与输入框补全同规则）。组件 Teleport 到 body，
// DOM 断言走 document 查询（test-utils wrapper 之外）。

const commandsMock = vi.fn()
vi.mock('../../composables/useSlashCommands', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../composables/useSlashCommands')>()
  return {
    ...actual,
    useSlashCommands: () => ({
      commands: { value: commandsMock() },
      loaded: { value: true },
      load: vi.fn(),
    }),
  }
})

const pushMock = vi.fn().mockResolvedValue(undefined)
let currentPath = '/'
vi.mock('vue-router', () => ({
  useRouter: () => ({
    push: (...a: any[]) => pushMock(...a),
    currentRoute: { value: { path: currentPath } },
  }),
}))

import CommandPalette from '../CommandPalette.vue'
import { useCommandPalette } from '../../composables/useCommandPalette'
import { useChatStore } from '../../stores/chat'
import type { SlashCommand } from '../../composables/useSlashCommands'

const TABLE: SlashCommand[] = [
  { name: 'review', description: '代码审查', argument_hint: '<路径>', source: 'custom' },
  { name: 'deploy', description: '部署相关', argument_hint: '<环境>', source: 'custom' },
  { name: 'help', description: '显示帮助', argument_hint: '', source: 'builtin' },
  { name: 'weather', description: '天气查询', argument_hint: '<任务描述>', source: 'skill' },
]

const q = (sel: string) => new DOMWrapper(document.querySelector(sel))
const qa = (sel: string) => [...document.querySelectorAll(sel)].map(el => new DOMWrapper(el))

beforeEach(() => {
  setActivePinia(createPinia())
  commandsMock.mockReset().mockReturnValue(TABLE)
  pushMock.mockClear()
  currentPath = '/'
  document.body.innerHTML = ''
  const palette = useCommandPalette()
  palette.close()
})

async function openPalette() {
  const palette = useCommandPalette()
  palette.open()
  const w = mount(CommandPalette)
  await flushPromises()
  return w
}

describe('CommandPalette (M6)', () => {
  it('visible=false 不渲染；打开后列出三源命令带徽标', async () => {
    const palette = useCommandPalette()
    const w = mount(CommandPalette)
    expect(document.querySelector('.palette-backdrop')).toBeNull()

    palette.open()
    await flushPromises()
    expect(document.querySelector('.palette-backdrop')).not.toBeNull()
    const items = qa('.palette-item')
    expect(items).toHaveLength(4)
    expect(items[0].find('.palette-badge').text()).toBe('自定义')
    expect(items[2].find('.palette-badge').text()).toBe('内置')
    expect(items[3].find('.palette-badge').text()).toBe('技能')
    w.unmount()
  })

  it('输入按 / 前缀过滤（与输入框补全同规则）；无命中显示空态', async () => {
    const w = await openPalette()
    await q('.palette-input').setValue('de')
    let names = qa('.palette-item .palette-cmd').map(n => n.text())
    expect(names).toEqual(['/deploy'])

    await q('.palette-input').setValue('zzz')
    expect(document.querySelector('.palette-empty')).not.toBeNull()
    w.unmount()
  })

  it('ArrowDown/ArrowUp 移动高亮（循环）；Enter 执行当前高亮项', async () => {
    const w = await openPalette()
    const input = q('.palette-input')
    expect(qa('.palette-item.active')).toHaveLength(1)
    expect(qa('.palette-item')[0].classes()).toContain('active')

    await input.trigger('keydown', { key: 'ArrowDown' })
    expect(qa('.palette-item')[1].classes()).toContain('active')
    await input.trigger('keydown', { key: 'ArrowUp' })
    await input.trigger('keydown', { key: 'ArrowUp' })
    // 回绕到末尾。
    expect(qa('.palette-item')[3].classes()).toContain('active')

    await input.trigger('keydown', { key: 'Enter' })
    const chat = useChatStore()
    expect(chat.commandDraft).toBe('/weather ')
    expect(useCommandPalette().visible.value).toBe(false)
    w.unmount()
  })

  it('执行 = 插入不发送：投递 commandDraft 并关面板；非聊天视图先导航回 /', async () => {
    currentPath = '/logs'
    const w = await openPalette()
    await qa('.palette-item')[0].trigger('mousedown')
    const chat = useChatStore()
    expect(chat.commandDraft).toBe('/review ')
    expect(pushMock).toHaveBeenCalledWith('/')
    expect(useCommandPalette().visible.value).toBe(false)
    w.unmount()
  })

  it('聊天视图执行不触发导航', async () => {
    const w = await openPalette()
    await qa('.palette-item')[2].trigger('mousedown')
    expect(pushMock).not.toHaveBeenCalled()
    w.unmount()
  })

  it('Escape 关面板', async () => {
    const w = await openPalette()
    await q('.palette-input').trigger('keydown', { key: 'Escape' })
    expect(document.querySelector('.palette-backdrop')).toBeNull()
    w.unmount()
  })
})
