#!/usr/bin/env bash
# 注释风格防复发闸（2026-11-06 注释治理战役交付；2026-11-28 用户裁决收缩为
# coord 单指标——原四闸中 bug/narr/truth 与现行 BUG 溯源注记纪律矛盾：缺陷
# 修复注释引用 BUG 编号是功能性溯源不是坐标文书，「实录/拍板」「真相源」
# 表述同样是现行注记纪律的组成部分，按计数闸管会永久红；三闸摘除，仅保留
# coord（阶段/批次坐标 token——战役唯一本意要压制的文书形态）。
# 扫描全仓 Rust 注释行（行首 //）的坐标文书指标，超线即红：
#   coord  阶段/批次坐标 token（P5/B12/L3/U7/G2/D9/E5/F3/K1/T4/J2/M4/WS9/S2-3/Phase N）
# 基线校准（2026-11-28 挂载 CI 时点）：存量 coord=2052（2872 rs 文件），
# 上限设 2400——高于存量留正常增长余量（约 +17%），低于失控复发形态；
# 后续可随治理进度 ratchet 收紧。原四闸基线（coord<=1500 bug<=400
# narr<=50 truth<=700）作废。
# 用法：bash scripts/check-comment-style.sh   （退出码 0=达标 / 1=超线）
set -u
cd "$(dirname "$0")/.."

PY=python3
command -v python3 >/dev/null 2>&1 || PY=python
"$PY" - "$@" <<'PYEOF'
import io, re, glob, sys

PATTERNS = {
    'coord':  re.compile(r'(?:^|[\s（(【\[/])(?:P\d|B\d|L\d|U\d|G\d|D\d|E\d|F\d|K\d|T\d|J\d|M\d|WS\d|S\d-\d|Phase\s*\d|阶段\s*\d)(?=[\s）)：:，,。；;、\d]|$)'),
}
LIMITS = {'coord': 2400}
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
for k in ('coord',):
    mark = 'OK ' if tot[k] <= LIMITS[k] else 'FAIL'
    if tot[k] > LIMITS[k]:
        bad = True
    print(f'[{mark}] {k:5s} {tot[k]:5d} / limit {LIMITS[k]}  ({files} rs files scanned)')
sys.exit(1 if bad else 0)
PYEOF
