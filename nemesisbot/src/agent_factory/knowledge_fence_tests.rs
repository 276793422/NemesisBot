//! 知识库围栏根单一真相源（`knowledge_fence_roots`）+ executor 通道透传
//! 契约测试（天工系统 v1，第二轮横扫 2026-11-26；第三轮横扫 2026-11-27
//! 扩展通道契约断言）。
//!
//! 背景：第一轮横扫修 gateway 侧 boundary 双根放行时只覆盖了 boundary 消费
//! 面；第二轮横扫抓出执行体分离子进程围栏缺同一组双根（exec 分离开关两侧
//! 围栏同根是 exec_world 钦定语义）——per-call 子进程收到的是已批准操作，
//! 但其文件围栏若不含 code_root/info_root，镜像/本地三件套读写会被子进程
//! 围栏拒绝（与 boundary code_root 缺口同族：gateway/run/acp 主臂 wrapper
//! 空 + selfbuilt 臂缺双根 + workflow world 内部空，三处）。第三轮横扫再
//! 抓出围栏修复是必要非充分：子进程侧重定向（`cfg.knowledge`）恒缺席——
//! 围栏放行了、工具仍按字面路径 File not found——通道构造收敛为「句柄
//! 单源派生」（围栏双根 + `knowledge_config_path` env 透传一个来源）。
//! 本文件钉死：
//! 1. `knowledge_fence_roots` 推导三臂（None / disabled → 空集；enabled →
//!    全部配对双根排序去重）——boundary 与 executor 通道同源消费；
//! 2. 通道构造器把句柄派生的双根如实落进 `ExecutorChannel.extra_roots`、
//!    配置路径落进 `knowledge_config_path`（`env_pairs()` 分别写
//!    `NEMESISBOT_EXECUTOR_EXTRA_ROOTS` / `NEMESISBOT_EXECUTOR_KNOWLEDGE_CONFIG`、
//!    子进程侧解析，由 agent crate 自测覆盖，此处不重复）；
//! 3. 项目 loop 形态（knowledge 句柄缺席）= 空集零透传（None 臂）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nemesis_agent::knowledge::KnowledgeHandle;
use nemesis_config::knowledge::{KnowledgeConfig, KnowledgePair, save_knowledge_config};
use nemesis_path::resolve_workspace_knowledge_config_path_in_workspace;

use super::shared::knowledge_fence_roots;

/// 落盘一份 enabled 知识库配置并返回句柄（生产同源路径 helper）。
/// 句柄以 Arc 包装（与生产 `Option<&Arc<KnowledgeHandle>>` 参数形态对齐）。
fn seed_knowledge_config(
    home: &Path,
    enabled: bool,
    pairs: Vec<KnowledgePair>,
) -> Arc<KnowledgeHandle> {
    let ws = home.join("workspace");
    let path = resolve_workspace_knowledge_config_path_in_workspace(&ws);
    std::fs::create_dir_all(path.parent().expect("config dir parent")).expect("mkdir config");
    save_knowledge_config(&path, &KnowledgeConfig { enabled, pairs })
        .expect("save knowledge config");
    Arc::new(KnowledgeHandle::new(path))
}

#[test]
fn fence_roots_none_handle_yields_empty() {
    // 项目 loop（build_project_agent_loop）knowledge=None → 空集零透传。
    assert!(knowledge_fence_roots(None).is_empty());
}

#[test]
fn fence_roots_disabled_config_yields_empty() {
    // enabled=false（fail-closed = 功能不存在）→ 空集，与 None 同语义。
    let tmp = tempfile::tempdir().expect("tempdir");
    let handle = seed_knowledge_config(
        tmp.path(),
        false,
        vec![KnowledgePair {
            code_root: tmp.path().join("proj").to_string_lossy().into_owned(),
            info_root: tmp.path().join("kb").to_string_lossy().into_owned(),
            tiangong_load: nemesis_config::knowledge::TIANGONG_LOAD_POINTER.to_string(),
        }],
    );
    assert!(knowledge_fence_roots(Some(&handle)).is_empty());
}

