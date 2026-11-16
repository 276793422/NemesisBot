//! 内置技能内嵌树 staging（BUG-20261116-030）。
//!
//! `builtin_registry` 以 `include_dir!` 内嵌仓库 `Skills/` 树。该树含
//! 二进制载荷（2026-11-16 实测：scanner-e2e-test 的 ClamAV 安装包回退
//! zip ≈40.6MB——该技能本身就在装时拦截分档（`EXPECTED_BLOCKED`），
//! zip 永远到不了安装面；desktop-automation 的 window-mcp.exe
//! ≈6.9MB），无条件整树嵌入会把 ≈47.5MB 塞进所有构建——iotsmall
//! 文档预算 ≈10MB 被直接击穿（skills 无 feature 闸恒编译），桌面
//! release 也凭空背测试安装包。内嵌臂的定位是「断网/纯内网实例可搜
//! 可装**技能文档**」，不是二进制载荷分发。
//!
//! 本脚本把 `Skills/` 复制到 `OUT_DIR/builtin_skills` 时跳过 ≥1MB 的
//! 文件（当前即上述两件；42 个文件共 360KB 文本树正常嵌入），并
//! `cargo:warning` 列出被跳过载荷供构建方目检。被跳过的载荷仍随仓库
//! 与 live GitHub 臂可得（源「nemesisbot」的 `Skills/{slug}` 路径含
//! 全量文件）。include_dir 0.7 无过滤能力，staging 复制是唯一过滤点。
//!
//! 阈值 1MB：技能目录的正常构成 = SKILL.md + 方法论文档 + 脚本（当前
//! 42 个文件全部 <100KB）；≥1MB 的实体在本仓库语境只有二进制载荷，
//! 出现即值得构建方目检。回归锁：`builtin_registry/tests.rs`
//! `embedded_tree_excludes_large_payloads_and_keeps_all_skill_docs`。

use std::fs;
use std::path::{Path, PathBuf};

/// 单文件内嵌上限（≥ 此值的文件跳过内嵌，见模块注释）。
const MAX_EMBED_FILE_BYTES: u64 = 1024 * 1024;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let skills_dir = manifest
        .parent()
        .and_then(Path::parent)
        .expect("manifest has grandparent")
        .join("Skills");
    let staging = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("builtin_skills");
    println!("cargo:rerun-if-changed={}", skills_dir.display());
    // Skills/ 不在（非仓库形态构建/发布包）= 空内嵌树，registry 自然空。
    if !skills_dir.is_dir() {
        fs::create_dir_all(&staging).expect("create empty staging dir");
        return;
    }
    copy_filtered(&skills_dir, &staging);
}

/// 递归复制（读目录顺序无关紧要——复制产物与序无关，消费侧 `dirs()`
/// 按树形结构访问）。
fn copy_filtered(dir: &Path, staging: &Path) {
    fs::create_dir_all(staging).expect("create staging subdir");
    for entry in fs::read_dir(dir).expect("read skills dir") {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => panic!("read skills entry failed: {e}"),
        };
        let path = entry.path();
        if path.is_dir() {
            copy_filtered(&path, &staging.join(entry.file_name()));
            continue;
        }
        let len = entry.metadata().expect("file metadata").len();
        if len >= MAX_EMBED_FILE_BYTES {
            println!(
                "cargo:warning=[builtin-skills-embed] skipping large file ({} bytes): {}",
                len,
                path.display()
            );
            continue;
        }
        fs::copy(&path, staging.join(entry.file_name()))
            .unwrap_or_else(|e| panic!("copy {} failed: {e}", path.display()));
    }
}
