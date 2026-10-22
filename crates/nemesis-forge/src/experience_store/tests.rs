// ExperienceStore tests (production surface: new/from_forge_dir/append/
// read_all/read_recent/count/clear/cleanup).
//
// 聚合 API（append_aggregated/read_aggregated/get_top_patterns/get_stats/
// 每日限额/file_newer_than/walk_jsonl_files）2026-10-22 随生产代码整体移除
// （审计 P2-a），对应测试同步删除。

use super::*;
use crate::types::Experience;

fn make_experience(tool: &str) -> Experience {
    Experience {
        id: uuid::Uuid::new_v4().to_string(),
        tool_name: tool.into(),
        input_summary: "test input".into(),
        output_summary: "ok".into(),
        success: true,
        duration_ms: 100,
        timestamp: chrono::Local::now().to_rfc3339(),
        session_key: "test-session".into(),
    }
}

#[tokio::test]
async fn test_fp2_read_recent_returns_last_n() {
    // F-P2: read_recent parses only the last `limit` lines (bounding parse cost
    // regardless of file size).
    use crate::types::CollectedExperience;
    let dir = tempfile::tempdir().unwrap();
    let exp_dir = dir.path().join("experiences");
    std::fs::create_dir_all(&exp_dir).unwrap();
    let mut content = String::new();
    for i in 0..5u32 {
        let ce = CollectedExperience {
            experience: Experience {
                id: format!("id{}", i),
                tool_name: format!("tool{}", i),
                input_summary: "x".into(),
                output_summary: "y".into(),
                success: true,
                duration_ms: 1,
                timestamp: "2026-01-01T00:00:00+08:00".into(),
                session_key: "s".into(),
            },
            dedup_hash: format!("h{}", i),
        };
        content.push_str(&serde_json::to_string(&ce).unwrap());
        content.push('\n');
    }
    std::fs::write(exp_dir.join("experiences.jsonl"), content).unwrap();

    let store = ExperienceStore::from_forge_dir(dir.path());
    let recent = store.read_recent(3).await.unwrap();
    assert_eq!(recent.len(), 3, "should return only the last 3");
    let names: Vec<&str> = recent
        .iter()
        .map(|c| c.experience.tool_name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["tool2", "tool3", "tool4"],
        "should be the last 3 in order"
    );
}

#[tokio::test]
async fn test_fd2_cleanup_trims_flat_file_by_age() {
    // F-D2: cleanup must trim the flat experiences.jsonl by age (previously it
    // only cleaned YYYYMM/ subdirs and skipped the flat file entirely).
    use crate::types::CollectedExperience;
    let dir = tempfile::tempdir().unwrap();
    let exp_dir = dir.path().join("experiences");
    std::fs::create_dir_all(&exp_dir).unwrap();
    let mk = |tool: &str, ts: &str| {
        serde_json::to_string(&CollectedExperience {
            experience: Experience {
                id: tool.into(),
                tool_name: tool.into(),
                input_summary: "x".into(),
                output_summary: "y".into(),
                success: true,
                duration_ms: 1,
                timestamp: ts.into(),
                session_key: "s".into(),
            },
            dedup_hash: tool.into(),
        })
        .unwrap()
    };
    let now = chrono::Local::now().to_rfc3339();
    let content = format!(
        "{}\n{}\n",
        mk("old_tool", "2020-01-01T00:00:00+08:00"),
        mk("new_tool", &now)
    );
    std::fs::write(exp_dir.join("experiences.jsonl"), content).unwrap();

    let store = ExperienceStore::from_forge_dir(dir.path());
    let removed = store.cleanup(30).await.unwrap();
    assert!(removed >= 1, "should remove the old entry");
    let remaining = std::fs::read_to_string(exp_dir.join("experiences.jsonl")).unwrap();
    assert!(!remaining.contains("old_tool"), "old entry should be gone");
    assert!(remaining.contains("new_tool"), "recent entry should remain");
}

#[tokio::test]
async fn test_from_forge_dir_creates_experiences_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    let ce = CollectedExperience {
        experience: make_experience("tool"),
        dedup_hash: "hash1".into(),
    };
    store.append(&ce).await.unwrap();
    assert!(dir.path().join("experiences").exists());
}

