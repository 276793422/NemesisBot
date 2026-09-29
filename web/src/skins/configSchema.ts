/**
 * 配置页字段 schema（P2b 契约层 §6.1 `config.schema(pageId)`）。
 *
 * 宿主发布每个配置页的字段 schema（key/label/type/range/options/default），
 * 皮肤脚本按 schema 渲染友好控件、写回走 `config.set_field` WSAPI——不抓
 * DOM、不硬编码字段清单。
 *
 * **漂移防线（v2 修订）**：`__tests__/contract.spec.ts` 遍历
 * `config.default.json` 全部叶子路径，断言「每叶有 schema 映射或显式
 * ignore」；反向断言 schema 每键在 config 里真实存在、声明 type 与默认值
 * 类型一致。新 config 字段未处理 → 测试红（强制显式决策，绝不静默缺失）。
 *
 * default 值运行时从 config.default.json 查（编译期内联，单一真相源——
 * 表内只写 label/type/range，不重复写默认值）。
 */

import configDefault from '../../../nemesisbot/config/config.default.json'

export type SchemaFieldType = 'boolean' | 'number' | 'string' | 'enum'

export interface SchemaField {
  /** config 路径（`config.set_field` 的 path 入参，点分；如 `agents.defaults.temperature`） */
  key: string
  label: string
  type: SchemaFieldType
  /** number 滑块范围（缺省 = 数值输入框） */
  range?: { min: number; max: number; step?: number }
  /** enum 下拉选项 */
  options?: string[]
  /** 秘密字段（token/secret/key）——脚本应按密码框渲染，宿主不回显明文 */
  secret?: boolean
  /** 默认值（config.default.json 编译期快照） */
  default: unknown
}

export interface SchemaPage {
  id: string
  title: string
  fields: SchemaField[]
}

/** 非通道字段的显式映射表（key = config 路径；label/type/range/options）。 */
const FIELDS: Record<string, Omit<SchemaField, 'key' | 'default'>> = {
  // ---- agents ----
  'agents.defaults.workspace': { label: '工作区目录', type: 'string' },
  'agents.defaults.restrict_to_workspace': { label: '限制在工作区内', type: 'boolean' },
  'agents.defaults.llm': { label: '默认模型', type: 'string' },
  'agents.defaults.max_tokens': { label: '单轮最大 token', type: 'number', range: { min: 256, max: 1_000_000, step: 256 } },
  'agents.defaults.temperature': { label: '温度', type: 'number', range: { min: 0, max: 2, step: 0.05 } },
  'agents.defaults.max_tool_iterations': { label: '工具迭代上限', type: 'number', range: { min: 1, max: 1000, step: 1 } },
  'agents.defaults.rate_limit_retries': { label: '限流重试次数', type: 'number', range: { min: 0, max: 100, step: 1 } },
  'agents.defaults.concurrent_request_mode': { label: '并发消息模式', type: 'enum', options: ['queue', 'reject', 'steer'] },
  'agents.defaults.queue_size': { label: '队列长度', type: 'number', range: { min: 1, max: 256, step: 1 } },
  'agents.defaults.max_continuation_permits': { label: '续行并发许可（0=不限）', type: 'number', range: { min: 0, max: 64, step: 1 } },
  // ---- executor ----
  'executor.enabled': { label: '执行体隔离（Layer 1）', type: 'boolean' },
  'executor.sandbox': { label: '沙盒隔离（Layer 2）', type: 'boolean' },
  // ---- terminal ----
  'terminal.enabled': { label: '启用内嵌终端', type: 'boolean' },
  'terminal.max_sessions': { label: '最大会话数', type: 'number', range: { min: 1, max: 32, step: 1 } },
  // ---- gateway ----
  'gateway.host': { label: '监听地址', type: 'string' },
  'gateway.port': { label: '监听端口', type: 'number', range: { min: 1, max: 65535, step: 1 } },
  // ---- tools.web ----
  'tools.web.brave.enabled': { label: 'Brave 搜索', type: 'boolean' },
  'tools.web.brave.api_key': { label: 'Brave API Key', type: 'string', secret: true },
  'tools.web.brave.max_results': { label: 'Brave 结果数上限', type: 'number', range: { min: 1, max: 20, step: 1 } },
  'tools.web.duckduckgo.enabled': { label: 'DuckDuckGo 搜索', type: 'boolean' },
  'tools.web.perplexity.enabled': { label: 'Perplexity 搜索', type: 'boolean' },
  'tools.web.perplexity.api_key': { label: 'Perplexity API Key', type: 'string', secret: true },
  'tools.web.perplexity.max_results': { label: 'Perplexity 结果数上限', type: 'number', range: { min: 1, max: 20, step: 1 } },
  'tools.cron.exec_timeout_minutes': { label: 'Cron 执行超时（分钟）', type: 'number', range: { min: 1, max: 720, step: 1 } },
  // ---- heartbeat ----
  'heartbeat.enabled': { label: '启用心跳', type: 'boolean' },
  'heartbeat.interval': { label: '心跳间隔（秒）', type: 'number', range: { min: 5, max: 3600, step: 5 } },
  // ---- devices ----
  'devices.enabled': { label: '启用设备管理', type: 'boolean' },
  'devices.monitor_usb': { label: '监控 USB 设备', type: 'boolean' },
  // ---- logging ----
  'logging.llm.enabled': { label: '记录 LLM 请求日志', type: 'boolean' },
  'logging.llm.log_dir': { label: 'LLM 日志目录', type: 'string' },
  'logging.llm.detail_level': { label: 'LLM 日志详细度', type: 'enum', options: ['summary', 'full'] },
  'logging.llm.save_raw': { label: '保存原始请求/响应', type: 'boolean' },
  'logging.general.enabled': { label: '启用文件日志', type: 'boolean' },
  'logging.general.enable_console': { label: '控制台输出', type: 'boolean' },
  'logging.general.level': { label: '日志级别', type: 'enum', options: ['TRACE', 'DEBUG', 'INFO', 'WARN', 'ERROR'] },
  'logging.general.file': { label: '日志文件路径', type: 'string' },
  // ---- 顶层开关 ----
  'security.enabled': { label: '启用安全管线（8 层）', type: 'boolean' },
  'skills.enabled': { label: '启用技能系统', type: 'boolean' },
  'forge.enabled': { label: '启用 Forge 自学习', type: 'boolean' },
  'cluster.enabled': { label: '启用集群', type: 'boolean' },
  'memory.enabled': { label: '启用增强记忆', type: 'boolean' },
  'memory.dreaming.enabled': { label: '记忆整理（dreaming）', type: 'boolean' },
  'memory.dreaming.cron': { label: '整理计划（cron）', type: 'string' },
  'memory.dreaming.top_k': { label: '整理检索条数', type: 'number', range: { min: 1, max: 64, step: 1 } },
  'mcp.enabled': { label: '启用 MCP', type: 'boolean' },
}

