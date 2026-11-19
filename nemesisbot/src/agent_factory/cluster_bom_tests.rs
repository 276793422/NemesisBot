//! BUG-20261119-179：集群 B 端 system prompt 装配三处人格文件读取
//! （IDENTITY/SOUL/EXPERTISE）走 read_config_string 剥前导 UTF-8 BOM
//! （记事本保存形态）——BOM 前缀不得进稳定前缀。纯装配逻辑，无 feature
//! 内 cluster 编排依赖（与 mod.rs 声明同挂 cluster 门，随模块走）。

use super::cluster::load_cluster_system_prompt;

#[test]
fn cluster_prompt_strips_bom_from_persona_files() {
    let tmp = tempfile::tempdir().unwrap();
    let cluster_dir = tmp.path().join("workspace").join("cluster");
    std::fs::create_dir_all(&cluster_dir).unwrap();
    std::fs::write(cluster_dir.join("IDENTITY.md"), "\u{feff}集群身份内容").unwrap();
    std::fs::write(cluster_dir.join("SOUL.md"), "集群灵魂内容").unwrap();

    let prompt = load_cluster_system_prompt(tmp.path(), true).expect("persona files present");

    assert!(prompt.contains("集群身份内容"));
    assert!(prompt.contains("集群灵魂内容"));
    assert!(
        !prompt.contains('\u{feff}'),
        "zero-width BOM must not leak into cluster system prompt"
    );
    // worker_discipline=true：行为契约段在场。
    assert!(prompt.contains(nemesis_prompts::professions::CLUSTER_WORKER_CONTRACT));
}

#[test]
fn cluster_prompt_bom_only_file_counts_as_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let cluster_dir = tmp.path().join("workspace").join("cluster");
    std::fs::create_dir_all(&cluster_dir).unwrap();
    // 只有 BOM 的文件（trim 后为空——BOM 不在 is_whitespace 内，但剥除
    // 后 trim 为空）= 视为空身份文件，不注入。
    std::fs::write(cluster_dir.join("IDENTITY.md"), "\u{feff}").unwrap();

    let prompt = load_cluster_system_prompt(tmp.path(), true).expect("contract section present");
    assert!(
        !prompt.contains('\u{feff}'),
        "BOM-only persona file must not inject anything"
    );
}
