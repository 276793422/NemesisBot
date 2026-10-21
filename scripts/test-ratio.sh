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
# 性能注记（2026-10-22 单进程重写，与 check-inline-tests.sh 同批根治）：
# 旧版逐文件孵化 wc+dirname+basename+sed+cut（每文件 5 个外部进程 × 2000+
# 文件 ≈ 万次进程创建，Windows 下 10 分钟级卡死）；现把分类/计数/归组全部
# 搬进单个 perl 进程（行数语义与 wc -l 一致：数换行符），2000 文件秒级。
# 分类规则逐条对齐旧版（tests.rs 精确名或 *_tests.rs 后缀；tests/ 目录判定
# 含 dirname 四形态；crate 归属取 /crates/、/plugins/ 后首段）。
#
# 用法：
#   scripts/test-ratio.sh                 # 打印汇总 + 写 target/test-ratio.txt
#   scripts/test-ratio.sh --quiet         # 只写文件不打印明细
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/target/test-ratio.txt"
QUIET=0
[ "${1:-}" = "--quiet" ] && QUIET=1

REPORT="$(find "$ROOT/crates" "$ROOT/plugins" "$ROOT/nemesisbot/src" -name '*.rs' -type f 2>/dev/null | sort | perl -e '
use strict; use warnings;

my ($prod, $test_lines) = (0, 0);
my (%cp, %ct);    # crate => lines

for my $f (map { chomp; $_ } <STDIN>) {
    next unless length $f;
    # 行数语义与 wc -l 一致：数换行符（末行无换行不计）
    open my $fh, "<", $f or next;
    my $content = do { local $/; <$fh> };
    close $fh;
    my $lines = ($content =~ tr/\n//);

    my $base = $f; $base =~ s{.*[\\/]}{};
    (my $dir = $f) =~ s{[\\/][^\\/]*$}{};
    my $kind;
    if ($base eq "tests.rs" || $base =~ /_tests\.rs$/) {
        $kind = "test";
    } elsif ($dir eq "tests" || $dir =~ m{^tests/} || $dir =~ m{/tests$} || $dir =~ m{/tests/}) {
        $kind = "test";
    } else {
        $kind = "prod";
    }

    # crate 归属：crates/<name>/... 取段名；plugins 同理；nemesisbot 取主程序
    my $crate = "other";
    if ($f =~ m{/crates/([^/]+)})        { $crate = $1; }
    elsif ($f =~ m{/plugins/([^/]+)})    { $crate = $1; }
    elsif ($f =~ m{/nemesisbot/src/})    { $crate = "nemesisbot"; }

    if ($kind eq "prod") { $prod += $lines; $cp{$crate} += $lines; }
    else                 { $test_lines += $lines; $ct{$crate} += $lines; }
}

my $total = $prod + $test_lines;
if ($total == 0) { print STDERR "no .rs files found\n"; exit 1; }

print "生产行: $prod\n";
print "测试行: $test_lines\n";
my $ratio = $prod > 0 ? sprintf("%.1f", $test_lines * 100 / $prod) : "n/a";
print "测试/生产比: ${ratio}%\n";
print "\n";
print "per-crate（生产行 / 测试行 / 比）:\n";
for my $c (sort keys %cp) {
    my $p = $cp{$c};
    my $t = $ct{$c} // 0;
    my $r = $p > 0 ? sprintf("%.0f", $t * 100 / $p) : "n/a";
    printf "  %-28s %8d %8d  %s%%\n", $c, $p, $t, $r;
}
')"
RC=$?

if [ "$RC" -ne 0 ]; then
  exit 1
fi

{
    echo "NemesisBot 生产/测试行数比（$(date +%F)）"
    echo "口径：生产文件 vs 独立测试文件（内联测试纪律保证切分干净；test-tools/ 不计）"
    echo ""
    echo "$REPORT"
} > "$OUT"

if [ "$QUIET" -eq 0 ]; then
    cat "$OUT"
fi