/** 通道显示名（`channels.<id>.*` 分组标题用）。 */
const CHANNEL_NAMES: Record<string, string> = {
  whatsapp: 'WhatsApp', telegram: 'Telegram', feishu: '飞书', discord: 'Discord',
  maixcam: 'MaixCam', qq: 'QQ', dingtalk: '钉钉', wecom: '企业微信', slack: 'Slack',
  line: 'LINE', onebot: 'OneBot', web: 'Web', websocket: 'WebSocket', external: 'External',
}

/** 通道字段映射（`channels.<id>.<field>` 的 field 段；所有已配置通道共用）。 */
const CHANNEL_FIELDS: Record<string, Omit<SchemaField, 'key' | 'default'>> = {
  enabled: { label: '启用', type: 'boolean' },
  token: { label: 'Bot Token', type: 'string', secret: true },
  proxy: { label: '代理', type: 'string' },
  bridge_url: { label: '桥接地址', type: 'string' },
  app_id: { label: 'App ID', type: 'string' },
  app_secret: { label: 'App Secret', type: 'string', secret: true },
  encrypt_key: { label: '加密 Key', type: 'string', secret: true },
  verification_token: { label: 'Verification Token', type: 'string', secret: true },
  host: { label: '主机地址', type: 'string' },
  port: { label: '端口', type: 'number', range: { min: 1, max: 65535, step: 1 } },
  path: { label: '路径', type: 'string' },
  auth_token: { label: '鉴权 Token', type: 'string', secret: true },
  heartbeat_interval: { label: '心跳间隔（秒）', type: 'number', range: { min: 1, max: 600, step: 1 } },
  session_timeout: { label: '会话超时（秒）', type: 'number', range: { min: 10, max: 86400, step: 10 } },
  webhook_url: { label: 'Webhook 地址', type: 'string' },
  encoding_aes_key: { label: 'Encoding AES Key', type: 'string', secret: true },
  corp_id: { label: 'Corp ID', type: 'string' },
  listen_addr: { label: '监听地址', type: 'string' },
  callback_path: { label: '回调路径', type: 'string' },
  bot_token: { label: 'Bot Token', type: 'string', secret: true },
  app_token: { label: 'App Token', type: 'string', secret: true },
  channel_secret: { label: 'Channel Secret', type: 'string', secret: true },
  channel_access_token: { label: 'Channel Access Token', type: 'string', secret: true },
  webhook_host: { label: 'Webhook 主机', type: 'string' },
  webhook_port: { label: 'Webhook 端口', type: 'number', range: { min: 1, max: 65535, step: 1 } },
  webhook_path: { label: 'Webhook 路径', type: 'string' },
  ws_url: { label: 'WebSocket 地址', type: 'string' },
  access_token: { label: 'Access Token', type: 'string', secret: true },
  reconnect_interval: { label: '重连间隔（秒）', type: 'number', range: { min: 1, max: 600, step: 1 } },
  input_exe: { label: '输入程序', type: 'string' },
  output_exe: { label: '输出程序', type: 'string' },
  chat_id: { label: '会话 ID', type: 'string' },
  client_id: { label: 'Client ID', type: 'string' },
  client_secret: { label: 'Client Secret', type: 'string', secret: true },
}

