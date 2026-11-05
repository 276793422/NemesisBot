//! skills.rs AGT 覆盖率批次（2026-09-24）。
//!
//! 前三个测试子模块已覆盖 parse_github_url 主臂 / source_add 重名拒绝 /
//! wiremock 网络臂；本文件只补剩余确定性臂：
//! - `Default` impl（tests 全走 `new()`，default 恒空）
//! - `parse_github_url` 的条件假延续臂：git@ 前缀命中但无 owner/repo 斜杠
//!   （`git@github.com:onlyrepo` → 落出 git@ 分支）、shorthand 有一侧为空
//!   （`/repo` → owner 空）→ 都落到最终 Err
//!
//! 结构性豁免（见报告）：`detect_skill_structure` 与 `source_add` 探测后
//! 全臂（api.github.com 网络）、`browse` 的 installed 集合臂（registry
//! 网络拉取前置）。

use super::*;

#[test]
fn agt_default_impl_and_parse_fallthrough_arms() {
    // Default impl（此前无测试触达）
    let h = SkillsHandler::default();
    assert_eq!(h.module_name(), "skills");

    // git@ 前缀命中但无 '/' → 不返 Ok，落到最终 Err（812 延续臂）
    let err = parse_github_url("git@github.com:onlyrepo").unwrap_err();
    assert!(err.contains("无法解析 URL"), "err: {err}");

    // shorthand owner 为空（"/repo"）→ 不返 Ok，落到最终 Err（819 延续臂）
    assert!(parse_github_url("/justrepo").is_err());
    // shorthand repo 为空（"user/"）→ 同臂
    assert!(parse_github_url("user/").is_err());
}
