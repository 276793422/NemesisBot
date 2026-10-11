#!/usr/bin/env bash
# =============================================================================
# NemesisBot WASM 插件开发包（devkit）打包脚本
#
# 产物：nightly-wasm-devkit.zip —— 最小可编译集合（不引用主仓库任何路径，
# 用户解压后 cargo run -p pack 一键编译+打包全部示例）。CI 在 daily-release
# （仅 linux，平台无关源码包）调用；本地开发验证同款命令。
#
# 用法：
#   bash scripts/pack-wasm-devkit.sh            # 组包 + 打 zip
#   bash scripts/pack-wasm-devkit.sh --verify   # 组包 + 打包 + 解压实编验证
#   bash scripts/pack-wasm-devkit.sh --verify --keep  # 验证后保留解压目录（排障）
#
# devkit 结构（staging-wasm-devkit/wasm-plugin-devkit/）：
#   Cargo.toml        ← 根 workspace（只收 pack；素材 scripts/wasm-devkit/root-Cargo.toml）
#   README.md         ← 素材 scripts/wasm-devkit/README.md（版本/commit 占位符替换）
#   pack/             ← 素材 scripts/wasm-devkit/pack/（纯 Rust 打包工具）
#   sdk/              ← crates/nemesis-plugin-sdk standalone 化（workspace 继承写死）
#   examples/<name>/  ← 仓库示例拷贝；sdk path 依赖改写为 ../sdk；Cargo.lock 随包
#
# 示例名单 = git ls-files 已入库者（工作区未提交的示例不进包——本地与 CI
# 产物一致）。当前入库：textstat / activity-log / translate。
# =============================================================================
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
SRC_SDK="$REPO_ROOT/crates/nemesis-plugin-sdk"
SRC_EXAMPLES="$REPO_ROOT/plugins/wasm"
ASSETS="$REPO_ROOT/scripts/wasm-devkit"
STAGE="$REPO_ROOT/staging-wasm-devkit"
PKG="$STAGE/wasm-plugin-devkit"
OUT_ZIP="$REPO_ROOT/nightly-wasm-devkit.zip"

VERIFY=0; KEEP=0
for a in "$@"; do
  case "$a" in
    --verify) VERIFY=1 ;;
    --keep)   KEEP=1 ;;
    *) echo "未知参数: $a"; exit 1 ;;
  esac
done

# ---- 版本信息（对齐构建脚本的 tag 优先口径）--------------------------------
# README 展示用原始 tag；Cargo.toml 的 version 字段必须是合法 semver——
# 仓库 tag 可能是 nightly-build 这类非 semver 形态，抠不到就落 0.0.0。
VERSION=$(git -C "$REPO_ROOT" describe --tags --abbrev=0 2>/dev/null || echo "0.0.0.dev")
COMMIT=$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo "unknown")
VER_SEMVER=$(printf '%s' "$VERSION" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)
VER_SEMVER=${VER_SEMVER:-"0.0.0"}

# ---- 示例名单：git 已入库的 plugins/wasm/<dir>（含 Cargo.toml 者）----------
# 注意 git ls-files 输出仓库相对路径（与 pathspec 形态无关），strip 用相对前缀；
# 新示例入库即自动进名单，无需改本脚本。
EX_REL="plugins/wasm"
mapfile -t EXAMPLES < <(git -C "$REPO_ROOT" ls-files "$EX_REL" \
  | sed "s|^$EX_REL/||" | cut -d/ -f1 | sort -u \
  | while read -r d; do
      [ -n "$d" ] && git -C "$REPO_ROOT" ls-files --error-unmatch \
        "$EX_REL/$d/Cargo.toml" >/dev/null 2>&1 && echo "$d"
    done)
if [ "${#EXAMPLES[@]}" -eq 0 ]; then
  echo "❌ plugins/wasm 下没有任何已入库示例"; exit 1
fi
echo "=== devkit 示例名单: ${EXAMPLES[*]}"

