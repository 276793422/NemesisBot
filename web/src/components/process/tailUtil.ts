/**
 * 后台进程输出 tail 工具（2026-11-06 P1 后台进程实时输出 UI）。
 *
 * 纯函数无副作用——ProcessOutputTail.vue 的取数循环与 vitest spec 共用。
 */

/**
 * 剥 ANSI 转义序列（CSI + OSC 简表）：ping/构建工具等常带颜色码，tail
 * 预览区按纯文本渲染，转义序列原样显示会碎屏。
 * - CSI：`\x1b[...<final byte @-~>`（含清屏/光标移动/SGR 颜色）。
 * - OSC：`\x1b]...(\x07 | \x1b\\)`（终端标题等）。
 * 不追求完备解析器——tail 观感干净即可，残留个别私有序列诚实可见。
 */
export function stripAnsi(s: string): string {
  return s
    // eslint-disable-next-line no-control-regex
    .replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, '')
    // eslint-disable-next-line no-control-regex
    .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, '')
}

/**
 * tail 起点坐标（produced 字节空间单调坐标，registry output() 同源语义）：
 * 优先从丢弃线开始（dropped 之前的字节已物理不在，请求必被 clamp），
 * 否则取「总量 - 窗口」的尾块起点（首次展开只见最近内容，不回灌全量）。
 * 空缓冲（total 0）返回 0。
 */
export function initialTailOffset(totalBytes: number, droppedBytes: number, window: number): number {
  if (droppedBytes > 0) return droppedBytes
  if (totalBytes <= window) return 0
  return totalBytes - window
}
