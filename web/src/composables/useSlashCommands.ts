import { ref } from 'vue'
import { useWSAPI } from './useWSAPI'

/**
 * 自定义 slash 命令补全（2026-08-29）：聊天输入 `/` 时给出命令菜单。
 *
 * 数据真相源 = `commands.list` WSAPI（workspace/config/config.commands.json，
 * AgentLoop 改写用同一份）。前端只做提示与填充——真正的模板展开在后端
 * AgentLoop 入口（rewrite_custom_command），对所有通道生效。
 */

export interface SlashCommand {
  name: string
  description: string
  argument_hint: string
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

  /** 拉取命令表。已加载且不强制时跳过（打开菜单时 force=false 静默）。 */
  async function load(force = false) {
    if (loaded.value && !force) return
    try {
      const data = await request('commands', 'list')
      commands.value = data?.commands || []
      loaded.value = true
    } catch {
      // 后端不可用 → 无补全，不影响正常输入。
    }
  }

  return { commands, loaded, load }
}
