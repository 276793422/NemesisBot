/**
 * UUID v4 生成（非安全上下文兜底，2026-09-20）。
 *
 * `crypto.randomUUID()` 是安全上下文（HTTPS / localhost）专属 API——经
 * `http://<公网IP>` 访问 dashboard 时该函数不存在，所有 WSAPI 请求在生成
 * reqId 时直接抛 `crypto.randomUUID is not a function`，整站瘫痪。
 * 三级兜底：原生 randomUUID → getRandomValues 拼 v4（非安全上下文仍可用）
 * → Math.random（无 crypto 的极端环境；非加密安全，仅作标识符）。
 */
export function uuidv4(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID()
  }
  if (typeof crypto !== 'undefined' && typeof crypto.getRandomValues === 'function') {
    const b = crypto.getRandomValues(new Uint8Array(16))
    b[6] = (b[6] & 0x0f) | 0x40 // version 4
    b[8] = (b[8] & 0x3f) | 0x80 // variant 10xx
    const h = Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('')
    return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`
  }
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0
    const v = c === 'x' ? r : (r & 0x3) | 0x8
    return v.toString(16)
  })
}