#[tokio::test]
async fn test_cleanup_removes_old() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    // Create an old file manually
    let old_date = chrono::Local::now() - chrono::Duration::days(100);
    let month_dir = dir
        .path()
        .join("experiences")
        .join(old_date.format("%Y%m").to_string());
    std::fs::create_dir_all(&month_dir).unwrap();
    let old_file = month_dir.join(format!("{}.jsonl", old_date.format("%Y%m%d")));
    std::fs::write(&old_file, "test data\n").unwrap();

    // Create a recent file
    let recent_date = chrono::Local::now();
    let recent_month = dir
        .path()
        .join("experiences")
        .join(recent_date.format("%Y%m").to_string());
    std::fs::create_dir_all(&recent_month).unwrap();
    let recent_file = recent_month.join(format!("{}.jsonl", recent_date.format("%Y%m%d")));
    std::fs::write(&recent_file, "recent data\n").unwrap();

    let removed = store.cleanup(30).await.unwrap();
    assert_eq!(removed, 1);
    assert!(!old_file.exists());
    assert!(recent_file.exists());
}

#[tokio::test]
async fn test_cleanup_no_dir() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    let removed = store.cleanup(30).await.unwrap();
    assert_eq!(removed, 0);
}

#[tokio::test]
async fn test_count_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    assert_eq!(store.count().await.unwrap(), 0);
}

#[tokio::test]
async fn test_clear() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    // Write to simple format
    let exp_path = dir.path().join("experiences").join("experiences.jsonl");
    std::fs::create_dir_all(exp_path.parent().unwrap()).unwrap();
    std::fs::write(&exp_path, "data\n").unwrap();

    store.clear().await.unwrap();
    assert!(!exp_path.exists());
}

#[tokio::test]
async fn test_read_all_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    let all = store.read_all().await.unwrap();
    assert!(all.is_empty());
}

#[tokio::test]
async fn test_read_all_with_data() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    // Write to simple format
    let exp_path = dir.path().join("experiences");
    std::fs::create_dir_all(&exp_path).unwrap();
    let exp = make_experience("test_tool");
    let ce = CollectedExperience {
        experience: exp,
        dedup_hash: "test-hash".into(),
    };
    let line = serde_json::to_string(&ce).unwrap();
    tokio::fs::write(exp_path.join("experiences.jsonl"), format!("{}\n", line))
        .await
        .unwrap();

    let all = store.read_all().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].experience.tool_name, "test_tool");
}

#[tokio::test]
async fn test_read_all_ignores_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    let exp_path = dir.path().join("experiences");
    std::fs::create_dir_all(&exp_path).unwrap();
    let content = "invalid json line\n{\"valid\": true}\n";
    tokio::fs::write(exp_path.join("experiences.jsonl"), content)
        .await
        .unwrap();

    let all = store.read_all().await.unwrap();
    assert!(all.is_empty()); // Neither line is valid CollectedExperience
}

#[tokio::test]
async fn test_count_after_append() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    let exp_path = dir.path().join("experiences");
    std::fs::create_dir_all(&exp_path).unwrap();
    let exp = make_experience("tool");
    let ce = CollectedExperience {
        experience: exp,
        dedup_hash: "h".into(),
    };
    let line = serde_json::to_string(&ce).unwrap();
    tokio::fs::write(exp_path.join("experiences.jsonl"), format!("{}\n", line))
        .await
        .unwrap();

    assert_eq!(store.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_clear_nonexistent_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    store.clear().await.unwrap(); // Should not panic
}

#[tokio::test]
async fn test_cleanup_keeps_recent() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    // Write recent data to the flat file.
    let ce = CollectedExperience {
        experience: make_experience("tool"),
        dedup_hash: "hash1".into(),
    };
    store.append(&ce).await.unwrap();

    let removed = store.cleanup(30).await.unwrap();
    assert_eq!(removed, 0);
    assert_eq!(store.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_append_multiple_same_day() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    for i in 0..5 {
        let ce = CollectedExperience {
            experience: make_experience(&format!("tool{}", i)),
            dedup_hash: format!("hash-{}", i),
        };
        store.append(&ce).await.unwrap();
    }

    assert_eq!(store.count().await.unwrap(), 5);
}

#[tokio::test]
async fn test_clear_with_data() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    let exp = make_experience("test_tool");
    let ce = CollectedExperience {
        experience: exp,
        dedup_hash: "hash-1".into(),
    };
    store.append(&ce).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 1);

    store.clear().await.unwrap();
    let all = store.read_all().await.unwrap();
    assert!(all.is_empty());
}

