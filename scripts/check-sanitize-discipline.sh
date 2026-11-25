#!/usr/bin/env bash
# SAN-05 纪律守卫：路径成分派生禁止裸 `replace(':')` / `replace([':'…)`——
# 一律用仓内单一真相源 `nemesis_utils::sanitize::sanitize_path_segment`
# （白名单 + 点守卫 + 80 截断）。历史上 8 处裸 replace 造成 B 端复合键
# `{node}/{chat}` 嵌套目录（SAN-01）与多套分歧消毒器并存（`..` 曾穿透
# transfer/request_logger 黑名单，SAN-03/04）。本守卫防回归：新增命中即红。
#
# 豁免形态（非路径用途，按内容豁免而非行号，新增豁免在此登记并写明理由）：
#   - 纯注释行（// /// //!）——文档需点名历史形态说明根因
#   - `replace(':', "")` —— marker 分隔符剥离（loop_tools 异步任务标记，
#     非路径拼接）
#   - 行尾 `SAN-05-EXEMPT` 注释锚 —— 净化器本体类豁免（2026-11-25，
#     BUG-20261125-063）：zip_util::sanitize_windows_entry_name 是「内容
#     必须照收不能拒绝」位点（scanner 扫描提取）的路径净化器，与真相源
#     语义系**有意分歧**（黑名单制：只 `:`→`_`，保留非 ASCII/空格；真相源
#     白名单制会把非 ASCII 条目名整体映射成 `_`，破坏照扫契约）。与 D-5
#     边界的 episodic 多字符 blacklist 形态同类：取舍记录在函数 doc，
#     不适用白名单真相源。锚在命中行内 → 内容豁免，新增裸 replace 仍红。
#
# 覆盖范围边界（D-5，复核 2026-09-16）：本脚本只守「单字符/前缀冒号」两
# 形态（replace(':') / replace([':…)。多字符 blacklist 形态（如
# nemesis-memory episodic::session_file 的
# replace(['/','\\',':','*','?','"','<','>','|'])）**不在 grep 覆盖内**——
# 它与真相源白名单的映射有差异（保留空格/非 ASCII/`@`），迁移需带旧名
# fallback，取舍记录在该函数 doc 注释，不在此脚本层面强拦。
set -uo pipefail
cd "$(dirname "$0")/.."

hits=$(grep -rnE "replace\(':'|replace\(\[':" crates nemesisbot/src --include='*.rs' 2>/dev/null \
  | grep -v '/tests' \
  | grep -v '_tests\.rs' \
  | grep -vE ':[0-9]+:[[:space:]]*//' \
  | grep -vF "replace(':', \"\")" \
  | grep -v 'SAN-05-EXEMPT' \
  || true)

if [ -n "$hits" ]; then
  echo "ERROR: 生产代码出现裸 replace(':')（SAN-05 路径消毒纪律）："
  echo "$hits"
  echo ""
  echo "路径成分派生请改用 nemesis_utils::sanitize::sanitize_path_segment；"
  echo "确属非路径用途需豁免时，在 scripts/check-sanitize-discipline.sh 头部登记理由。"
  exit 1
fi
echo "SAN-OK: 生产代码无裸 replace(':') 路径派生"
