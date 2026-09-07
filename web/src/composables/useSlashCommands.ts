import { ref } from 'vue'
import { useWSAPI } from './useWSAPI'

/**
 * slash 命令补全（2026-08-29 自定义命令；K3 2026-09-06 并入内置命令与
 * 已安装技能）：聊天输入 `/` 时给出命令菜单。
 *
 * 数据真相源 = 后端：
 * - `commands.list` 返回自定义命令表（workspace/config/config.commands.json，
 *   AgentLoop 改写用同一份）+ `builtins`（内置命令名，单一真相源在
 *   nemesis-types 的 BUILTIN_SLASH_COMMANDS——与 AgentLoop 跳过名单同源）；
 * - `skills.installed` 返回已安装技能（后端 `/skill-name` 回落改写为
 *   "Use the {name} skill..."，见 loop.rs rewrite_skill_fallback）。
 *
 * 前端只做提示与填充——真正的模板展开在后端 AgentLoop 入口
 * （rewrite_custom_command），对所有通道生效。同名去重：自定义命令 >
 * 内置 > 技能（与后端解析优先级一致）。
 */

export interface SlashCommand {
  name: string
  description: string
  argument_hint: string
  /** M6（devtool-upgrade 阶段 7）：命令来源徽标。同名去重时先到先得，
   *  所以 source 反映的是实际生效的解析来源（custom > builtin > skill，
   *  与后端 K3 改写优先级一致）。 */
  source: 'custom' | 'builtin' | 'skill'
}

/** 过滤规则：输入以 `/` 开头且不含空白（命令段还在输入中）才给菜单；
 * 名称前缀匹配优先，描述包含兜底。 */
export function filterSlashCommands(
  input: string,
  commands: SlashCommand[],
): SlashCommand[] {
  if (!input.startsWith('/') || /\s/.test(input)) return []
  const q = input.slice(1).toLowerCase()
  if (!commands.length) return []
  const byName = commands.filter(c => c.name.toLowerCase().startsWith(q))
  if (byName.length) return byName
  return commands.filter(c => c.description.toLowerCase().includes(q))
}

export function useSlashCommands() {
  const { request } = useWSAPI()

  const commands = ref<SlashCommand[]>([])
  const loaded = ref(false)

  /** 拉取命令表 + 内置 + 已装技能并合并。已加载且不强制时跳过
   * （打开菜单时 force=false 静默）。技能列表拉取失败不影响命令补全。 */
  async function load(force = false) {
    if (loaded.value && !force) return
    try {
      const data = await request('commands', 'list')
      const merged: SlashCommand[] = (data?.commands || []).map(
        (c: Partial<SlashCommand>) => ({
          name: c.name ?? '',
          description: c.description ?? '',
          argument_hint: c.argument_hint ?? '',
          source: 'custom' as const,
        }),
      )
      // K3：内置命令（名称由后端下发，与 AgentLoop BUILTIN 清单同源）。
      for (const b of data?.builtins || []) {
        pushUnique(merged, {
          name: b.name,
          description: b.description || '内置命令',
          argument_hint: b.argument_hint || '',
          source: 'builtin',
        })
      }
      // K3：已安装技能斜杠化（后端回落改写；菜单提示同形）。
      try {
        const sk = await request('skills', 'installed')
        for (const s of sk?.skills || []) {
          pushUnique(merged, {
            name: s.name,
            description: s.description || '已安装技能',
            argument_hint: s.has_skill_md ? '<任务描述>' : '',
            source: 'skill',
          })
        }
      } catch {
        // 技能列表不可用 → 命令补全照常。
      }
      commands.value = merged
      loaded.value = true
    } catch {
      // 后端不可用 → 无补全，不影响正常输入。
    }
  }

  return { commands, loaded, load }
}

/** 同名去重：先到先得（自定义命令 > 内置 > 技能，与后端解析优先级一致）。 */
function pushUnique(list: SlashCommand[], item: SlashCommand) {
  if (list.some(c => c.name === item.name)) return
  list.push(item)
}