/** 页标题（pageId → 中文标题；pageId = config 顶层段名）。 */
const PAGE_TITLES: Record<string, string> = {
  agents: 'Agent 默认', executor: '执行体隔离', terminal: '内嵌终端', gateway: '网关',
  tools: '工具', heartbeat: '心跳', devices: '设备', logging: '日志', security: '安全',
  skills: '技能', forge: 'Forge', cluster: '集群', memory: '记忆', mcp: 'MCP',
  channels: '通道',
}

/** 编译期默认值按路径查找（drift 测试同时保证此查找不落空）。 */
function defaultAt(path: string): unknown {
  let cur: unknown = configDefault
  for (const seg of path.split('.')) {
    if (cur && typeof cur === 'object' && seg in (cur as Record<string, unknown>)) {
      cur = (cur as Record<string, unknown>)[seg]
    } else {
      return undefined
    }
  }
  return cur
}

/** 渠道内层字段 → SchemaField（channels.<id>.<field>）。 */
function channelField(key: string): SchemaField | null {
  const parts = key.split('.')
  if (parts.length !== 3 || parts[0] !== 'channels') return null
  const [, id, field] = parts
  const ent = CHANNEL_FIELDS[field]
  if (!ent) return null
  const name = CHANNEL_NAMES[id] || id
  return { key, default: defaultAt(key), ...ent, label: `${name} · ${ent.label}` }
}

/** 所有已映射 schema 字段（扁平；测试与分页共用）。 */
export function allSchemaFields(): SchemaField[] {
  const out: SchemaField[] = []
  for (const [key, ent] of Object.entries(FIELDS)) {
    out.push({ key, default: defaultAt(key), ...ent })
  }
  // 通道字段按 config.default.json 实际在场的通道展开（配置里没有的通道
  // 不产幽灵字段；在场通道缺映射字段 = drift 测试红）
  const channels = ((configDefault as Record<string, unknown>).channels ?? {}) as Record<
    string,
    Record<string, unknown>
  >
  for (const id of Object.keys(channels).sort()) {
    for (const field of Object.keys(channels[id]).sort()) {
      const f = channelField(`channels.${id}.${field}`)
      if (f) out.push(f)
    }
  }
  return out
}

/** 全部页 id（`NemesisSkin.config.pages()` 载荷）。 */
export function schemaPageIds(): string[] {
  return Object.keys(PAGE_TITLES)
}

/**
 * 按页取 schema（`NemesisSkin.config.schema(pageId)`；未知页返回 null）。
 * 字段按 key 排序（通道页内同通道字段相邻，确定性输出便于脚本渲染）。
 */
export function configSchema(pageId: string): SchemaPage | null {
  const title = PAGE_TITLES[pageId]
  if (!title) return null
  const fields = allSchemaFields()
    .filter((f) => f.key === pageId || f.key.startsWith(pageId + '.'))
    .sort((a, b) => a.key.localeCompare(b.key))
  return { id: pageId, title, fields }
}
