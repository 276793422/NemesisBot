use super::*;
use nemesis_types::forge::CycleStatus;

fn make_cycle(id: &str) -> LearningCycle {
    LearningCycle {
        id: id.into(),
        started_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        patterns_found: 2,
        actions_taken: 1,
        status: CycleStatus::Running,
    }
}

#[tokio::test]
async fn test_append_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());

    let cycle1 = make_cycle("cycle-001");
    let mut cycle2 = make_cycle("cycle-002");
    cycle2.status = CycleStatus::Completed;
    cycle2.completed_at = Some(chrono::Utc::now().to_rfc3339());

    store.append(&cycle1).await.unwrap();
    store.append(&cycle2).await.unwrap();

    let cycles = store.read_all().await.unwrap();
    assert_eq!(cycles.len(), 2);
    assert_eq!(cycles[0].id, "cycle-001");
    assert_eq!(cycles[1].id, "cycle-002");
}

#[tokio::test]
async fn test_append_uses_month_directory() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());

    let cycle = make_cycle("month-test");
    store.append(&cycle).await.unwrap();

    // Verify month directory structure was created
    let now = chrono::Utc::now();
    let month_dir = dir.path().join(now.format("%Y%m").to_string());
    assert!(month_dir.exists());

    let file_path = month_dir.join(format!("{}.jsonl", now.format("%Y%m%d")));
    assert!(file_path.exists());
}

#[tokio::test]
async fn test_get_latest() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());

    store.append(&make_cycle("first")).await.unwrap();
    store.append(&make_cycle("second")).await.unwrap();

    let latest = store.get_latest().await.unwrap().unwrap();
    assert_eq!(latest.id, "second");
}

#[tokio::test]
async fn test_load_latest_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());

    store.append(&make_cycle("a")).await.unwrap();
    store.append(&make_cycle("b")).await.unwrap();
    store.append(&make_cycle("c")).await.unwrap();

    let latest = store.load_latest_cycle().await.unwrap().unwrap();
    assert_eq!(latest.id, "c");
}

#[tokio::test]
async fn test_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());

    let cycles = store.read_all().await.unwrap();
    assert!(cycles.is_empty());

    let latest = store.get_latest().await.unwrap();
    assert!(latest.is_none());
}

#[tokio::test]
async fn test_read_cycles_with_since_filter() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());

    // Create an old file manually
    let old_date = chrono::Utc::now() - chrono::Duration::days(100);
    let old_month = dir.path().join(old_date.format("%Y%m").to_string());
    std::fs::create_dir_all(&old_month).unwrap();
    let old_cycle = LearningCycle {
        id: "old-cycle".into(),
        started_at: old_date.to_rfc3339(),
        completed_at: None,
        patterns_found: 1,
        actions_taken: 0,
        status: CycleStatus::Completed,
    };
    let old_file = old_month.join(format!("{}.jsonl", old_date.format("%Y%m%d")));
    std::fs::write(&old_file, format!("{}\n", serde_json::to_string(&old_cycle).unwrap())).unwrap();

    // Create a recent cycle via append
    store.append(&make_cycle("recent-cycle")).await.unwrap();

    // Read all
    let all = store.read_cycles(None).await.unwrap();
    assert_eq!(all.len(), 2);

    // Read since 7 days ago (should only get recent)
    let since = chrono::Utc::now() - chrono::Duration::days(7);
    let recent = store.read_cycles(Some(since)).await.unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].id, "recent-cycle");
}

#[tokio::test]
async fn test_cleanup_removes_old() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());

    // Create an old file manually
    let old_date = chrono::Utc::now() - chrono::Duration::days(100);
    let old_month = dir.path().join(old_date.format("%Y%m").to_string());
    std::fs::create_dir_all(&old_month).unwrap();
    let old_file = old_month.join(format!("{}.jsonl", old_date.format("%Y%m%d")));
    std::fs::write(&old_file, "test data\n").unwrap();

    // Create a recent file via append
    store.append(&make_cycle("keep-me")).await.unwrap();

    let removed = store.cleanup(30).await.unwrap();
    assert_eq!(removed, 1);
    assert!(!old_file.exists());

    // Recent file should still be readable
    let cycles = store.read_all().await.unwrap();
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0].id, "keep-me");
}

