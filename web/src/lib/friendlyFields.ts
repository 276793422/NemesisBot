/**
 * Map raw config keys → human labels / control types for simple forms.
 */

export type FieldKind = 'toggle' | 'text' | 'password' | 'number' | 'readonly'

export interface FriendlyField {
  key: string
  label: string
  kind: FieldKind
  hint?: string
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

/** Security policy keys */
export const SECURITY_FIELD_META: Record<string, FriendlyField> = {
  enabled: { key: 'enabled', label: '启用安全策略', kind: 'toggle' },
  default_action: { key: 'default_action', label: '默认动作 (allow/deny)', kind: 'text', hint: '通常为 allow 或 deny' },
  approval_timeout_seconds: { key: 'approval_timeout_seconds', label: '审批超时（秒）', kind: 'number' },
  audit_log_file_enabled: { key: 'audit_log_file_enabled', label: '写入审计日志文件', kind: 'toggle' },
  audit_log_retention_days: { key: 'audit_log_retention_days', label: '审计保留天数', kind: 'number' },
  log_all_operations: { key: 'log_all_operations', label: '记录全部操作', kind: 'toggle' },
  log_denials_only: { key: 'log_denials_only', label: '仅记录拒绝', kind: 'toggle' },
  max_pending_requests: { key: 'max_pending_requests', label: '最大待审批数', kind: 'number' },
  synchronous_mode: { key: 'synchronous_mode', label: '同步模式', kind: 'toggle' },
  audit_log_path: { key: 'audit_log_path', label: '审计日志路径', kind: 'text' },
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
