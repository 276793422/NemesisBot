//! session_home 单测：分配 / 解析 / fork 复制三面。
//!
//! meta 侧车经 path 单例落真实 home 的 `logs/session_logs/`——沿用本 crate
//! 测试惯例（session/tests.rs 同款）：uuid 唯一键防碰撞 + delete_chat_log
//! 清 jsonl（meta 侧车不删，与既有测试一致，体积可忽略）。

use std::path::PathBuf;

use super::*;

fn unique_key(prefix: &str) -> String {
    format!("test:sesshome:{}:{}", prefix, uuid::Uuid::new_v4().simple())
}

#[test]
fn test_assign_and_resolve_roundtrip() {
    let key = unique_key("roundtrip");
    let ws = tempfile::tempdir().unwrap();
    let workspace = ws.path().to_path_buf();

    let id = assign_session_home(&workspace, &key).expect("assign should succeed");
    assert!(!id.is_empty());

    // 幂等：第二次分配返回同一 id（不覆盖）。
    let id2 = assign_session_home(&workspace, &key).expect("re-assign should succeed");
    assert_eq!(id, id2);

    // 解析面：id + home 路径都按绑定走。
    assert_eq!(resolve_home_agent_id(&key).as_deref(), Some(id.as_str()));
    let home = resolve_session_home(&workspace, &key).expect("bound session resolves");
    assert_eq!(
        home,
        workspace.join("agents").join(&id).join("home"),
        "home 路径 = <workspace>/agents/<id>/home"
    );
    // 分配即物化：目录在（幂等 create_dir_all，fork/消费方/验收面无需特判）。
    assert!(home.is_dir(), "assign 后 home 目录已物化");
}

#[test]
fn test_resolve_unbound_is_none() {
    let ws = tempfile::tempdir().unwrap();
    let workspace = ws.path().to_path_buf();

    // 从未写过的键：meta 缺席 → None。
    let never = unique_key("never");
    assert!(resolve_home_agent_id(&never).is_none());
    assert!(resolve_session_home(&workspace, &never).is_none());

    // 有 meta 但没绑定（存量会话形态）：照旧 None（lenient 契约）。
    let legacy = unique_key("legacy");
    crate::chat_log::append_chat_log(&legacy, "user", "hi");
    assert!(resolve_home_agent_id(&legacy).is_none());
    assert!(resolve_session_home(&workspace, &legacy).is_none());
}

#[test]
fn test_fork_copy_source_unbound_is_lenient_noop() {
    let source = unique_key("fork-src-unbound");
    let new_key = unique_key("fork-new-unbound");
    let ws = tempfile::tempdir().unwrap();
    let workspace = ws.path().to_path_buf();

    // 源未绑定（存量形态）= 新会话同样不绑定，行为对齐源。
    let result = fork_copy_session_home(&workspace, &source, &new_key)
        .expect("lenient no-op should not fail");
    assert!(result.is_none());
    assert!(resolve_home_agent_id(&new_key).is_none());
}

#[test]
fn test_fork_copy_copies_tree() {
    let source = unique_key("fork-src-bound");
    let new_key = unique_key("fork-new-bound");
    let ws = tempfile::tempdir().unwrap();
    let workspace = ws.path().to_path_buf();

    // 源绑定 + 资产树（嵌套目录 + 内容 + unicode 文件名）。
    let src_id = assign_session_home(&workspace, &source).expect("assign source");
    let src_home = nemesis_path::resolve_agent_home_in_workspace(&workspace, &src_id);
    std::fs::create_dir_all(src_home.join("uploads").join("nested")).unwrap();
    std::fs::create_dir_all(src_home.join("images")).unwrap();
    std::fs::write(src_home.join("uploads").join("a.txt"), "hello uploads").unwrap();
    std::fs::write(
        src_home.join("uploads").join("nested").join("b.md"),
        "# 深层",
    )
    .unwrap();
    std::fs::write(src_home.join("images").join("图片.png"), "png-bytes").unwrap();

    let new_id = fork_copy_session_home(&workspace, &source, &new_key)
        .expect("copy should succeed")
        .expect("source bound → new bound");

    // 新绑定 ≠ 源绑定（独立 uuid）。
    assert_ne!(new_id, src_id);

    let dst_home = nemesis_path::resolve_agent_home_in_workspace(&workspace, &new_id);
    assert_eq!(
        std::fs::read_to_string(dst_home.join("uploads").join("a.txt")).unwrap(),
        "hello uploads"
    );
    assert_eq!(
        std::fs::read_to_string(dst_home.join("uploads").join("nested").join("b.md")).unwrap(),
        "# 深层"
    );
    assert_eq!(
        std::fs::read_to_string(dst_home.join("images").join("图片.png")).unwrap(),
        "png-bytes"
    );
    // 新会话解析面按新 id 走。
    assert_eq!(
        resolve_session_home(&workspace, &new_key),
        Some(PathBuf::from(&dst_home))
    );
    // 源树原样保留（fork 不动源）。
    assert!(src_home.join("uploads").join("a.txt").is_file());
}

