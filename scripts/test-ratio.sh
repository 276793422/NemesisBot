#!/usr/bin/env bash
# T7b（扩展计划 D6）：生产/测试代码行数比统计。
#
# 口径（依赖 2026-07-17 起的内联测试纪律：生产文件零内联测试体，测试代码
# 全在独立文件——scripts/check-inline-tests.sh 是该前提的门禁）：
#   - 生产行 = crates/ + plugins/ + nemesisbot/src 下 .rs，且文件名不是
#     tests.rs / *_tests.rs、不在 tests/ 目录内；
#   - 测试行 = 上述根下命中测试文件形态的 .rs 全部行。
#   - test-tools/ 与 Skills/ 不计入任何一侧（它们是测试基础设施/流程，
#     不是被测产物的镜像）。
#
# 用法：
#   scripts/test-ratio.sh                 # 打印汇总 + 写 target/test-ratio.txt
#   scripts/test-ratio.sh --quiet         # 只写文件不打印明细
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/target/test-ratio.txt"
QUIET=0
[ "${1:-}" = "--quiet" ] && QUIET=1

prod=0
test_lines=0

# per-crate 计数（关联数组：crate → "prod test"）
declare -A crate_prod crate_test

for root_dir in crates plugins nemesisbot/src; do
    while IFS= read -r f; do
        lines=$(wc -l < "$f")
        base="$(basename "$f")"
        case "$base" in
            tests.rs|*_tests.rs) kind=test ;;
            *)
                case "$(dirname "$f")" in
                    */tests|*/tests/*|tests|tests/*) kind=test ;;
                    *) kind=prod ;;
                esac
                ;;
        esac
        # crate 归属：crates/<name>/... 取段名；plugins 同理；nemesisbot 取主程序
        # （$f 是绝对路径，模式须带 */ 前缀）
        case "$f" in
            */crates/*) crate=$(echo "$f" | sed 's|.*/crates/||' | cut -d/ -f1) ;;
            */plugins/*) crate=$(echo "$f" | sed 's|.*/plugins/||' | cut -d/ -f1) ;;
            */nemesisbot/src/*) crate=nemesisbot ;;
            *) crate=other ;;
        esac
        if [ "$kind" = prod ]; then
            prod=$((prod + lines))
            crate_prod[$crate]=$(( ${crate_prod[$crate]:-0} + lines ))
        else
            test_lines=$((test_lines + lines))
            crate_test[$crate]=$(( ${crate_test[$crate]:-0} + lines ))
        fi
    done < <(find "$ROOT/$root_dir" -name '*.rs' -type f 2>/dev/null)
done

total=$((prod + test_lines))
if [ "$total" -eq 0 ]; then
    echo "no .rs files found" >&2; exit 1
fi
ratio=$(awk "BEGIN{printf \"%.1f\", $test_lines * 100 / $prod}")

{
    echo "NemesisBot 生产/测试行数比（$(date +%F)）"
    echo "口径：生产文件 vs 独立测试文件（内联测试纪律保证切分干净；test-tools/ 不计）"
    echo ""
    echo "生产行: $prod"
    echo "测试行: $test_lines"
    echo "测试/生产比: ${ratio}%"
    echo ""
    echo "per-crate（生产行 / 测试行 / 比）:"
    for c in "${!crate_prod[@]}"; do
        p=${crate_prod[$c]}
        t=${crate_test[$c]:-0}
        r=$(awk "BEGIN{if ($p>0) printf \"%.0f\", $t*100/$p; else printf \"n/a\"}")
        printf "  %-28s %8d %8d  %s%%\n" "$c" "$p" "$t" "$r"
    done | sort
} > "$OUT"

if [ "$QUIET" -eq 0 ]; then
    cat "$OUT"
fi
