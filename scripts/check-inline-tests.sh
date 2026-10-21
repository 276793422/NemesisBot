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
#   - *_acceptance.rs（coding-bench 隐藏验收测试载荷——数据性质，注入被测夹具
#     由验收流程编译，不进本仓任何编译单元；2026-10-15）
#
# 性能注记（2026-10-22 单进程重写）：旧版逐文件孵化 perl+sed+grep+子 shell
# （每文件 4 个进程 × 数千文件，Windows 进程创建开销下 10 分钟级，多次卡死
# 门禁流水）；现把整个扫描连同 [[test]] 白名单解析全部搬进单个 perl 进程
# （Cargo.toml 解析按 package 根缓存），数千文件秒级完成。语义与旧版逐条
# 对齐（四形态正则、白名单规则、输出格式、退出码均不变）。
#
# 用法：bash scripts/check-inline-tests.sh
#   （测试可用环境变量 CHECK_INLINE_ROOTS 覆盖扫描根目录；CI 不传 = 默认四目录）
# 退出码：0 = 无违规；1 = 有违规（输出逐条违规供人工确认）
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [ -n "${CHECK_INLINE_ROOTS:-}" ]; then
  read -r -a ROOTS <<< "$CHECK_INLINE_ROOTS"
else
  ROOTS=("$ROOT/crates" "$ROOT/nemesisbot" "$ROOT/test-tools" "$ROOT/plugins")
fi

# 扫描范围：crates、nemesisbot、test-tools、plugins 的全部 .rs（含 build.rs）
CANDIDATES="$(find "${ROOTS[@]}" \
  -name '*.rs' -not -path '*/target/*' -not -path '*/node_modules/*' 2>/dev/null | sort)"

RESULT="$(printf '%s\n' "$CANDIDATES" | perl -e '
use strict; use warnings;

my @files = map { chomp; $_ } <STDIN>;
my %cache;    # pkg_root => [declared [[test]] relpaths]
my ($total, $found) = (0, 0);

# 文件所属 package 根（最近的有 Cargo.toml 的祖先目录）
sub pkg_root_of {
    my ($f) = @_;
    (my $d = $f) =~ s{[\\/][^\\/]*$}{};
    while ($d =~ m{/} && $d ne "/" && $d ne ".") {
        return $d if -f "$d/Cargo.toml";
        $d =~ s{[\\/][^\\/]*$}{};
    }
    return undef;
}

# 某 package Cargo.toml 中 [[test]] 声明的 path（相对 package 根，posix 化）。
# 块范围语义与旧 sed 版一致：[[test]] 头开启，遇到下一个非 [[ 开头的节头
# 关闭；连续多个 [[test]] 块由同一开关覆盖。
sub declared_test_paths {
    my ($root) = @_;
    return @{ $cache{$root} } if exists $cache{$root};
    my @paths;
    if (open my $fh, "<", "$root/Cargo.toml") {
        my $in_block = 0;
        while (my $ln = <$fh>) {
            if ($ln =~ /^\s*\[\[test\]\]/) { $in_block = 1; next; }
            if ($ln =~ /^\s*\[[^[]/)       { $in_block = 0; next; }
            if ($in_block && $ln =~ /^\s*path\s*=\s*"([^"]*)"/) {
                (my $p = $1) =~ tr{\\}{/};
                push @paths, $p;
            }
        }
        close $fh;
    }
    $cache{$root} = \@paths;
    return @paths;
}

sub is_declared_test_target {
    my ($f) = @_;
    my $root = pkg_root_of($f) or return 0;
    (my $rel = $f) =~ s{^\Q$root\E[\\/]}{};
    $rel =~ tr{\\}{/};
    for my $p (declared_test_paths($root)) { return 1 if $p eq $rel; }
    return 0;
}

for my $f (@files) {
    next unless length $f;
    $total++;
    my $base = $f; $base =~ s{.*[\\/]}{};
    next if $f =~ m{/tests/};                # 合法：tests/ 目录
    next if $base =~ /tests\.rs$/;           # 合法：tests.rs / *_tests.rs
    next if $base =~ /_acceptance\.rs$/;     # 合法：验收测试载荷
    next if is_declared_test_target($f);     # 合法：[[test]] path 声明
    open my $fh, "<", $f or next;
    my @lines = <$fh>;
    close $fh;
    my $viol = 0;
    for (my $i = 0; $i < @lines; $i++) {
        my $ln = $lines[$i];
        # 形态 B/C：裸测试属性（#[test] / #[tokio::test] / 含参形态也计
        # ——2026-09-23 加固：带参 tokio::test 此前漏网）
        if ($ln =~ /^\s*#\[\s*(::)?(tokio::)?test(\([^)]*\))?\s*\]/) {
            print "VIOLATION TESTFN $f:", $i + 1, ": ", $ln; $viol = 1;
        }
        # 形态 A/D：cfg(test) 系属性后面紧跟「带 body 的 mod」
        if ($ln =~ /^\s*#\[\s*cfg\(\s*(all\()?\s*test/) {
            my $j = $i + 1;
            # 跳过空行、其他属性行、纯注释行
            while ($j < @lines && $lines[$j] =~ /^\s*(#\[.*\]|\/\/.*)?\s*$/) { $j++ }
            if ($j < @lines && $lines[$j] =~ /^\s*(pub\s+)?mod\s+\w+\s*\{/) {
                print "VIOLATION MODBODY $f:", $j + 1, ": ", $lines[$j]; $viol = 1;
            }
            # mod 名与 { 换行分开的形态
            elsif ($j < @lines && $lines[$j] =~ /^\s*(pub\s+)?mod\s+\w+\s*$/) {
                my $k = $j + 1;
                while ($k < @lines && $lines[$k] =~ /^\s*$/) { $k++ }
                if ($k < @lines && $lines[$k] =~ /^\s*\{/) {
                    print "VIOLATION MODBODY $f:", $j + 1, ": ", $lines[$j]; $viol = 1;
                }
            }
        }
    }
    $found++ if $viol;
}

print "----\n";
print "scanned: $total files, violations: $found\n";
exit($found > 0 ? 1 : 0);
')"
RC=$?
printf '%s\n' "$RESULT"

if [ "$RC" -ne 0 ]; then
  echo "RESULT: FAIL（存在生产文件内联测试，请迁移到 <stem>/tests.rs）"
  exit 1
fi
echo "RESULT: PASS"
exit 0
