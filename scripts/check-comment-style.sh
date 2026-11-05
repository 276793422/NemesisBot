#!/usr/bin/env bash
# 注释风格防复发闸（2026-11-06 注释治理战役交付）
# 扫描全仓 Rust 注释行（行首 //）的四类坐标文书指标，超线即红：
#   coord  阶段/批次坐标 token（P5/B12/L3/U7/G2/D9/E5/F3/K1/T4/J2/M4/WS9/S2-3/Phase N）
#   bug    BUG-YYYYMMDD-NN 引用
#   narr   实录/拍板 叙事标签
#   truth  真相源/唯一源/单一源/唯一真相/单一真相 表述
# 基线（2026-11-06 战役终局）：coord<=1500 bug<=100 narr<=50 truth<=700
# 用法：bash scripts/check-comment-style.sh   （退出码 0=达标 / 1=超线）
set -u
cd "$(dirname "$0")/.."

python - "$@" <<'PYEOF'
import io, re, glob, sys

PATTERNS = {
    'coord':  re.compile(r'(?:^|[\s（(【\[/])(?:P\d|B\d|L\d|U\d|G\d|D\d|E\d|F\d|K\d|T\d|J\d|M\d|WS\d|S\d-\d|Phase\s*\d|阶段\s*\d)(?=[\s）)：:，,。；;、\d]|$)'),
    'bug':    re.compile(r'\bBUG-\d{8}-\d+'),
    'narr':   re.compile(r'实录|拍板'),
    'truth':  re.compile(r'真相源|唯一源|单一源|唯一真相|单一真相'),
}
LIMITS = {'coord': 1500, 'bug': 100, 'narr': 50, 'truth': 700}
COMMENT = re.compile(r'^\s*//')
tot = {k: 0 for k in PATTERNS}
files = 0
for pat in ['crates/**/*.rs', 'nemesisbot/src/**/*.rs', 'test-tools/**/*.rs', 'plugins/**/*.rs']:
    for p in glob.glob(pat, recursive=True):
        if 'target' in p.replace('\\', '/').split('/'):
            continue
        try:
            src = io.open(p, encoding='utf-8', errors='replace').read()
        except OSError:
            continue
        files += 1
        for line in src.splitlines():
            if not COMMENT.match(line):
                continue
            for k, rex in PATTERNS.items():
                tot[k] += len(rex.findall(line))

bad = False
for k in ('coord', 'bug', 'narr', 'truth'):
    mark = 'OK ' if tot[k] <= LIMITS[k] else 'FAIL'
    if tot[k] > LIMITS[k]:
        bad = True
    print(f'[{mark}] {k:5s} {tot[k]:5d} / limit {LIMITS[k]}  ({files} rs files scanned)')
sys.exit(1 if bad else 0)
PYEOF
