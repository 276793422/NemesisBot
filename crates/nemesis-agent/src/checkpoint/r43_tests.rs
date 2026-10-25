//! BUG-20261025-43（BUG-HUNT R120）：persist 原子写 + load 损坏镜像 loud 留痕
//! 回归锁。
//!
//! 修复前 `persist` 用裸 `std::fs::write` 直写目标——崩溃/断电落在写中 →
//! 半截 JSON（turn-N.json 每 turn 被 snapshot/seal 多次覆写，撕裂毁掉的是
//! 先前已落盘的好镜像），重启后 load 解析失败被静默跳过 → rewind 选择器丢
//! 条目、tree 基线/封印指纹消失（冲突预检对该 turn 致盲）、redo 陈旧性守卫
//! 基线漂移。修复后走 `nemesis_utils::write_file_atomic`（唯一临时名 +
//! sync_all 先于 rename + 失败清理），load 解析失败 warn 留痕。
//!
//! 进程内无法真实注入「写中崩溃」（loop_continuation R14 同族先例的诚实
//! 边界），本批锁可观察行为：
//! ① persist→重载往返保真 + seal 覆写取最新 + 全程零 `.tmp-` 残留；
//! ② 崩溃残留的 `.tmp-*`（无 .json 扩展名）不被 load 当 checkpoint、不被
//!    truncate_from_scoped 的 turn-*.json 扫描误删——原子写引入临时文件后
//!    的新不变量；
//! ③ 旧形态撕裂产物（半截 turn-N.json）被 load 跳过后，同 turn 再次
//!    persist 必须把损坏文件修复成合法镜像（覆盖写路径健在）。

use super::*;

fn r43_change(rel: &str, kind: FileChangeKind) -> FileChange {
    FileChange {
        path: rel.to_string(),
        kind,
    }
}

/// persist→重载往返保真；seal 覆写同一 turn 后重载取最新（sealed=true +
/// after 指纹）；全程零 `.tmp-` 残留（原子写的 rename/清理路径健在）。
#[tokio::test]
async fn persist_roundtrip_seal_overwrite_and_no_tmp_residue() {
    let dir = tempfile::tempdir().unwrap();
    let ckpt_dir = dir.path().join(".ck");
    let root = dir.path().to_path_buf();

    {
        let store = CheckpointStore::new(Some(ckpt_dir.clone()), root.clone());
        store.begin(0, "r43 roundtrip");
        // Create kind → content=None，仍走 persist（落盘纪律对 snapshot 无条件）。
        store
            .snapshot(&r43_change("a.txt", FileChangeKind::Create))
            .await;
        tokio::fs::write(root.join("a.txt"), "body-v1")
            .await
            .unwrap();
        // seal 覆写 turn-0.json（补 after 指纹 + sealed 旗标）。
        store.seal_turn(0);
    }

    let mirror = ckpt_dir.join("turn-0.json");
    assert!(mirror.exists(), "turn-0.json 必须已落盘");
    // 原子写不得残留临时文件。
    let residue: Vec<_> = std::fs::read_dir(&ckpt_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
        .collect();
    assert!(
        residue.is_empty(),
        "checkpoint 目录不得残留 .tmp- 临时文件，实际 {residue:?}"
    );

    // 重载：往返保真（声明路径在、sealed=true、after 指纹带 a.txt）。
    let store2 = CheckpointStore::new(Some(ckpt_dir.clone()), root.clone());
    let metas = store2.list_meta();
    assert_eq!(metas.len(), 1, "重载必须看到 turn 0，实际 {metas:?}");
    assert_eq!(metas[0].turn, 0);
    assert!(
        metas[0].paths.iter().any(|p| p == "a.txt"),
        "重载后声明路径必须含 a.txt，实际 {:?}",
        metas[0].paths
    );

    // 覆盖写路径在第一个块内已走到：snapshot 先落 sealed=false 的 v1 镜像，
    // seal_turn 再原子覆盖成 sealed=true + after 指纹的 v2——盘上终态必须是
    // 合法的 v2（覆盖不撕裂、不追加）。
    let bytes = std::fs::read(&mirror).unwrap();
    let reparsed: Checkpoint =
        serde_json::from_slice(&bytes).expect("seal 覆盖后 turn-0.json 必须仍是合法 JSON");
    assert!(reparsed.sealed);
    assert!(
        reparsed.after.iter().any(|f| f.path == "a.txt"),
        "seal 覆盖后的镜像必须带 after 指纹"
    );
}

/// 崩溃残留的 `.tmp-*` 文件（无 .json 扩展名）不得被 load 当 checkpoint，
/// 也不得被 truncate_from_scoped 的 turn-*.json 磁盘扫描误删/误判——
/// 原子写引入临时文件形态后，镜像扫描必须天然无视它们。
#[tokio::test]
async fn crash_tmp_residue_ignored_by_load_and_truncate() {
    let dir = tempfile::tempdir().unwrap();
    let ckpt_dir = dir.path().join(".ck");
    std::fs::create_dir_all(&ckpt_dir).unwrap();
    // 模拟崩溃残缺：半截 JSON 落在 write_file_atomic 的临时名形态里。
    let tmp = ckpt_dir.join(format!(".tmp-{}-0-1", std::process::id()));
    std::fs::write(&tmp, r#"{"turn": 99, "prompt": "half-writ"#).unwrap();

    let root = dir.path().to_path_buf();
    let store = CheckpointStore::new(Some(ckpt_dir.clone()), root.clone());
    assert!(
        store.list_meta().is_empty(),
        "崩溃残留 .tmp-* 不得进 checkpoint 索引"
    );

    // 全局截断的磁盘镜像扫描也不得碰 .tmp-*（残留归原子写自己的失败清理）。
    store.truncate_from(0);
    assert!(
        tmp.exists(),
        "truncate_from_scoped 不得误删 .tmp-* 崩溃残留"
    );
}

/// 旧形态撕裂产物（半截 turn-N.json，修复前裸写的崩溃产物）被 load 跳过
/// 后，同 turn 再次 persist 必须把它覆盖成合法镜像——损坏不粘连后续落盘。
#[tokio::test]
async fn torn_mirror_skipped_then_repaired_by_next_persist() {
    let dir = tempfile::tempdir().unwrap();
    let ckpt_dir = dir.path().join(".ck");
    std::fs::create_dir_all(&ckpt_dir).unwrap();
    // 模拟修复前形态的撕裂镜像。
    std::fs::write(ckpt_dir.join("turn-3.json"), r#"{"turn": 3, "prom"#).unwrap();

    let root = dir.path().to_path_buf();
    // load：损坏镜像被跳过（并 warn 留痕），不炸不进索引。
    {
        let store = CheckpointStore::new(Some(ckpt_dir.clone()), root.clone());
        assert!(store.list_meta().is_empty(), "撕裂镜像不得进索引");
    }
    // 同 turn 再次 persist → 覆盖成合法 JSON。
    let store = CheckpointStore::new(Some(ckpt_dir.clone()), root.clone());
    store.begin(3, "r43 repair");
    tokio::fs::write(root.join("b.txt"), "body").await.unwrap();
    store
        .snapshot(&r43_change("b.txt", FileChangeKind::Create))
        .await;

    let bytes = std::fs::read(ckpt_dir.join("turn-3.json")).unwrap();
    let reparsed: Checkpoint =
        serde_json::from_slice(&bytes).expect("重 persist 后 turn-3.json 必须是合法镜像");
    assert_eq!(reparsed.turn, 3);
    assert!(reparsed.paths.iter().any(|p| p == "b.txt"));
}