#[tokio::test]
async fn test_cleanup_no_dir() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path().join("nonexistent"));
    let removed = store.cleanup(30).await.unwrap();
    assert_eq!(removed, 0);
}

#[tokio::test]
async fn test_count() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());

    assert_eq!(store.count().await.unwrap(), 0);

    store.append(&make_cycle("c1")).await.unwrap();
    store.append(&make_cycle("c2")).await.unwrap();
    store.append(&make_cycle("c3")).await.unwrap();

    assert_eq!(store.count().await.unwrap(), 3);
}

#[tokio::test]
async fn test_new_adds_learning_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::new(dir.path());
    store.append(&make_cycle("test")).await.unwrap();

    // Should have created a "learning" subdirectory
    let learning_dir = dir.path().join("learning");
    assert!(learning_dir.exists());
}

#[tokio::test]
async fn test_read_nonexistent_dir() {
    let store = CycleStore::from_base("/nonexistent/path/for/testing");
    let cycles = store.read_all().await.unwrap();
    assert!(cycles.is_empty());
}

#[tokio::test]
async fn test_cleanup_keeps_recent() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());

    store.append(&make_cycle("recent")).await.unwrap();

    // Cleanup with 0 days should remove today's file
    // Actually, 0 days means cutoff is today, so today's file might still be kept
    // depending on exact timestamp. Use 1 day to be safe.
    let _removed = store.cleanup(1).await.unwrap();
    // Recent file should still be readable since it was just created
    let cycles = store.read_all().await.unwrap();
    assert_eq!(cycles.len(), 1);
}

#[tokio::test]
async fn test_read_cycles_ignores_non_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());

    // Create a non-jsonl file in the month directory
    let now = chrono::Utc::now();
    let month_dir = dir.path().join(now.format("%Y%m").to_string());
    std::fs::create_dir_all(&month_dir).unwrap();
    std::fs::write(month_dir.join("readme.txt"), "not a cycle").unwrap();

    // Should still work without errors
    let cycles = store.read_all().await.unwrap();
    assert!(cycles.is_empty());
}

#[tokio::test]
async fn test_read_cycles_ignores_malformed_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());

    // Create a valid cycle
    store.append(&make_cycle("valid")).await.unwrap();

    // Append malformed data to the same file
    let now = chrono::Utc::now();
    let month_dir = dir.path().join(now.format("%Y%m").to_string());
    let file_path = month_dir.join(format!("{}.jsonl", now.format("%Y%m%d")));
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(&file_path).unwrap();
    writeln!(f, "not valid json").unwrap();
    writeln!(f, "").unwrap();

    // Should still return the valid cycle
    let cycles = store.read_all().await.unwrap();
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0].id, "valid");
}

#[tokio::test]
async fn test_append_multiple_cycles_same_day() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());

    for i in 0..5 {
        store.append(&make_cycle(&format!("cycle-{}", i))).await.unwrap();
    }

    let cycles = store.read_all().await.unwrap();
    assert_eq!(cycles.len(), 5);
}

// --- Additional cycle_store tests ---

#[tokio::test]
async fn test_append_single_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());
    store.append(&make_cycle("single")).await.unwrap();
    let cycles = store.read_all().await.unwrap();
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0].id, "single");
}

#[tokio::test]
async fn test_append_completed_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());
    let mut cycle = make_cycle("completed");
    cycle.status = CycleStatus::Completed;
    cycle.completed_at = Some(chrono::Utc::now().to_rfc3339());
    cycle.patterns_found = 10;
    cycle.actions_taken = 3;
    store.append(&cycle).await.unwrap();
    let loaded = store.read_all().await.unwrap();
    assert_eq!(loaded[0].status, CycleStatus::Completed);
    assert_eq!(loaded[0].patterns_found, 10);
    assert_eq!(loaded[0].actions_taken, 3);
}

