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
}

/** 工具名 → 展示标签。未收录（含 mcp_*）返回原名。 */
export function toolLabel(tool: string): string {
  return TOOL_LABELS[tool] ?? tool
}
