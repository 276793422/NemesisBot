/**
 * Date → `datetime-local` 输入值（本地墙钟，`YYYY-MM-DDTHH:mm`）。
 *
 * 不要用 `d.toISOString().slice(0, 16)` 拼这个值——toISOString 产出的是
 * UTC 墙钟，而 datetime-local 输入与 `new Date(str)`（无时区后缀）都按
 * **本地**解析，混用会把时间整体平移一个时区差（UTC+8 用户看到的"现在"
 * 实际是 8 小时前，用作默认窗口会静默丢掉最近的数据）。
 */
export function toLocalInputValue(d: Date): string {
  const p = (n: number): string => String(n).padStart(2, '0')
  return (
    `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}` +
    `T${p(d.getHours())}:${p(d.getMinutes())}`
  )
}