#[tokio::test]
async fn test_append_experiences_raw_format() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    // Append raw collected experiences
    let exp = make_experience("test_tool");
    let ce = CollectedExperience {
        experience: exp,
        dedup_hash: "hash-1".into(),
    };
    store.append(&ce).await.unwrap();

    let all = store.read_all().await.unwrap();
    assert_eq!(all.len(), 1);
}

#[tokio::test]
async fn test_append_immediately_visible_to_read_all() {
    // 回归测试（2026-09-03 Linux CI flake 根因）：tokio `fs::File::write_all`
    // 只把字节提交给后台阻塞任务，`append` 返回时数据可能尚未写入文件
    // （观测特征：文件存在但 len=0 且 created==modified）。缺 `flush` 时本
    // 测试在暖进程里数轮内必红；flush 在位则确定性通过。
    for i in 0..200 {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        let ce = CollectedExperience {
            experience: make_experience("test_tool"),
            dedup_hash: "hash-1".into(),
        };
        store.append(&ce).await.unwrap();
        let all = store.read_all().await.unwrap();
        assert_eq!(
            all.len(),
            1,
            "iteration {i}: append 不可见于紧随其后的 read_all"
        );
    }
}

#[tokio::test]
async fn test_append_keeps_duplicate_hashes() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    let exp = make_experience("test_tool");
    let ce = CollectedExperience {
        experience: exp,
        dedup_hash: "same-hash".into(),
    };
    // First append should succeed
    store.append(&ce).await.unwrap();
    // Second with same hash should also succeed
    store.append(&ce).await.unwrap();

    let all = store.read_all().await.unwrap();
    // append() does not deduplicate — both are stored（旧名
    // test_append_experience_deduplication 暗示去重，与断言相反）。
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn test_read_all_skips_blank_lines() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    std::fs::create_dir_all(base).unwrap();

    let ce = CollectedExperience {
        experience: make_experience("blank_tool"),
        dedup_hash: "hb1".into(),
    };
    let line = serde_json::to_string(&ce).unwrap();
    std::fs::write(
        base.join("experiences.jsonl"),
        format!("\n{}\n\n   \n", line),
    )
    .unwrap();

    let store = ExperienceStore::new(base.to_path_buf());
    let all = store.read_all().await.unwrap();
    assert_eq!(all.len(), 1, "blank lines must be skipped");
    assert_eq!(all[0].experience.tool_name, "blank_tool");
}

#[tokio::test]
async fn test_cleanup_drops_legacy_aggregate_lines() {
    // Lines that parse as the retired AggregatedExperience (legacy pollution,
    // type removed 2026-10-22) are dropped during the flat-file rewrite even
    // when young. The line is hand-built in the old wire format because the
    // type no longer exists.
    use crate::types::CollectedExperience;
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    std::fs::create_dir_all(base).unwrap();

    let ce = CollectedExperience {
        experience: make_experience("young_tool"),
        dedup_hash: "hy".into(),
    };
    let legacy = serde_json::json!({
        "pattern_hash": "legacyhash",
        "tool_name": "agg_tool",
        "count": 9,
        "avg_duration_ms": 100,
        "success_rate": 0.9,
        "last_seen": chrono::Local::now().to_rfc3339(),
    })
    .to_string();
    std::fs::write(
        base.join("experiences.jsonl"),
        format!("{}\n{}\n", serde_json::to_string(&ce).unwrap(), legacy),
    )
    .unwrap();

    let store = ExperienceStore::new(base.to_path_buf());
    let removed = store.cleanup(30).await.unwrap();
    assert!(removed >= 1, "legacy aggregate line counts as removed");

    let remaining = std::fs::read_to_string(base.join("experiences.jsonl")).unwrap();
    assert!(
        remaining.contains("young_tool"),
        "young collected experience must survive: {}",
        remaining
    );
    assert!(
        !remaining.contains("legacyhash"),
        "legacy aggregate line must be dropped: {}",
        remaining
    );
}

