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

fn make_aggregated(hash: &str, tool: &str, count: u64) -> AggregatedExperience {
    AggregatedExperience {
        pattern_hash: hash.to_string(),
        tool_name: tool.to_string(),
        count,
        avg_duration_ms: 100,
        success_rate: 0.9,
        last_seen: chrono::Local::now().to_rfc3339(),
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
async fn test_append_and_read_aggregated() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    let agg1 = make_aggregated("hash1", "file_read", 10);
    let agg2 = make_aggregated("hash2", "file_write", 5);

    store.append_aggregated(&agg1).await.unwrap();
    store.append_aggregated(&agg2).await.unwrap();

    let all = store.read_aggregated().await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn test_read_aggregated_since_filter() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    // Create an old file manually
    let old_date = chrono::Local::now() - chrono::Duration::days(100);
    let old_month = dir
        .path()
        .join("experiences")
        .join(old_date.format("%Y%m").to_string());
    std::fs::create_dir_all(&old_month).unwrap();
    let old_agg = AggregatedExperience {
        pattern_hash: "old_hash".to_string(),
        tool_name: "old_tool".to_string(),
        count: 5,
        avg_duration_ms: 200,
        success_rate: 0.5,
        last_seen: old_date.to_rfc3339(),
    };
    let old_file = old_month.join(format!("{}.jsonl", old_date.format("%Y%m%d")));
    std::fs::write(
        &old_file,
        format!("{}\n", serde_json::to_string(&old_agg).unwrap()),
    )
    .unwrap();

    // Append a recent record
    let recent_agg = make_aggregated("recent_hash", "recent_tool", 10);
    store.append_aggregated(&recent_agg).await.unwrap();

    // Read all
    let all = store.read_aggregated().await.unwrap();
    assert_eq!(all.len(), 2);

    // Read since 7 days ago (should only get recent)
    let since = chrono::Local::now() - chrono::Duration::days(7);
    let recent = store.read_aggregated_since(Some(since)).await.unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].pattern_hash, "recent_hash");
}

#[tokio::test]
async fn test_read_aggregated_by_day() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    let agg = make_aggregated("hash1", "tool_a", 3);
    store.append_aggregated(&agg).await.unwrap();

    let by_day = store.read_aggregated_by_day().await.unwrap();
    assert!(!by_day.is_empty());
}

#[tokio::test]
async fn test_get_top_patterns() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    let agg1 = make_aggregated("hash1", "file_read", 50);
    let agg2 = make_aggregated("hash2", "file_write", 10);
    let agg3 = make_aggregated("hash3", "exec", 30);

    store.append_aggregated(&agg1).await.unwrap();
    store.append_aggregated(&agg2).await.unwrap();
    store.append_aggregated(&agg3).await.unwrap();

    let top = store.get_top_patterns(2).await.unwrap();
    assert_eq!(top.len(), 2);
    assert_eq!(top[0].tool_name, "file_read");
    assert_eq!(top[1].tool_name, "exec");
}

#[tokio::test]
async fn test_get_top_patterns_since() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    // Create an old file manually with high count
    let old_date = chrono::Local::now() - chrono::Duration::days(100);
    let old_month = dir
        .path()
        .join("experiences")
        .join(old_date.format("%Y%m").to_string());
    std::fs::create_dir_all(&old_month).unwrap();
    let old_agg = AggregatedExperience {
        pattern_hash: "old_hash".to_string(),
        tool_name: "old_tool".to_string(),
        count: 999,
        avg_duration_ms: 100,
        success_rate: 0.9,
        last_seen: old_date.to_rfc3339(),
    };
    let old_file = old_month.join(format!("{}.jsonl", old_date.format("%Y%m%d")));
    std::fs::write(
        &old_file,
        format!("{}\n", serde_json::to_string(&old_agg).unwrap()),
    )
    .unwrap();

    // Append recent records
    store
        .append_aggregated(&make_aggregated("hash1", "recent_tool", 20))
        .await
        .unwrap();

    // Without filter, old should be included
    let all_top = store.get_top_patterns(1).await.unwrap();
    assert_eq!(all_top[0].tool_name, "old_tool"); // 999 > 20

    // With since filter, only recent should appear
    let since = chrono::Local::now() - chrono::Duration::days(7);
    let recent_top = store.get_top_patterns_since(Some(since), 1).await.unwrap();
    assert_eq!(recent_top[0].tool_name, "recent_tool");
}

