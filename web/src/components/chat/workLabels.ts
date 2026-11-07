/**
 * Work 时间线工具标签映射（2026-10-30 ChatPanel 长任务时间线改造）。
 *
 * 工具名 → 中文短标签（单行条展示）。映射缺失时 fallback 原名——
 * `mcp_` 前缀与未收录工具直接显示原名（诚实，不猜）。
 */

const TOOL_LABELS: Record<string, string> = {
  exec: '执行命令',
  write_file: '写入文件',
  edit_file: '编辑文件',
  multiedit: '编辑文件',
  read_file: '读取文件',
  grep: '搜索代码',
  git: 'Git 操作',
  web_search: '网页搜索',
  web_fetch: '抓取网页',
  spawn: '生成子代理',
  create_dir: '创建目录',
  code_run: '运行脚本',
  cluster_rpc: '集群调用',
  board_issue: '看板操作',
  generate_image: '生成图片',
  // 后台进程（2026-11-06 后台进程实时输出）。
  background_start: '启动后台进程',
  background_output: '读后台输出',
  background_kill: '停止后台进程',
  background_steer: '注入后台输入',
}

/** 工具名 → 展示标签。未收录（含 mcp_*）返回原名。 */
export function toolLabel(tool: string): string {
  return TOOL_LABELS[tool] ?? tool
}

/**
 * spawn 行任务文本提取（2026-11-06 B 档历史钻取）。
 *
 * args_preview 是截断到 200 字符的调用参数 JSON（tool_event_hook
 * ARGS_PREVIEW_CAP）：完整可 parse 时取 `task` 字段；截断则正则抠出
 * `"task":"…"` 前缀，捕获组回包 JSON.parse 还原转义（\n/\" 等）。
 * 提取失败返回空串（诚实降级为参数原文展示）。
 */
export function spawnTaskFromArgs(preview: string): string {
  if (!preview) return ''
  try {
    const v = JSON.parse(preview)
    if (v && typeof v === 'object' && typeof (v as Record<string, unknown>).task === 'string') {
      return (v as Record<string, string>).task
    }
  } catch { /* 截断 JSON——走正则前缀提取 */ }
  const m = /"task"\s*:\s*"((?:[^"\\]|\\.)*)/.exec(preview)
  if (!m) return ''
  try {
    return JSON.parse(`"${m[1]}"`)
  } catch {
    return m[1]
  }
}

/**
 * background_start 行的 job_id 提取（2026-11-06 后台进程实时输出）。
 *
 * result_preview 是截断的返回值 JSON（同 ARGS_PREVIEW_CAP 形态）：
 * 完整可 parse 时取 `job_id` 数字字段；截断则正则抠 `"job_id":<digits>`。
 * 提取失败返回 null（行渲染诚实降级为无内嵌 tail）。
 */
export function jobIdFromPreview(preview: string | null | undefined): number | null {
  if (!preview) return null
  try {
    const v = JSON.parse(preview)
    if (v && typeof v === 'object') {
      const id = (v as Record<string, unknown>).job_id
      if (typeof id === 'number' && Number.isFinite(id)) return id
    }
  } catch { /* 截断 JSON——走正则提取 */ }
  const m = /"job_id"\s*:\s*(\d+)/.exec(preview)
  return m ? Number(m[1]) : null
}