#[tokio::test]
async fn test_cleanup_rename_skipped_when_tmp_write_fails() {
    // If the temp rewrite target cannot be written (blocked by a directory),
    // the rename is skipped and the original flat file stays intact.
    use crate::types::CollectedExperience;
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    std::fs::create_dir_all(base).unwrap();

    let mk_old = || {
        serde_json::to_string(&CollectedExperience {
            experience: Experience {
                id: "old".into(),
                tool_name: "old_tool".into(),
                input_summary: "x".into(),
                output_summary: "y".into(),
                success: true,
                duration_ms: 1,
                timestamp: "2020-01-01T00:00:00+08:00".into(),
                session_key: "s".into(),
            },
            dedup_hash: "ho".into(),
        })
        .unwrap()
    };
    std::fs::write(base.join("experiences.jsonl"), format!("{}\n", mk_old())).unwrap();
    // Block the temp path with a directory.
    std::fs::create_dir_all(base.join("experiences.jsonl.tmp")).unwrap();

    let store = ExperienceStore::new(base.to_path_buf());
    let removed = store.cleanup(30).await.unwrap();
    assert!(removed >= 1, "the old entry is still counted as removed");

    // Original file untouched because the rewrite never happened.
    let remaining = std::fs::read_to_string(base.join("experiences.jsonl")).unwrap();
    assert!(
        remaining.contains("old_tool"),
        "rewrite failure must leave the original file intact: {}",
        remaining
    );
}

#[tokio::test]
async fn test_s8_cleanup_ignores_non_jsonl_files_and_missing_flat() {
    let dir = tempfile::tempdir().unwrap();
    let month = dir.path().join("202001");
    std::fs::create_dir_all(&month).unwrap();
    std::fs::write(month.join("20200101.jsonl"), "\n").unwrap();
    std::fs::write(month.join("notes.txt"), "keep me").unwrap();

    let store = ExperienceStore::new(dir.path());
    let removed = store.cleanup(1).await.unwrap();
    // Only the dated .jsonl file is old enough; notes.txt untouched and the
    // flat experiences.jsonl does not exist at all.
    assert_eq!(removed, 1);
    assert!(month.join("notes.txt").exists());
    assert!(!month.join("20200101.jsonl").exists());
}

#[tokio::test]
async fn test_s8_cleanup_flat_file_skips_blank_and_legacy_lines() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::new(dir.path());

    // A young (kept) collected experience.
    let exp = Experience {
        id: "kept-1".into(),
        tool_name: "tool_a".into(),
        input_summary: "in".into(),
        output_summary: "out".into(),
        success: true,
        duration_ms: 10,
        timestamp: chrono::Local::now().to_rfc3339(),
        session_key: "sess".into(),
    };
    let ce = CollectedExperience {
        experience: exp,
        dedup_hash: "sha256:deadbeef".into(),
    };
    store.append(&ce).await.unwrap();

    // Blank line + unparsable legacy line in the flat file.
    use std::io::Write;
    let flat = dir.path().join("experiences.jsonl");
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&flat)
        .unwrap();
    writeln!(f).unwrap();
    writeln!(f, "not json").unwrap();

    let removed = store.cleanup(30).await.unwrap();
    assert_eq!(removed, 1); // only the legacy line dropped
    assert_eq!(store.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_s8_append_empty_base_dir_takes_no_parent_branch() {
    // base_dir "" has no parent component → the `if let Some(parent)`
    // fall-through in append() runs. create_dir_all("") is a no-op, and the
    // open creates experiences.jsonl in the CWD (cargo test 的 cwd = crate 根)。
    // 只清理自己创建的文件：若 crate 根碰巧已有同名文件（上次崩溃残留等），
    // 不得替别人收拾——先记录存在性，append 后按需删除。
    let cwd_file = std::path::Path::new("experiences.jsonl");
    let pre_existing = cwd_file.exists();
    let store = ExperienceStore::new("");
    let exp = Experience {
        id: "e1".into(),
        tool_name: "t".into(),
        input_summary: "i".into(),
        output_summary: "o".into(),
        success: true,
        duration_ms: 1,
        timestamp: chrono::Local::now().to_rfc3339(),
        session_key: "s".into(),
    };
    let ce = CollectedExperience {
        experience: exp,
        dedup_hash: "sha256:x".into(),
    };
    // 空 base_dir 的 append 应当成功（CWD 落盘），失败同样要响亮暴露——
    // 旧版 `let _ =` 吞掉结果，整测退化成纯「不 panic」。
    let res = store.append(&ce).await;
    assert!(
        res.is_ok(),
        "append with empty base_dir must succeed: {res:?}"
    );
    if !pre_existing {
        let _ = tokio::fs::remove_file(cwd_file).await;
    }
}