#[test]
fn test_fork_copy_source_bound_dir_absent_creates_fresh_home() {
    let source = unique_key("fork-src-ghost");
    let new_key = unique_key("fork-new-ghost");
    let ws = tempfile::tempdir().unwrap();
    let workspace = ws.path().to_path_buf();

    // 绑定在但目录缺席（人工删除/迁移残留）：新会话绑定全新空 home。
    let src_id = assign_session_home(&workspace, &source).expect("assign source");
    // 模拟「目录缺席」：删掉物化目录（绑定仍在 meta）。
    let src_home = nemesis_path::resolve_agent_home_in_workspace(&workspace, &src_id);
    std::fs::remove_dir_all(&src_home).unwrap();
    assert!(!src_home.is_dir());

    let new_id = fork_copy_session_home(&workspace, &source, &new_key)
        .expect("ghost branch should succeed")
        .expect("new session bound");

    let dst_home = nemesis_path::resolve_agent_home_in_workspace(&workspace, &new_id);
    assert!(dst_home.is_dir(), "全新空 home 已物化");
    assert_ne!(new_id, src_id);
}

#[test]
fn test_assign_dir_create_failure_is_err() {
    // 失败路径防线：workspace 是个文件（目录建不出来）= Err——绑定假成功
    // 防线（meta 已写但目录缺席的场景被 Err 诚实暴露，调用方按 lenient
    // 处理时不产出「读不到的绑定」）。
    let key = unique_key("fail-path");
    let ws = tempfile::tempdir().unwrap();
    let file_ws = ws.path().join("not-a-dir");
    std::fs::write(&file_ws, "i am a file").unwrap();

    let result = assign_session_home(&file_ws, &key);
    assert!(result.is_err(), "目录创建失败必须 Err");
}

#[test]
fn test_tampered_home_agent_id_is_rejected() {
    // BUG-20261112-116 红绿：meta 是 workspace 内文件（LLM 经 write_file
    // 可触达），home_agent_id 形态不受控时直接拼 <workspace>/agents/<id>/
    // home 会把 meta 污染升级成路径越界（plans 写放行锚点、uploads/images
    // 落盘、memory 域同源跟随，且内部 fs 写不经 WorkspaceBoundary）。
    // 解析点强制受控 uuid simple 形态：污染 = None（lenient 降级）。
    let key = unique_key("tampered");
    let ws = tempfile::tempdir().unwrap();
    let workspace = ws.path().to_path_buf();

    for evil in [
        "../../../evil",                     // 相对越界链
        "..\\..\\evil",                      // Windows 分隔符越界
        "sub\\dir",                          // 分隔符注入
        "0123456789abcdef0123456789abcdefX", // 33 位（长度不符）
        "0123456789ABCDEF0123456789ABCDEF",  // 大写 hex（非生成形态）
        "not-a-uuid-at-all",                 // 任意短串
    ] {
        crate::chat_log::write_session_home_agent(&key, evil);
        assert!(
            resolve_home_agent_id(&key).is_none(),
            "污染形态 {evil:?} 必须被拒"
        );
        assert!(
            resolve_session_home(&workspace, &key).is_none(),
            "污染形态 {evil:?} 不得解析出 home 路径"
        );
    }

    // 合法生成形态照常通过（uuid simple：32 位小写 hex）。
    let good = uuid::Uuid::new_v4().simple().to_string();
    crate::chat_log::write_session_home_agent(&key, &good);
    assert_eq!(resolve_home_agent_id(&key).as_deref(), Some(good.as_str()));
    let home = resolve_session_home(&workspace, &key).expect("合法绑定照常解析");
    assert_eq!(home, workspace.join("agents").join(&good).join("home"));
}

#[test]
fn test_memory_domain_fork_bound_is_home_vector() {
    // fork 血缘 + home 绑定 → 独立记忆域 <home>/memory/memory_vector。
    let key = unique_key("mem-fork");
    let ws = tempfile::tempdir().unwrap();
    let workspace = ws.path().to_path_buf();

    assign_session_home(&workspace, &key).expect("assign");
    crate::chat_log::write_session_parent(&key, "test:parent-key", 3);

    let home = resolve_session_home(&workspace, &key).expect("bound");
    assert_eq!(
        resolve_session_memory_domain(&workspace, &key),
        Some(home.join("memory").join("memory_vector")),
        "fork 产物记忆域 = <home>/memory/memory_vector"
    );
}

#[test]
fn test_memory_domain_bound_without_lineage_is_none() {
    // 绑定但无 fork 血缘（普通新会话）= None（主域/项目域不变，行为零变化）。
    let key = unique_key("mem-plain");
    let ws = tempfile::tempdir().unwrap();
    let workspace = ws.path().to_path_buf();

    assign_session_home(&workspace, &key).expect("assign");
    assert!(resolve_session_memory_domain(&workspace, &key).is_none());
}

#[test]
fn test_memory_domain_unbound_or_lineage_only_is_none() {
    // 未绑定（存量会话）与只有血缘没绑定（fork 但 copy lenient 跳过）都 =
    // None——两条件必须同时成立才分域。
    let ws = tempfile::tempdir().unwrap();
    let workspace = ws.path().to_path_buf();

    // 存量：连 meta 都没有。
    let never = unique_key("mem-never");
    assert!(resolve_session_memory_domain(&workspace, &never).is_none());

    // 只有血缘：fork 了但源未绑定 → 新会话不绑定（lenient no-op）。
    let lineage_only = unique_key("mem-lineage-only");
    crate::chat_log::append_chat_log(&lineage_only, "user", "hi");
    crate::chat_log::write_session_parent(&lineage_only, "test:parent-key", 0);
    assert!(resolve_session_memory_domain(&workspace, &lineage_only).is_none());
}