#[tokio::test]
async fn test_get_stats() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    let agg = make_aggregated("hash1", "tool", 10);
    store.append_aggregated(&agg).await.unwrap();
    let agg2 = make_aggregated("hash2", "tool2", 5);
    store.append_aggregated(&agg2).await.unwrap();

    let (total, unique) = store.get_stats().await.unwrap();
    assert_eq!(total, 15);
    assert_eq!(unique, 2);
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
async fn test_daily_limit_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExperienceStoreConfig {
        max_experiences_per_day: 2,
    };
    let store = ExperienceStore::from_forge_dir_with_config(dir.path(), config);

    let agg = make_aggregated("hash1", "tool", 10);

    // First two should succeed
    store.append_aggregated(&agg).await.unwrap();
    store.append_aggregated(&agg).await.unwrap();

    // Third should be silently dropped
    store.append_aggregated(&agg).await.unwrap();

    let all = store.read_aggregated().await.unwrap();
    assert_eq!(all.len(), 2); // Only 2, not 3
}

#[tokio::test]
async fn test_daily_limit_zero_means_unlimited() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExperienceStoreConfig {
        max_experiences_per_day: 0,
    };
    let store = ExperienceStore::from_forge_dir_with_config(dir.path(), config);

    let agg = make_aggregated("hash1", "tool", 10);

    for _ in 0..5 {
        store.append_aggregated(&agg).await.unwrap();
    }

    let all = store.read_aggregated().await.unwrap();
    assert_eq!(all.len(), 5);
}

// --- Additional experience_store tests ---

#[tokio::test]
async fn test_from_forge_dir_creates_experiences_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    let agg = make_aggregated("hash1", "tool", 1);
    store.append_aggregated(&agg).await.unwrap();
    assert!(dir.path().join("experiences").exists());
}

#[tokio::test]
async fn test_read_aggregated_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    let all = store.read_aggregated().await.unwrap();
    assert!(all.is_empty());
}

#[tokio::test]
async fn test_read_aggregated_since_no_filter() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    let agg = make_aggregated("hash1", "tool", 5);
    store.append_aggregated(&agg).await.unwrap();
    let all = store.read_aggregated_since(None).await.unwrap();
    assert_eq!(all.len(), 1);
}

#[tokio::test]
async fn test_read_aggregated_since_future() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    let agg = make_aggregated("hash1", "tool", 5);
    store.append_aggregated(&agg).await.unwrap();
    let future = chrono::Local::now() + chrono::Duration::days(1);
    let result = store.read_aggregated_since(Some(future)).await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn test_get_top_patterns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    let top = store.get_top_patterns(10).await.unwrap();
    assert!(top.is_empty());
}

#[tokio::test]
async fn test_get_top_patterns_all_returned() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    store
        .append_aggregated(&make_aggregated("h1", "a", 10))
        .await
        .unwrap();
    store
        .append_aggregated(&make_aggregated("h2", "b", 5))
        .await
        .unwrap();
    store
        .append_aggregated(&make_aggregated("h3", "c", 1))
        .await
        .unwrap();
    let top = store.get_top_patterns(0).await.unwrap();
    assert_eq!(top.len(), 3);
}

#[tokio::test]
async fn test_get_stats_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    let (total, unique) = store.get_stats().await.unwrap();
    assert_eq!(total, 0);
    assert_eq!(unique, 0);
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

#[test]
fn test_file_newer_than_same_day() {
    let now = chrono::Local::now();
    let filename = format!("{}.jsonl", now.format("%Y%m%d"));
    assert!(ExperienceStore::file_newer_than(&filename, &now));
}

#[test]
fn test_file_newer_than_older() {
    let now = chrono::Local::now();
    let old_date = now - chrono::Duration::days(10);
    let filename = format!("{}.jsonl", old_date.format("%Y%m%d"));
    assert!(!ExperienceStore::file_newer_than(&filename, &now));
}

#[test]
fn test_file_newer_than_newer() {
    let now = chrono::Local::now();
    let future_date = now + chrono::Duration::days(10);
    let filename = format!("{}.jsonl", future_date.format("%Y%m%d"));
    assert!(ExperienceStore::file_newer_than(&filename, &now));
}

#[tokio::test]
async fn test_cleanup_keeps_recent() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    // Write recent data
    let agg = make_aggregated("hash1", "tool", 5);
    store.append_aggregated(&agg).await.unwrap();

    let removed = store.cleanup(30).await.unwrap();
    assert_eq!(removed, 0);

    let all = store.read_aggregated().await.unwrap();
    assert_eq!(all.len(), 1);
}

#[tokio::test]
async fn test_append_multiple_same_day() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    for i in 0..5 {
        let agg = make_aggregated(&format!("hash-{}", i), "tool", i + 1);
        store.append_aggregated(&agg).await.unwrap();
    }

    let all = store.read_aggregated().await.unwrap();
    assert_eq!(all.len(), 5);
}

