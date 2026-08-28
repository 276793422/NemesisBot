import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useSlashCommands, filterSlashCommands, type SlashCommand } from '../useSlashCommands'

// 2026-08-29：自定义 slash 命令补全的过滤逻辑（纯函数）+ 拉取缓存。

const requestMock = vi.fn()
vi.mock('../useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

const TABLE: SlashCommand[] = [
  { name: 'review', description: '代码审查', argument_hint: '<路径>' },
  { name: 'daily', description: '每日总结', argument_hint: '' },
  { name: 'deploy', description: '部署相关', argument_hint: '<环境>' },
]

describe('filterSlashCommands', () => {
  it('非 / 开头 → 空菜单', () => {
    expect(filterSlashCommands('普通消息', TABLE)).toEqual([])
  })

  it('命令段含空白（已进入参数）→ 空菜单', () => {
    expect(filterSlashCommands('/review src/main.rs', TABLE)).toEqual([])
  })

  it('名称前缀匹配', () => {
    const r = filterSlashCommands('/de', TABLE)
    expect(r.map(c => c.name)).toEqual(['deploy'])
  })

  it('描述包含兜底（前缀无命中时）', () => {
    const r = filterSlashCommands('/总', TABLE)
    expect(r.map(c => c.name)).toEqual(['daily'])
  })

  it('大小写不敏感', () => {
    const r = filterSlashCommands('/REVIEW', TABLE)
    expect(r.map(c => c.name)).toEqual(['review'])
  })
})

describe('useSlashCommands 拉取', () => {
  beforeEach(() => {
    requestMock.mockReset()
  })

  it('load 拉取命令表；二次调用走缓存', async () => {
    requestMock.mockResolvedValue({ commands: TABLE, total: 3 })
    const { commands, loaded, load } = useSlashCommands()
    await load()
    expect(loaded.value).toBe(true)
    expect(commands.value.length).toBe(3)
    expect(requestMock).toHaveBeenCalledTimes(1)
    await load()
    expect(requestMock).toHaveBeenCalledTimes(1)
  })

  it('后端不可用 → 静默降级为空表', async () => {
    requestMock.mockRejectedValue(new Error('ws down'))
    const { commands, loaded, load } = useSlashCommands()
    await load()
    expect(loaded.value).toBe(false)
    expect(commands.value).toEqual([])
  })
})