# ---- standalone sdk 的字段值：从根 Cargo.toml 抠取（与主仓库同源同点）------
EDITION=$(grep -E '^edition = ' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/.*= *"//;s/".*//')
WIT_BINDGEN=$(grep -E '^wit-bindgen = ' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/.*= *"//;s/".*//')
SERDE_VER=$(grep -E '^serde = ' "$REPO_ROOT/Cargo.toml" | head -1 \
  | sed 's/.*version *= *"\([^"]*\)".*/\1/')
[ -n "$EDITION" ] && [ -n "$WIT_BINDGEN" ] && [ -n "$SERDE_VER" ] \
  || { echo "❌ 从根 Cargo.toml 抠取 edition/wit-bindgen/serde 版本失败"; exit 1; }
echo "=== standalone sdk: edition=$EDITION wit-bindgen=$WIT_BINDGEN serde=$SERDE_VER"

# ---- 组包 -------------------------------------------------------------------
rm -rf "$STAGE"
mkdir -p "$PKG"

tar_copy() { # tar_copy <src> <dst> [exclude 前缀...] —— 零 rsync 依赖的目录拷贝
  local src=$1 dst=$2; shift 2
  local excl=(); for e in "$@"; do excl+=(--exclude="$e"); done
  mkdir -p "$dst"
  tar -C "$src" "${excl[@]}" -cf - . | tar -C "$dst" -xf -
}

# 1. pack + 根 workspace + README（占位符替换）。pack/Cargo.toml 尾部的
# [workspace] 空表是仓库内独立构建所需（scripts/ 不在主 workspace 树），
# 进 devkit 后 pack 由根 workspace 收编——剥离，否则 multiple workspace roots。
tar_copy "$ASSETS/pack" "$PKG/pack" target
sed -i '/^\[workspace\]/d' "$PKG/pack/Cargo.toml"
cp "$ASSETS/root-Cargo.toml" "$PKG/Cargo.toml"
sed -e "s|@DEVKIT_VERSION@|$VERSION|g" -e "s|@DEVKIT_COMMIT@|$COMMIT|g" \
  "$ASSETS/README.md" > "$PKG/README.md"

# 2. sdk standalone：src 原样拷 + wit 从宿主 crate 权威合同目录拷（SDK 无
# 本地副本，devkit 自包含要求包内自带——来源单一化后不存在拷错版本）。
# host.rs 的 bindgen path 在仓库内指向宿主 crate（crate 外），devkit 内无
# 该目录——改写回 crate 内 /wit（wit 已拷至 sdk/wit）。
tar_copy "$SRC_SDK/src" "$PKG/sdk/src"
tar_copy "$REPO_ROOT/crates/nemesis-plugins-wasm/wit" "$PKG/sdk/wit"
sed -i 's|"/\.\./nemesis-plugins-wasm/wit"|"/wit"|' "$PKG/sdk/src/host.rs"
grep -q '"/\.\./nemesis-plugins-wasm/wit"' "$PKG/sdk/src/host.rs" \
  && { echo "❌ devkit sdk host.rs 残留仓库内 wit 路径引用"; exit 1; }
cat > "$PKG/sdk/Cargo.toml" <<EOF
[package]
name = "nemesis-plugin-sdk"
version = "$VER_SEMVER"
edition = "$EDITION"
publish = false

description = "NemesisBot WASM 插件三方开发 SDK（guest 侧：宿主能力面封装 + 导出宏）——devkit standalone 版（由 scripts/pack-wasm-devkit.sh 生成）"
license = "MIT OR Apache-2.0"

[dependencies]
wit-bindgen = "$WIT_BINDGEN"
serde = { version = "$SERDE_VER", features = ["derive"], optional = true }
serde_json = { version = "$SERDE_VER", optional = true }

[features]
default = ["json"]
json = ["dep:serde", "dep:serde_json"]

# standalone 声明：sdk 不进 devkit 根 workspace（根只收 pack）
[workspace]
EOF

# 3. 示例：原样拷（排除 target/dist）+ sdk path 依赖改写 + Cargo.lock 随包
for name in "${EXAMPLES[@]}"; do
  tar_copy "$SRC_EXAMPLES/$name" "$PKG/examples/$name" target dist
  # 原始写法示例（直接依赖 wit-bindgen）无此行，sed 不命中即原样。
  # 相对层级：examples/<name>/ → devkit 根的 sdk/，须两级上跳。
  sed -i 's|path *= *"[^"]*crates/nemesis-plugin-sdk"|path = "../../sdk"|' \
    "$PKG/examples/$name/Cargo.toml"
  if [ -f "$SRC_EXAMPLES/$name/Cargo.lock" ]; then
    cp "$SRC_EXAMPLES/$name/Cargo.lock" "$PKG/examples/$name/Cargo.lock"
  else
    echo "⚠️ 示例 $name 无 Cargo.lock（跳过——依赖解析交由用户构建时）"
  fi
  grep -q 'path = "\.\./\.\./sdk"' "$PKG/examples/$name/Cargo.toml" \
    || echo "（$name：原始写法示例，直连 wit-bindgen，无需 sdk）"
done

# sdk standalone lock：generate-lockfile（拉 index/缓存，不编译）——保证包内
# 依赖组合 = 发布时验证过的组合
if (cd "$PKG/sdk" && cargo generate-lockfile --offline) \
   || (cd "$PKG/sdk" && cargo generate-lockfile); then
  echo "=== sdk/Cargo.lock 已生成"
else
  echo "⚠️ sdk generate-lockfile 失败（无网络且无缓存）——devkit 无 sdk lock 继续"
fi

echo "=== staging-wasm-devkit 结构 ==="
find "$STAGE" -maxdepth 3 -type d | sort
echo "=== 校验：devkit 内不得残留主仓库路径引用 ==="
if grep -rn "crates/nemesis-plugin-sdk\|NemesisBot_Rust" "$PKG" --include="*.toml" --include="*.rs" -l; then
  echo "❌ devkit 残留主仓库路径引用"; exit 1
fi
echo "（无残留）"

# ---- 打包（zip 优先；zip 缺席时 tar.gz 兜底并提示，CI 环境恒有 zip）---------
if command -v zip >/dev/null 2>&1; then
  (cd "$STAGE" && zip -qr "$OUT_ZIP" wasm-plugin-devkit)
  echo "=== 产物: $OUT_ZIP"; unzip -l "$OUT_ZIP" | tail -5
else
  (cd "$STAGE" && tar -czf "${OUT_ZIP%.zip}.tar.gz" wasm-plugin-devkit)
  echo "=== 本机无 zip 命令，产物: ${OUT_ZIP%.zip}.tar.gz（CI 环境恒产 zip）"
fi

# ---- --verify：解压实编（CI 质量门：编不过不上 release）---------------------
if [ "$VERIFY" -eq 1 ]; then
  VERIFY_DIR=$(mktemp -d "${TMPDIR:-/tmp}/devkit-verify.XXXXXX")
  trap '[ "$KEEP" -eq 1 ] || rm -rf "$VERIFY_DIR"' EXIT
  echo "=== verify: 解压到 $VERIFY_DIR ==="
  if [ -f "$OUT_ZIP" ]; then
    unzip -q "$OUT_ZIP" -d "$VERIFY_DIR"
  else
    tar -xzf "${OUT_ZIP%.zip}.tar.gz" -C "$VERIFY_DIR"
  fi
  echo "=== verify: cargo run -p pack（真实编译全部示例）==="
  # rustup target add 必须在 verify 目录内跑：本仓库 rust-toolchain.toml 钉
  # 1.95.0，在仓库根执行会装进钉住工具链；临时目录的 cargo 用默认工具链，
  # 两者不一致 = E0463 can't find crate for core（本轮实测）。
  (cd "$VERIFY_DIR/wasm-plugin-devkit" \
    && rustup target add wasm32-wasip2 && cargo run -p pack)
  echo "=== verify: staging 产物清单 ==="
  ls -la "$VERIFY_DIR/wasm-plugin-devkit/dist/"
  if [ "$KEEP" -eq 1 ]; then
    echo "=== verify 目录保留: $VERIFY_DIR ==="
  fi
  echo "✅ devkit 验证通过（最小可编译集合成立）"
fi
