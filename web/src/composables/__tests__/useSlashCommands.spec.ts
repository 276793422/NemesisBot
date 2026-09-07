import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useSlashCommands, filterSlashCommands, type SlashCommand } from '../useSlashCommands'

// 2026-08-29：自定义 slash 命令补全的过滤逻辑（纯函数）+ 拉取缓存。
// K3（2026-09-06）：load 并入内置命令（commands.list 的 builtins）与
// 已安装技能（skills.installed），同名去重 先自定义 > 内置 > 技能。

const requestMock = vi.fn()
vi.mock('../useWSAPI', () => ({
  useWSAPI: () => ({ request: (...args: any[]) => requestMock(...args) }),
}))

/** 按 (module, cmd) 分发应答——K3 起 load 会发两种请求。 */
function mockBackend(custom: SlashCommand[]) {
  requestMock.mockImplementation((module: string, cmd: string) => {
    if (module === 'commands' && cmd === 'list') {
      return Promise.resolve({
        commands: custom,
        total: custom.length,
        builtins: [
          { name: 'help', description: '显示帮助', argument_hint: '' },
          { name: 'compact', description: '压缩会话上下文', argument_hint: '' },
          { name: 'plan', description: '计划模式（停用文件修改）', argument_hint: '' },
          { name: 'build', description: '切回构建模式', argument_hint: '' },
        ],
      })
    }
    if (module === 'skills' && cmd === 'installed') {
      return Promise.resolve({
        skills: [
          { name: 'weather', has_skill_md: true, description: '天气查询' },
          { name: 'summarize', has_skill_md: true, description: '' },
        ],
      })
    }
    return Promise.reject(new Error(`unexpected request: ${module}.${cmd}`))
  })
}

const TABLE: SlashCommand[] = [
  { name: 'review', description: '代码审查', argument_hint: '<路径>', source: 'custom' },
  { name: 'daily', description: '每日总结', argument_hint: '', source: 'custom' },
  { name: 'deploy', description: '部署相关', argument_hint: '<环境>', source: 'custom' },
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

  it('load 合并自定义 + 内置 + 技能；二次调用走缓存', async () => {
    mockBackend(TABLE)
    const { commands, loaded, load } = useSlashCommands()
    await load()
    expect(loaded.value).toBe(true)
    // 自定义 3 + 内置 3 + 技能 2（无同名冲突）。
    expect(commands.value.map(c => c.name)).toEqual([
      'review', 'daily', 'deploy', 'help', 'compact', 'plan', 'build', 'weather', 'summarize',
    ])
    // 技能带 SKILL.md → 参数提示预填；描述空 → 兜底标签。
    expect(commands.value.find(c => c.name === 'weather')?.argument_hint).toBe('<任务描述>')
    expect(commands.value.find(c => c.name === 'summarize')?.description).toBe('已安装技能')
    expect(requestMock).toHaveBeenCalledTimes(2)
    await load()
    expect(requestMock).toHaveBeenCalledTimes(2)
  })

  it('同名去重：自定义 > 内置 > 技能', async () => {
    mockBackend([
      { name: 'plan', description: '自定义 plan 覆盖内置', argument_hint: '', source: 'custom' },
      { name: 'weather', description: '自定义同名命令', argument_hint: '', source: 'custom' },
    ])
    const { commands, load } = useSlashCommands()
    await load()
    const plan = commands.value.find(c => c.name === 'plan')
    expect(plan?.description).toBe('自定义 plan 覆盖内置')
    // 内置 help 与技能 summarize 照常进入；weather 只出现一次（自定义版）。
    expect(commands.value.filter(c => c.name === 'weather')).toHaveLength(1)
    expect(commands.value.find(c => c.name === 'help')).toBeDefined()
    expect(commands.value.find(c => c.name === 'summarize')).toBeDefined()
  })

  it('技能列表失败 → 命令补全照常（自定义 + 内置）', async () => {
    requestMock.mockImplementation((module: string, cmd: string) => {
      if (module === 'commands') {
        return Promise.resolve({ commands: TABLE, total: 3, builtins: [
          { name: 'help', description: '显示帮助', argument_hint: '' },
        ] })
      }
      return Promise.reject(new Error('skills down'))
    })
    const { commands, loaded, load } = useSlashCommands()
    await load()
    expect(loaded.value).toBe(true)
    expect(commands.value.map(c => c.name)).toEqual([...TABLE.map(c => c.name), 'help'])
  })

  it('后端不可用 → 静默降级为空表', async () => {
    requestMock.mockRejectedValue(new Error('ws down'))
    const { commands, loaded, load } = useSlashCommands()
    await load()
    expect(loaded.value).toBe(false)
    expect(commands.value).toEqual([])
  })
})

// M6（2026-09-07）：source 来源标记——命令面板按来源渲染徽标；同名去重
// 先到先得，source 反映实际生效的解析来源（custom > builtin > skill）。

describe('useSlashCommands source 标记 (M6)', () => {
  beforeEach(() => {
    requestMock.mockReset()
  })

  it('三源各自带正确 source', async () => {
    mockBackend(TABLE)
    const { commands, load } = useSlashCommands()
    await load()
    expect(commands.value.find(c => c.name === 'review')?.source).toBe('custom')
    expect(commands.value.find(c => c.name === 'help')?.source).toBe('builtin')
    expect(commands.value.find(c => c.name === 'weather')?.source).toBe('skill')
  })

  it('同名去重保留胜者（含其 source）', async () => {
    mockBackend([
      { name: 'plan', description: '自定义 plan 覆盖内置', argument_hint: '', source: 'custom' },
      { name: 'weather', description: '自定义同名命令', argument_hint: '', source: 'custom' },
    ])
    const { commands, load } = useSlashCommands()
    await load()
    expect(commands.value.find(c => c.name === 'plan')?.source).toBe('custom')
    expect(commands.value.find(c => c.name === 'weather')?.source).toBe('custom')
  })

  it('内置与技能同名 → 内置胜出', async () => {
    mockBackend([])
    requestMock.mockImplementation((module: string, cmd: string) => {
      if (module === 'commands' && cmd === 'list') {
        return Promise.resolve({
          commands: [],
          total: 0,
          builtins: [{ name: 'weather', description: '内置 weather 覆盖技能', argument_hint: '' }],
        })
      }
      if (module === 'skills' && cmd === 'installed') {
        return Promise.resolve({ skills: [{ name: 'weather', has_skill_md: true, description: '天气' }] })
      }
      return Promise.reject(new Error(`unexpected: ${module}.${cmd}`))
    })
    const { commands, load } = useSlashCommands()
    await load()
    const hits = commands.value.filter(c => c.name === 'weather')
    expect(hits).toHaveLength(1)
    expect(hits[0].source).toBe('builtin')
  })
})