#[test]
fn fence_roots_enabled_derives_sorted_dedup_double_roots() {
    // 两对乱序插入（b 在前）→ 断言输出排序且四根齐全（双根语义：每对
    // code_root + info_root 都进围栏，不只是 info_root）。
    let tmp = tempfile::tempdir().expect("tempdir");
    let code_a = tmp.path().join("proj-a");
    let info_a = tmp.path().join("kb-a");
    let code_b = tmp.path().join("proj-b");
    let info_b = tmp.path().join("kb-b");
    let pair = |code: &Path, info: &Path| KnowledgePair {
        code_root: code.to_string_lossy().into_owned(),
        info_root: info.to_string_lossy().into_owned(),
        tiangong_load: nemesis_config::knowledge::TIANGONG_LOAD_POINTER.to_string(),
    };
    // 故意 b 对在前、pair 内 code/info 交错，排序职责在函数不在夹具。
    let handle = seed_knowledge_config(
        tmp.path(),
        true,
        vec![pair(&code_b, &info_b), pair(&code_a, &info_a)],
    );

    let mut want: Vec<PathBuf> = vec![code_a, code_b, info_a, info_b];
    want.sort();
    assert_eq!(
        knowledge_fence_roots(Some(&handle)),
        want,
        "enabled 配对的全部双根必须排序后进围栏"
    );
}

#[test]
fn executor_channel_receives_knowledge_fence_roots() {
    // 主臂装配契约（agent_factory/main.rs 同款表达式）：executor.enabled=true
    // + 知识库 enabled → 调用侧只传句柄（非 knowledge 附加根 Vec::new()），
    // 装配函数单源派生的双根必须落进 `ExecutorChannel.extra_roots`、配置
    // 路径必须落进 `knowledge_config_path`（env 透传在 spawn 期由
    // env_pairs() 消费——通道字段是装配面唯一可观测物）。配置经 ConfigStore
    // 注入（夹具真相源，与 build_agent_loop_with_executor_layer1_wraps_move_tools
    // 同纪律）。
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().to_path_buf();
    std::fs::write(
        home.join("config.json"),
        r#"{ "executor": { "enabled": true } }"#,
    )
    .expect("seed config.json");
    let code_root = home.join("proj");
    let info_root = home.join("kb");
    let handle = seed_knowledge_config(
        &home,
        true,
        vec![KnowledgePair {
            code_root: code_root.to_string_lossy().into_owned(),
            info_root: info_root.to_string_lossy().into_owned(),
            tiangong_load: nemesis_config::knowledge::TIANGONG_LOAD_POINTER.to_string(),
        }],
    );

    let store = nemesis_config::ConfigStore::load(&home.join("config.json")).expect("load store");
    let channel = crate::exec_world::build_executor_channel_with_extra_roots(
        &home,
        &home.join("workspace"),
        store.handle(),
        Vec::new(),
        true,
        Some(&handle),
    )
    .expect("channel build must not error offline")
    .expect("executor.enabled=true → Some(channel)");

    let mut want: Vec<PathBuf> = vec![code_root, info_root];
    want.sort();
    assert_eq!(
        channel.extra_roots, want,
        "知识库双根必须透传进 executor 子进程围栏（开关两侧围栏同根）"
    );
    let expected_path =
        resolve_workspace_knowledge_config_path_in_workspace(&home.join("workspace"));
    assert_eq!(
        channel.knowledge_config_path.as_ref(),
        Some(&expected_path),
        "配置路径必须落 channel（env_pairs 写 NEMESISBOT_EXECUTOR_KNOWLEDGE_CONFIG，\
         子进程据此构造 KnowledgeHandle——第三轮横扫修复的重定向透传契约）"
    );
}