#[tokio::test]
async fn test_read_aggregated_merges_same_hash() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    // Same hash, different tool names - should be merged
    let agg1 = AggregatedExperience {
        pattern_hash: "shared-hash".into(),
        tool_name: "tool_a".into(),
        count: 10,
        avg_duration_ms: 100,
        success_rate: 0.8,
        last_seen: "2026-05-01T00:00:00Z".into(),
    };
    let agg2 = AggregatedExperience {
        pattern_hash: "shared-hash".into(),
        tool_name: "tool_b".into(),
        count: 5,
        avg_duration_ms: 200,
        success_rate: 0.6,
        last_seen: "2026-05-02T00:00:00Z".into(),
    };
    store.append_aggregated(&agg1).await.unwrap();
    store.append_aggregated(&agg2).await.unwrap();

    let all = store.read_aggregated().await.unwrap();
    // Same hash entries are stored separately (no merge on read)
    let shared: Vec<_> = all
        .iter()
        .filter(|a| a.pattern_hash == "shared-hash")
        .collect();
    assert_eq!(shared.len(), 2);
}

// --- Additional coverage tests ---

#[tokio::test]
async fn test_get_top_patterns_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    store
        .append_aggregated(&make_aggregated("h1", "tool_low", 2))
        .await
        .unwrap();
    store
        .append_aggregated(&make_aggregated("h2", "tool_mid", 5))
        .await
        .unwrap();
    store
        .append_aggregated(&make_aggregated("h3", "tool_high", 20))
        .await
        .unwrap();
    store
        .append_aggregated(&make_aggregated("h4", "tool_med2", 10))
        .await
        .unwrap();

    let top = store.get_top_patterns(3).await.unwrap();
    assert_eq!(top.len(), 3);
    // Should be sorted by count descending
    assert!(top[0].count >= top[1].count);
    assert!(top[1].count >= top[2].count);
}

#[tokio::test]
async fn test_get_stats_with_data() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    store
        .append_aggregated(&make_aggregated("h1", "a", 5))
        .await
        .unwrap();
    store
        .append_aggregated(&make_aggregated("h2", "b", 3))
        .await
        .unwrap();
    store
        .append_aggregated(&make_aggregated("h3", "c", 1))
        .await
        .unwrap();

    let (total, unique) = store.get_stats().await.unwrap();
    assert_eq!(total, 9); // 5 + 3 + 1
    assert!(unique >= 1);
}

#[tokio::test]
async fn test_read_aggregated_since_today() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    store
        .append_aggregated(&make_aggregated("h1", "tool", 5))
        .await
        .unwrap();

    let today = chrono::Local::now() - chrono::Duration::days(1);
    let result = store.read_aggregated_since(Some(today)).await.unwrap();
    assert_eq!(result.len(), 1);
}

#[tokio::test]
async fn test_clear_with_data() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());
    // Use raw format (append/read_all) since clear() only removes experiences.jsonl
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
async fn test_append_experience_deduplication() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::from_forge_dir(dir.path());

    let exp = make_experience("test_tool");
    let ce = CollectedExperience {
        experience: exp,
        dedup_hash: "same-hash".into(),
    };
    // First append should succeed
    store.append(&ce).await.unwrap();
    // Second with same hash should be deduplicated
    store.append(&ce).await.unwrap();

    let all = store.read_all().await.unwrap();
    // append() does not deduplicate — both are stored
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn test_file_newer_than_invalid_filename() {
    // "notadate" >= "20260512" is true lexicographically (lowercase > digits)
    assert!(ExperienceStore::file_newer_than(
        "notadate.jsonl",
        &chrono::Local::now()
    ));
}

// --- constructor / walk / cleanup edge coverage ---

#[tokio::test]
async fn test_with_config_ctor_applies_daily_limit() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExperienceStoreConfig {
        max_experiences_per_day: 1,
    };
    let store = ExperienceStore::with_config(dir.path().to_path_buf(), config);

    store
        .append_aggregated(&make_aggregated("h1", "tool_a", 1))
        .await
        .unwrap();
    // Second record on the same day is silently skipped by the limit.
    store
        .append_aggregated(&make_aggregated("h2", "tool_b", 1))
        .await
        .unwrap();

    let all = store.read_aggregated().await.unwrap();
    assert_eq!(all.len(), 1, "daily limit of 1 must cap the day's writes");
    assert_eq!(all[0].tool_name, "tool_a");
}

