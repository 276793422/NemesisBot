/**
 * One-click MCP server templates — fill command/args for common servers.
 */

export interface McpPreset {
  id: string
  label: string
  description: string
  transport_type: 'stdio' | 'http' | 'sse'
  /** Command or URL */
  url: string
  args?: string[]
  /** Env keys the user should fill (values blank) */
  envKeys?: string[]
  tags?: string[]
}

export const MCP_PRESETS: McpPreset[] = [
  {
    id: 'filesystem',
    label: '本地文件系统',
    description: '让 Agent 安全读写指定目录（官方 filesystem 服务器）',
    transport_type: 'stdio',
    url: 'npx',
    args: ['-y', '@modelcontextprotocol/server-filesystem', '.'],
    tags: ['本地', '文件'],
  },
  {
    id: 'memory',
    label: '记忆知识图谱',
    description: '持久化实体与关系的记忆 MCP',
    transport_type: 'stdio',
    url: 'npx',
    args: ['-y', '@modelcontextprotocol/server-memory'],
    tags: ['记忆'],
  },
  {
    id: 'fetch',
    label: '网页抓取',
    description: '通过 MCP 抓取网页正文',
    transport_type: 'stdio',
    url: 'npx',
    args: ['-y', '@modelcontextprotocol/server-fetch'],
    tags: ['网络'],
  },
  {
    id: 'github',
    label: 'GitHub',
    description: '仓库与 Issue 操作（需 Personal Access Token）',
    transport_type: 'stdio',
    url: 'npx',
    args: ['-y', '@modelcontextprotocol/server-github'],
    envKeys: ['GITHUB_PERSONAL_ACCESS_TOKEN'],
    tags: ['GitHub'],
  },
  {
    id: 'custom',
    label: '自定义…',
    description: '手动选择传输方式与命令（进阶）',
    transport_type: 'stdio',
    url: '',
    tags: [],
  },
]