#[tokio::test]
async fn test_append_failed_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());
    let mut cycle = make_cycle("failed");
    cycle.status = CycleStatus::Failed;
    store.append(&cycle).await.unwrap();
    let loaded = store.read_all().await.unwrap();
    assert_eq!(loaded[0].status, CycleStatus::Failed);
}

#[tokio::test]
async fn test_read_cycles_since_now() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());
    store.append(&make_cycle("a")).await.unwrap();
    // Since=now should include just-created entries
    let since = chrono::Utc::now() - chrono::Duration::seconds(1);
    let cycles = store.read_cycles(Some(since)).await.unwrap();
    assert_eq!(cycles.len(), 1);
}

#[tokio::test]
async fn test_read_cycles_future_since() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());
    store.append(&make_cycle("a")).await.unwrap();
    // Since=future should return empty
    let future = chrono::Utc::now() + chrono::Duration::days(1);
    let cycles = store.read_cycles(Some(future)).await.unwrap();
    assert!(cycles.is_empty());
}

#[tokio::test]
async fn test_load_latest_cycle_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());
    let result = store.load_latest_cycle().await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_count_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());
    assert_eq!(store.count().await.unwrap(), 0);
}

#[tokio::test]
async fn test_count_after_multiple_appends() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());
    for i in 0..10 {
        store.append(&make_cycle(&format!("c-{}", i))).await.unwrap();
    }
    assert_eq!(store.count().await.unwrap(), 10);
}

#[tokio::test]
async fn test_cleanup_large_number_old_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());

    // Create multiple old files
    for i in 0..5 {
        let old_date = chrono::Utc::now() - chrono::Duration::days(100 + i);
        let old_month = dir.path().join(old_date.format("%Y%m").to_string());
        std::fs::create_dir_all(&old_month).unwrap();
        let old_file = old_month.join(format!("{}.jsonl", old_date.format("%Y%m%d")));
        std::fs::write(&old_file, "old data\n").unwrap();
    }

    let removed = store.cleanup(30).await.unwrap();
    assert_eq!(removed, 5);
}

#[tokio::test]
async fn test_cycle_json_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());
    let mut cycle = make_cycle("json-test");
    cycle.status = CycleStatus::Completed;
    cycle.completed_at = Some("2026-05-09T12:00:00Z".to_string());
    cycle.patterns_found = 42;
    cycle.actions_taken = 7;

    store.append(&cycle).await.unwrap();
    let loaded = store.read_all().await.unwrap();
    assert_eq!(loaded.len(), 1);
    let c = &loaded[0];
    assert_eq!(c.id, "json-test");
    assert_eq!(c.status, CycleStatus::Completed);
    assert_eq!(c.patterns_found, 42);
    assert_eq!(c.actions_taken, 7);
    assert_eq!(c.completed_at, Some("2026-05-09T12:00:00Z".to_string()));
}

#[tokio::test]
async fn test_from_base_creates_correct_path() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path().join("custom"));
    store.append(&make_cycle("test")).await.unwrap();
    // Should have created custom/ directory
    assert!(dir.path().join("custom").exists());
}

#[tokio::test]
async fn test_new_creates_learning_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::new(dir.path());
    store.append(&make_cycle("test")).await.unwrap();
    assert!(dir.path().join("learning").exists());
}

#[tokio::test]
async fn test_cleanup_does_not_remove_recent() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());
    for i in 0..3 {
        store.append(&make_cycle(&format!("keep-{}", i))).await.unwrap();
    }
    let removed = store.cleanup(1).await.unwrap();
    assert_eq!(removed, 0);
    let cycles = store.read_all().await.unwrap();
    assert_eq!(cycles.len(), 3);
}

#[tokio::test]
async fn test_read_cycles_preserves_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = CycleStore::from_base(dir.path());
    store.append(&make_cycle("first")).await.unwrap();
    store.append(&make_cycle("second")).await.unwrap();
    store.append(&make_cycle("third")).await.unwrap();
    let cycles = store.read_all().await.unwrap();
    assert_eq!(cycles[0].id, "first");
    assert_eq!(cycles[1].id, "second");
    assert_eq!(cycles[2].id, "third");
}
