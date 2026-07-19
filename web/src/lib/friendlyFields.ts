/**
 * Map raw config keys → human labels / control types for simple forms.
 * Supports: toggle, text, password, number, slider, select, range, readonly
 */

export type FieldKind = 'toggle' | 'text' | 'password' | 'number' | 'slider' | 'select' | 'range' | 'readonly'

export interface SelectOption {
  label: string
  value: string | number | boolean
}

export interface FriendlyField {
  key: string
  label: string
  kind: FieldKind
  hint?: string
  /** For slider: min/max/step */
  min?: number
  max?: number
  step?: number
  /** For select: options */
  options?: SelectOption[]
  /** For range: array of preset values */
  presets?: (string | number)[]
  /** Unit suffix (e.g. '秒', 'MB') */
  unit?: string
}

/** Common channel config keys */
export const CHANNEL_FIELD_META: Record<string, FriendlyField> = {
  enabled: { key: 'enabled', label: '启用', kind: 'toggle' },
  token: { key: 'token', label: 'Bot Token', kind: 'password', hint: '从 BotFather / 开发者后台获取' },
  bot_token: { key: 'bot_token', label: 'Bot Token', kind: 'password' },
  app_id: { key: 'app_id', label: 'App ID', kind: 'text' },
  app_secret: { key: 'app_secret', label: 'App Secret', kind: 'password' },
  client_id: { key: 'client_id', label: 'Client ID', kind: 'text' },
  client_secret: { key: 'client_secret', label: 'Client Secret', kind: 'password' },
  channel_access_token: { key: 'channel_access_token', label: '访问令牌', kind: 'password' },
  channel_secret: { key: 'channel_secret', label: 'Channel Secret', kind: 'password' },
  verification_token: { key: 'verification_token', label: 'Verification Token', kind: 'password' },
  encrypt_key: { key: 'encrypt_key', label: '加密密钥', kind: 'password' },
  access_token: { key: 'access_token', label: 'Access Token', kind: 'password' },
  ws_url: { key: 'ws_url', label: 'WebSocket 地址', kind: 'text' },
  webhook_path: { key: 'webhook_path', label: 'Webhook 路径', kind: 'text' },
  webhook_port: { key: 'webhook_port', label: 'Webhook 端口', kind: 'number' },
  host: { key: 'host', label: '监听地址', kind: 'text' },
  port: { key: 'port', label: '端口', kind: 'number' },
}

/** Security policy keys — now with sliders and selects */
export const SECURITY_FIELD_META: Record<string, FriendlyField> = {
  enabled: { key: 'enabled', label: '启用安全策略', kind: 'toggle' },
  default_action: {
    key: 'default_action',
    label: '默认动作',
    kind: 'select',
    options: [
      { label: '允许 (allow)', value: 'allow' },
      { label: '拒绝 (deny)', value: 'deny' },
    ],
    hint: '未匹配规则时的默认行为',
  },
  approval_timeout_seconds: {
    key: 'approval_timeout_seconds',
    label: '审批超时',
    kind: 'slider',
    min: 30,
    max: 3600,
    step: 30,
    unit: '秒',
    hint: '审批请求自动过期时间',
  },
  audit_log_file_enabled: { key: 'audit_log_file_enabled', label: '写入审计日志文件', kind: 'toggle' },
  audit_log_retention_days: {
    key: 'audit_log_retention_days',
    label: '审计保留天数',
    kind: 'slider',
    min: 7,
    max: 365,
    step: 7,
    unit: '天',
  },
  log_all_operations: { key: 'log_all_operations', label: '记录全部操作', kind: 'toggle' },
  log_denials_only: { key: 'log_denials_only', label: '仅记录拒绝', kind: 'toggle' },
  max_pending_requests: {
    key: 'max_pending_requests',
    label: '最大待审批数',
    kind: 'slider',
    min: 1,
    max: 100,
    step: 1,
    unit: '个',
  },
  synchronous_mode: { key: 'synchronous_mode', label: '同步模式', kind: 'toggle' },
  audit_log_path: { key: 'audit_log_path', label: '审计日志路径', kind: 'text' },
}

