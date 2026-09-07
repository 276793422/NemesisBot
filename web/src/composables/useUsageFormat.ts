/**
 * M5 (2026-09-05) — 会话用量格式化（tokens / cost）。
 *
 * 消费方：SessionSidebar（侧栏小字）与 ChatPanel（context/cost 常驻条）。
 * 后端字段来自 `request_logs` 按 session_key 的聚合（sessions.list /
 * logs.session_usage 回填）。零值/缺省返回空串（调用方据此不渲染）。
 */

/** 1200 → "1.2k tok"；1500000 → "1.5M tok"；缺省/0 → ""。 */
export function fmtTokens(n?: number): string {
  if (!n) return ''
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M tok`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k tok`
  return `${n} tok`
}

/** 1.5 → "$1.50"；0.5 → "$0.50"；0.0031 → "$0.0031"；缺省/0 → ""。 */
export function fmtCost(c?: number): string {
  if (!c) return ''
  // ≥ $0.01 用两位小数（0.5 的自然显示是 $0.50，不是剥尾零后的 $0.5）；
  // 更小的计量（per-token 价尾差）才展开到 4 位并去尾零。
  if (c >= 0.01) return `$${c.toFixed(2)}`
  const s = c.toFixed(4).replace(/0+$/, '')
  return `$${s.endsWith('.') ? `${s}0` : s}`
}

/** "1.2k tok · $0.0031"（两者皆缺省 → ""）。 */
export function fmtUsageLine(s: { tokens?: number; cost?: number }): string {
  return [fmtTokens(s.tokens), fmtCost(s.cost)].filter(Boolean).join(' · ')
}
