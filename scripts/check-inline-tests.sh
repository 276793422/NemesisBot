#!/usr/bin/env bash
# check-inline-tests.sh —— 生产文件内联测试扫描（防回归检查）
#
# 纪律来源：2026-07-17 起「测试代码放独立文件」（<stem>/tests.rs + `#[cfg(test)] mod tests;` 声明）。
# 本脚本扫描四种违规形态：
#   A. #[cfg(test)] mod xxx { …带body… }      —— 内联测试模块
#   B. 裸 #[test] fn                           —— 生产文件里的测试函数
#   C. 裸 #[tokio::test] fn                    —— 同上（异步形态）
#   D. #[cfg(all(test,…))] mod xxx { …带body… } —— feature 门控内联测试模块
#
# 合法形态（不算违规）：
#   - `#[cfg(test)] mod xxx;`（分号结尾的声明，指向独立测试文件）
#   - 文件本身就是测试文件：tests.rs / *_tests.rs / 位于 tests/ 目录下
#   - Cargo.toml 里 [[test]] path = "..." 显式声明的测试 target 文件
#
# 用法：bash scripts/check-inline-tests.sh
# 退出码：0 = 无违规；1 = 有违规（输出逐条违规供人工确认）
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

is_test_file() {
  case "$1" in
    */tests/*) return 0 ;;
    *tests.rs|*_tests.rs) return 0 ;;
    *) return 1 ;;
  esac
}

# 找文件所属 package 根（最近的有 Cargo.toml 的祖先目录）
pkg_root_of() {
  local d; d="$(dirname "$1")"
  while [ "$d" != "/" ] && [ "$d" != "." ]; do
    if [ -f "$d/Cargo.toml" ]; then printf '%s' "$d"; return 0; fi
    d="$(dirname "$d")"
  done
  return 1
}

# 列出某 package Cargo.toml 中 [[test]] 声明的 path（相对 package 根，posix 化）
declared_test_paths() {
  awk '
    /^\s*\[\[test\]\]/ { in_test = 1; next }
    /^\s*\[/ { in_test = 0; next }
    in_test && match($0, /^\s*path\s*=\s*"([^"]+)"/, m) { print m[1] }
  ' "$1/Cargo.toml" 2>/dev/null | tr '\\' '/'
}

# 缓存：pkg_root -> 换行分隔的绝对路径集
PKG_TESTS_CACHE_KEYS=""
PKG_TESTS_CACHE_VALS=""

is_declared_test_target() {
  local f="$1" root; root="$(pkg_root_of "$f")" || return 1
  local i key="" idx=-1
  # 查缓存
  i=0
  while IFS=$'\t' read -r k; do
    [ -z "$k" ] && continue
    i=$((i+1))
    if [ "$k" = "$root" ]; then idx=$((i-1)); break; fi
  done <<< "$PKG_TESTS_CACHE_KEYS"
  local paths=""
  if [ "$idx" -ge 0 ]; then
    paths="$(printf '%s\n' "$PKG_TESTS_CACHE_VALS" | sed -n "$((idx+1))p")"
  else
    paths="$(declared_test_paths "$root")"
    PKG_TESTS_CACHE_KEYS="${PKG_TESTS_CACHE_KEYS}${root}"$'\t'
    PKG_TESTS_CACHE_VALS="${PKG_TESTS_CACHE_VALS}${paths%$'\n'}"$'\n'
  fi
  [ -z "$paths" ] && return 1
  local rel="${f#"$root"/}"
  printf '%s\n' "$paths" | grep -qx "$rel"
}

scan_one() {
  local f="$1"
  is_test_file "$f" && return 0
  is_declared_test_target "$f" && return 0
  perl -e '
    my $f = $ARGV[0];
    open my $fh, "<", $f or die "cannot open $f";
    my @lines = <$fh>;
    for (my $i = 0; $i < @lines; $i++) {
      my $ln = $lines[$i];
      # 形态 B/C：裸测试属性（#[test] / #[tokio::test] / #[::tokio::test] 等）
      if ($ln =~ /^\s*#\[\s*(::)?(tokio::)?test\s*\]/) {
        print "VIOLATION TESTFN $f:", $i+1, ": ", $ln;
      }
      # 形态 A/D：cfg(test) 系属性后面紧跟「带 body 的 mod」
      if ($ln =~ /^\s*#\[\s*cfg\(\s*(all\()?\s*test/) {
        my $j = $i + 1;
        # 跳过空行、其他属性行、纯注释行
        while ($j < @lines && $lines[$j] =~ /^\s*(#\[.*\]|\/\/.*)?\s*$/) { $j++ }
        if ($j < @lines && $lines[$j] =~ /^\s*(pub\s+)?mod\s+\w+\s*\{/) {
          print "VIOLATION MODBODY $f:", $j+1, ": ", $lines[$j];
        }
        # mod 名与 { 换行分开的形态
        elsif ($j < @lines && $lines[$j] =~ /^\s*(pub\s+)?mod\s+\w+\s*$/) {
          my $k = $j + 1;
          while ($k < @lines && $lines[$k] =~ /^\s*$/) { $k++ }
          if ($k < @lines && $lines[$k] =~ /^\s*\{/) {
            print "VIOLATION MODBODY $f:", $j+1, ": ", $lines[$j];
          }
        }
      }
    }
  ' "$f"
}

# 扫描范围：crates、nemesisbot、test-tools、plugins 的全部 .rs（含 build.rs）
CANDIDATES="$(find "$ROOT/crates" "$ROOT/nemesisbot" "$ROOT/test-tools" "$ROOT/plugins" \
  -name '*.rs' -not -path '*/target/*' -not -path '*/node_modules/*' 2>/dev/null | sort)"

TOTAL=0
FOUND=0
while IFS= read -r f; do
  [ -z "$f" ] && continue
  TOTAL=$((TOTAL+1))
  out="$(scan_one "$f")"
  if [ -n "$out" ]; then
    FOUND=$((FOUND+1))
    printf '%s\n' "$out"
  fi
done <<< "$CANDIDATES"

echo "----"
echo "scanned: $TOTAL files, violations: $FOUND"
if [ "$FOUND" -gt 0 ]; then
  echo "RESULT: FAIL（存在生产文件内联测试，请迁移到 <stem>/tests.rs）"
  exit 1
fi
echo "RESULT: PASS"
exit 0
