#!/usr/bin/env python3
"""修复 migrate_inbound_literals.py 的 indent bug。

损坏特征：builder 续行带上了行中代码前缀（如 `let inbound = ` 或
`bus.publish_inbound(`）+ 4 空格 + `.with_xxx(...)`。
修复：剥掉前缀，只保留行首空白。

合法 builder 行（纯空白缩进 + `.with_`）不受影响（模式要求非空白前缀）。
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# 损坏行：行首空白 + 非空白代码 + ≥4 空格 + .with_五方法
BROKEN = re.compile(
    r"^(\s+)\S.*?\s{4}(\.with_(?:session_key|media|correlation_id|metadata_map|voice_playback)\()"
)


def repair(path: Path) -> int:
    src = path.read_text(encoding="utf-8")
    lines = src.split("\n")
    fixed = 0
    out = []
    for line in lines:
        m = BROKEN.match(line)
        if m:
            out.append(m.group(1) + m.group(2) + line[m.end():])
            fixed += 1
        else:
            out.append(line)
    if fixed:
        path.write_text("\n".join(out), encoding="utf-8", newline="")
    return fixed


def main():
    targets = sys.argv[1:]
    total = 0
    for rel in targets:
        p = ROOT / rel
        if not p.exists():
            print(f"MISSING {rel}")
            continue
        n = repair(p)
        if n:
            print(f"{rel}: {n} broken builder lines repaired")
        total += n
    print(f"TOTAL repaired: {total}")


if __name__ == "__main__":
    main()