#[tokio::test]
async fn test_from_forge_dir_with_config_ctor() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExperienceStoreConfig {
        max_experiences_per_day: 1,
    };
    let store = ExperienceStore::from_forge_dir_with_config(dir.path(), config);

    store
        .append_aggregated(&make_aggregated("ha", "tool_x", 3))
        .await
        .unwrap();
    store
        .append_aggregated(&make_aggregated("hb", "tool_y", 4))
        .await
        .unwrap();

    // Base dir must be {forge_dir}/experiences.
    let exp_dir = dir.path().join("experiences");
    assert!(
        exp_dir.exists(),
        "from_forge_dir must add experiences subdir"
    );
    let all = store.read_aggregated().await.unwrap();
    assert_eq!(all.len(), 1, "custom daily limit must apply");
}

#[tokio::test]
async fn test_count_lines_in_file_read_error_returns_zero() {
    // A path that exists but is a directory: read_to_string fails -> 0 lines.
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::new(dir.path().to_path_buf());
    let as_dir = dir.path().join("not_a_file.jsonl");
    std::fs::create_dir_all(&as_dir).unwrap();

    let count = store.count_lines_in_file(&as_dir).await;
    assert_eq!(count, 0, "unreadable path must count as zero lines");
}

#[tokio::test]
async fn test_walk_jsonl_skips_stray_entries_and_blank_lines() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let month = base.join("202608");
    std::fs::create_dir_all(&month).unwrap();

    // Top-level stray file (not a YYYYMM dir) must be skipped.
    std::fs::write(base.join("stray.txt"), "junk").unwrap();
    // Non-.jsonl file inside the month dir must be skipped.
    std::fs::write(month.join("notes.txt"), "junk").unwrap();

    let valid = serde_json::to_string(&make_aggregated("hk", "keep_me", 2)).unwrap();
    let day_file = month.join("20260801.jsonl");
    std::fs::write(&day_file, format!("{}\n\nnot json at all\n", valid)).unwrap();

    let store = ExperienceStore::new(base.to_path_buf());
    let all = store.read_aggregated().await.unwrap();
    assert_eq!(all.len(), 1, "only the single valid record must parse");
    assert_eq!(all[0].tool_name, "keep_me");
    assert_eq!(all[0].count, 2);
}

#[tokio::test]
async fn test_walk_jsonl_files_wrapper_no_filter() {
    // The backward-compat wrapper behaves like read_aggregated with no filter.
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let month = base.join("202608");
    std::fs::create_dir_all(&month).unwrap();
    let valid = serde_json::to_string(&make_aggregated("hw", "wrap_tool", 5)).unwrap();
    std::fs::write(month.join("20260801.jsonl"), format!("{}\n", valid)).unwrap();

    let store = ExperienceStore::new(base.to_path_buf());
    let mut results = Vec::new();
    store.walk_jsonl_files(&mut results).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_name, "wrap_tool");
}

#[tokio::test]
async fn test_read_all_skips_blank_lines() {
    use crate::types::CollectedExperience;
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
    // Lines that parse as AggregatedExperience (legacy pollution) are dropped
    // during the flat-file rewrite even when young.
    use crate::types::CollectedExperience;
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    std::fs::create_dir_all(base).unwrap();

    let ce = CollectedExperience {
        experience: make_experience("young_tool"),
        dedup_hash: "hy".into(),
    };
    let legacy = serde_json::to_string(&make_aggregated("legacyhash", "agg_tool", 9)).unwrap();
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

// --- S8 coverage additions (quality-hardening goal 冲刺 S8) ---

#[tokio::test]
async fn test_s8_get_top_patterns_keeps_latest_last_seen() {
    let dir = tempfile::tempdir().unwrap();
    let store = ExperienceStore::new(dir.path());

    let mut r1 = make_aggregated("p1", "tool_a", 3);
    r1.last_seen = "2026-01-01T00:00:00Z".to_string();
    let mut r2 = make_aggregated("p1", "tool_a", 5);
    r2.last_seen = "2026-02-02T00:00:00Z".to_string();

    store.append_aggregated(&r1).await.unwrap();
    store.append_aggregated(&r2).await.unwrap();

    let top = store.get_top_patterns(10).await.unwrap();
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].count, 8);
    // Merged record must carry the LATER last_seen.
    assert_eq!(top[0].last_seen, "2026-02-02T00:00:00Z");
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
    let ce = crate::types::CollectedExperience {
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
    // open may create experiences.jsonl in the CWD — remove it afterwards so
    // the working tree stays clean either way.
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
    let ce = crate::types::CollectedExperience {
        experience: exp,
        dedup_hash: "sha256:x".into(),
    };
    let _ = store.append(&ce).await;
    let _ = tokio::fs::remove_file(std::path::Path::new("experiences.jsonl")).await;
}
