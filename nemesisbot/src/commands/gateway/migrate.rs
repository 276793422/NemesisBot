// ---------------------------------------------------------------------------
// Gateway command
// ---------------------------------------------------------------------------

// 唯一消费者 migrate_legacy_workflow_dir 为 workflow 门——随门收放。
#[cfg(feature = "workflow")]
use tracing::{info, warn};

/// One-shot migration: move pre-refactor workflow files from
/// `{home}/workflow/` (the legacy flat layout) into the new four-subdir
/// layout under `{home}/workspace/workflow/`.
///
/// Legacy layout (pre-refactor):
///   {home}/workflow/{wf}_{exec}.jsonl         -> executions/
///   {home}/workflow/checkpoints/{exec}/{cp}.json -> checkpoints/
///
/// New layout (post-refactor):
///   {home}/workspace/workflow/executions/{wf}_{exec}.jsonl
///   {home}/workspace/workflow/checkpoints/{exec}/{cp}.json
///
/// Runs only if the legacy dir exists. Skips files that already exist at
/// the destination (idempotent across re-runs). Removes the legacy dir if
/// it ends up empty. Errors are logged at warn level — gateway startup
/// proceeds regardless, since stale data shouldn't block the service.
#[cfg(feature = "workflow")]
pub(crate) fn migrate_legacy_workflow_dir(
    home: &std::path::Path,
    new_executions_dir: &std::path::Path,
    new_checkpoints_dir: &std::path::Path,
) {
    let legacy_root = home.join("workflow");
    if !legacy_root.exists() {
        return;
    }
    info!(
        "[Gateway] Migrating legacy workflow dir: {} -> {}",
        legacy_root.display(),
        new_executions_dir
            .parent()
            .unwrap_or(new_executions_dir)
            .display()
    );

    // Move *.jsonl execution logs.
    if let Ok(entries) = std::fs::read_dir(&legacy_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|ext| ext == "jsonl")
                    .unwrap_or(false)
            {
                let dest = new_executions_dir.join(path.file_name().unwrap_or_default());
                if dest.exists() {
                    continue;
                }
                if let Err(e) = std::fs::rename(&path, &dest) {
                    warn!(
                        "[Gateway] migration: failed to move {}: {}",
                        path.display(),
                        e
                    );
                }
            }
        }
    }

    // Move checkpoints/ subdir contents.
    let legacy_checkpoints = legacy_root.join("checkpoints");
    if legacy_checkpoints.exists()
        && let Ok(entries) = std::fs::read_dir(&legacy_checkpoints)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let dest = new_checkpoints_dir.join(&name);
            if dest.exists() {
                continue;
            }
            if let Err(e) = std::fs::rename(&path, &dest) {
                warn!(
                    "[Gateway] migration: failed to move checkpoint {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }

    // Best-effort cleanup: remove legacy dir if empty (ignoring the
    // now-empty checkpoints/ subdir). Don't touch non-empty dir — user may
    // have files we don't recognise.
    let _ = std::fs::remove_dir(legacy_root.join("checkpoints"));
    if std::fs::read_dir(&legacy_root)
        .map(|mut e| e.next().is_none())
        .unwrap_or(false)
    {
        let _ = std::fs::remove_dir(&legacy_root);
        info!("[Gateway] Migration complete; removed empty legacy workflow dir");
    } else {
        warn!(
            "[Gateway] Migration partial: legacy dir {} not empty, left in place",
            legacy_root.display()
        );
    }
}
