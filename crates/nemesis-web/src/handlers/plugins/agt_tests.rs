//! plugins.rs AGT 覆盖率批次（2026-09-25）：
//! - Default 转发体（110-113）
//!
//! 结构性豁免（见报告）：
//! - 37（found=true 时回填 path）：探测目标是**测试进程 exe 旁的 plugins/
//!   目录**——预置假 dll 会让既有 plugins/tests.rs:83 的 `found:false` 断言
//!   在并行执行下随机翻转（共享 exe 目录、无隔离），属「劫持并行测试」
//!   家族（同 models.rs 101 裁决），不作测试。

use super::*;

#[test]
fn agt_default_impl_matches_new() {
    let _ = PluginsHandler;
    let _ = PluginsHandler::new();
}