/** Settings field meta with sliders and presets */
export const SETTINGS_FIELD_META: Record<string, FriendlyField> = {
  // Agent settings
  'agents.defaults.temperature': {
    key: 'agents.defaults.temperature',
    label: '回复风格',
    kind: 'select',
    options: [
      { label: '严谨', value: 0.2 },
      { label: '均衡', value: 0.7 },
      { label: '创意', value: 1.2 },
    ],
    hint: '温度越低回答越确定，越高越发散',
  },
  'agents.defaults.max_tokens': {
    key: 'agents.defaults.max_tokens',
    label: '回复长度上限',
    kind: 'select',
    options: [
      { label: '短 (2048)', value: 2048 },
      { label: '中 (4096)', value: 4096 },
      { label: '长 (8192)', value: 8192 },
      { label: '超长 (16384)', value: 16384 },
    ],
  },
  'agents.defaults.restrict_to_workspace': { key: 'agents.defaults.restrict_to_workspace', label: '限制在工作空间内操作', kind: 'toggle' },

  // Tools
  'tools.web.brave.enabled': { key: 'tools.web.brave.enabled', label: 'Brave 搜索', kind: 'toggle' },
  'tools.web.duckduckgo.enabled': { key: 'tools.web.duckduckgo.enabled', label: 'DuckDuckGo 搜索', kind: 'toggle' },
  'tools.cron.exec_timeout_minutes': {
    key: 'tools.cron.exec_timeout_minutes',
    label: 'Cron 执行超时',
    kind: 'slider',
    min: 5,
    max: 120,
    step: 5,
    unit: '分钟',
  },

  // Logging
  'logging.general.enabled': { key: 'logging.general.enabled', label: '通用日志', kind: 'toggle' },
  'logging.general.enable_console': { key: 'logging.general.enable_console', label: '控制台输出', kind: 'toggle' },
  'logging.general.level': {
    key: 'logging.general.level',
    label: '日志级别',
    kind: 'select',
    options: [
      { label: 'DEBUG', value: 'debug' },
      { label: 'INFO', value: 'info' },
      { label: 'WARN', value: 'warn' },
      { label: 'ERROR', value: 'error' },
    ],
  },
  'logging.llm.enabled': { key: 'logging.llm.enabled', label: 'LLM 通信日志', kind: 'toggle' },

  // Services
  'heartbeat.enabled': { key: 'heartbeat.enabled', label: 'Heartbeat', kind: 'toggle' },
  'devices.monitor_usb': { key: 'devices.monitor_usb', label: 'USB 监控', kind: 'toggle' },
  'security.enabled': { key: 'security.enabled', label: 'Security', kind: 'toggle' },
  'forge.enabled': { key: 'forge.enabled', label: 'Forge', kind: 'toggle' },
  'mcp.enabled': { key: 'mcp.enabled', label: 'MCP', kind: 'toggle' },
}

export function humanizeKey(key: string): string {
  return key
    .replace(/_/g, ' ')
    .replace(/\b\w/g, (c) => c.toUpperCase())
}

export function fieldMetaFor(
  key: string,
  value: unknown,
  table: Record<string, FriendlyField>,
): FriendlyField {
  if (table[key]) return table[key]
  if (typeof value === 'boolean') return { key, label: humanizeKey(key), kind: 'toggle' }
  if (typeof value === 'number') return { key, label: humanizeKey(key), kind: 'number' }
  const lower = key.toLowerCase()
  if (lower.includes('token') || lower.includes('secret') || lower.includes('password') || lower.includes('key')) {
    return { key, label: humanizeKey(key), kind: 'password' }
  }
  return { key, label: humanizeKey(key), kind: 'text' }
}

/** Flatten only primitive top-level fields for simple forms */
export function primitiveEntries(obj: Record<string, unknown>): [string, unknown][] {
  return Object.entries(obj || {}).filter(([, v]) => v === null || ['string', 'number', 'boolean'].includes(typeof v))
}
