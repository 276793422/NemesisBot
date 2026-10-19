#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""runtime-manifest.json 生成器（A1 运行时完整性清单，2026-10-19）。

单一真相源：三平台构建脚本（build-windows.bat / build-linux.sh /
build-android.bat / customize.{bat,sh}）全部调用本脚本，在产物目录落
runtime-manifest.json —— 覆盖主二进制 + static/ 前端全量文件（sha256 +
size）。gateway 启动早期由 nemesisbot/src/runtime_manifest.rs 校验
（三态语义：缺席 warn 不阻塞 / 全过静默 / 违例响亮拒启，
--skip-runtime-check 显式逃生门）。

用法：
    python scripts/gen_runtime_manifest.py --out-dir bin/bin_windows

版本/构建号解析优先级：--version/--commit 参数 > 环境变量
NEMESISBOT_VERSION / NEMESISBOT_GIT_COMMIT > git 探测（describe/rev-parse）
> "unknown"。

设计约束：
- 仅用 Python 标准库（构建机零额外依赖）；
- path 一律正斜杠相对路径（跨平台稳定，与 Rust 校验器共享同一形态）；
- 生成失败（如主二进制缺失）退出码非零 + 明确信息——构建脚本自行决定
  warn+skip（manifest 是增强面，不阻塞打包）。
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

MANIFEST_FILENAME = "runtime-manifest.json"

# 主二进制候选名（按平台探测顺序）。customize/IoT 形态同名。
BIN_CANDIDATES = ["nemesisbot.exe", "nemesisbot", "nemesisbot.bin"]

# 清单覆盖的部署面顶层目录（递归全量）。除主二进制外的关键资产面。
SCANNED_DIRS = ["static"]


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def resolve_version(args) -> str:
    if args.version:
        return args.version
    env = os.environ.get("NEMESISBOT_VERSION")
    if env:
        return env
    try:
        out = subprocess.run(
            ["git", "describe", "--tags", "--abbrev=0"],
            capture_output=True, text=True, timeout=10,
        )
        if out.returncode == 0 and out.stdout.strip():
            return out.stdout.strip()
    except Exception:
        pass
    return "unknown"


def resolve_commit(args) -> str:
    if args.commit:
        return args.commit
    env = os.environ.get("NEMESISBOT_GIT_COMMIT")
    if env:
        return env
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, timeout=10,
        )
        if out.returncode == 0 and out.stdout.strip():
            return out.stdout.strip()
    except Exception:
        pass
    return "unknown"


def collect_files(out_dir: Path):
    """主二进制 + SCANNED_DIRS 递归全量 → [{path, sha256, size}]（路径正斜杠）。"""
    entries = []

    binary = None
    for cand in BIN_CANDIDATES:
        p = out_dir / cand
        if p.is_file():
            binary = p
            break
    if binary is None:
        raise SystemExit(
            f"ERROR: no main binary found in {out_dir} (tried {BIN_CANDIDATES}); "
            "stage build artifacts before generating the manifest"
        )
    entries.append(_entry(out_dir, binary))

    for sub in SCANNED_DIRS:
        root = out_dir / sub
        if not root.is_dir():
            print(f"WARN: {sub}/ not found in {out_dir} — skipped", file=sys.stderr)
            continue
        for p in sorted(root.rglob("*")):
            if p.is_file():
                entries.append(_entry(out_dir, p))
    return entries


def _entry(out_dir: Path, p: Path) -> dict:
    rel = p.relative_to(out_dir).as_posix()
    return {
        "path": rel,
        "sha256": sha256_of(p),
        "size": p.stat().st_size,
    }


def main() -> int:
    ap = argparse.ArgumentParser(description="Generate runtime-manifest.json")
    ap.add_argument("--out-dir", required=True, help="deployment output directory")
    ap.add_argument("--version", default="", help="override version string")
    ap.add_argument("--commit", default="", help="override git commit hash")
    args = ap.parse_args()

    out_dir = Path(args.out_dir)
    if not out_dir.is_dir():
        print(f"ERROR: out-dir does not exist: {out_dir}", file=sys.stderr)
        return 1

    entries = collect_files(out_dir)
    manifest = {
        "version": resolve_version(args),
        "git_commit": resolve_commit(args),
        "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "files": entries,
    }
    target = out_dir / MANIFEST_FILENAME
    with open(target, "w", encoding="utf-8", newline="\n") as f:
        json.dump(manifest, f, indent=2, ensure_ascii=False)
        f.write("\n")
    print(f"  OK {MANIFEST_FILENAME}: {len(entries)} file(s) hashed -> {target}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
