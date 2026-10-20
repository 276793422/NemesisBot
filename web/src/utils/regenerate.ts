/**
 * 重新生成规划（纯函数，2026-10-20 重新生成修复）。
 *
 * 「重新生成」的真实语义 = 回退到本轮提问行之前（`sessions.rewind_to_message`
 * `mode:"before"`：该轮与其后所有行移除、文件滚回提问前）+ 重发提问原文。
 * 本模块只做**规划**（定位轮首 + 自动重发守卫裁决），副作用（rewind/发送）
 * 在 ChatPanel。
 */

export interface RegenRowLike {
  /** chat_log jsonl 行号（历史批次连续推导 + live 递增；渲染行无此字段）。 */
  rowIndex?: number
  role: string
  content: string
  imageCount?: number
}

export interface RegenPlan {
  /** 本轮起始 user 行的 jsonl 下标（rewind mode:"before" 的目标）。 */
  turnStart: number
  /** 提问原文（自动重发内容）。 */
  content: string
  /** 非空 = 不自动重发的原因（只回退，提问原文回填输入框手动发）。 */
  noResendReason: string | null
}

/**
 * 从点击行定位本轮起始 user 行并裁决自动重发。
 *
 * 轮首定位（结构性，按 rowIndex）：从点击行向上越过连续 user 行（steer 插
 * 话也是 user-role 行，属本轮一并回退），停在上一条 assistant 行——其后即
 * 轮首。渲染行（error/system，无 rowIndex）天然跳过。翻页窗口边界（rowIndex
 * 跳号）落在窗口内最早的 user 行——诚实降级，不猜窗口外的行。
 *
 * 守卫（诚实降级：只回退不重发）：
 * - `[steer] ` 开头 = 轮首即插话行（异常形态），原文不宜原样重发；
 * - imageCount > 0 = 历史响应只有图片计数、路径拿不到，重发会丢图；
 * - `/` 开头 = 斜杠命令，重放有副作用；
 * - 空内容无可重发。
 */
export function planRegenerate(
  messages: RegenRowLike[],
  clickedRow: number,
  clickedRole: string,
): RegenPlan | null {
  const rowMap = new Map<number, RegenRowLike>()
  for (const m of messages) {
    if (m.rowIndex !== undefined) rowMap.set(m.rowIndex, m)
  }
  // 向上走：assistant 行从其前一行起；user 行从自身起。连续 user 行属本轮。
  let s = clickedRole === 'assistant' ? clickedRow - 1 : clickedRow
  while (s > 0 && rowMap.get(s)?.role === 'user') s--
  const turnStart = rowMap.get(s)?.role === 'user' ? 0 : s + 1
  const q = rowMap.get(turnStart)
  if (!q) return null
  const content = (q.content || '').trim()
  let noResendReason: string | null = null
  if (!content) noResendReason = '提问内容为空'
  else if (content.startsWith('[steer] ')) noResendReason = '插话轮不自动重发'
  else if ((q.imageCount ?? 0) > 0) noResendReason = '消息含图片（重发会丢附件）'
  else if (content.startsWith('/')) noResendReason = '斜杠命令不自动重发（防副作用重放）'
  return { turnStart, content, noResendReason }
}
