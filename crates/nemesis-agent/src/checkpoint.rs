//! Checkpoint store — snapshot-based edit safety net.
//!
//! Before a writer tool (`write_file`/`edit_file`/`append_file`/`delete_file`)
//! changes a file, the agent records the file's pre-edit content here, keyed to
//! the current user turn. A rewind can then restore the workspace to an earlier
//! turn — restoring code, or (caller-side) the conversation, or both.
//!
//! Git-free: snapshots live beside the session under `{workspace}/.checkpoints/
//! <session>.ckpt/`, never touch the user's git, and work in a non-git dir. Only
//! edit-tool changes are tracked — `shell`/`async_shell` side effects are NOT
//! (their targets can't be known in advance). Persistence is one JSON file per
//! turn (cheap delete, corruption-isolated).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::r#loop::{FileChange, FileChangeKind};

/// One file's pre-edit state at the moment it was first touched in a turn.
/// `content == None` means the file did not exist then, so a restore deletes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnap {
    pub path: String,
    pub content: Option<String>,
}

/// Anchors the pre-edit state of every distinct file touched during one user turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub turn: usize,
    pub time: String, // RFC3339
    pub prompt: String,
    pub files: Vec<FileSnap>,
}

/// Picker-facing summary of a checkpoint (no file contents).
#[derive(Debug, Clone)]
pub struct CheckpointMeta {
    pub turn: usize,
    pub time: String,
    pub prompt: String,
    pub paths: Vec<String>,
}

struct Inner {
    done: Vec<Checkpoint>,
    cur: Option<Checkpoint>,
    seen: HashSet<String>, // paths already snapshotted in the current turn
}

/// Holds a session's checkpoints in memory and, when `dir` is set, persists one
/// JSON file per turn under it. All methods are safe for concurrent use.
pub struct CheckpointStore {
    dir: Option<PathBuf>,
    root: PathBuf,
    inner: Mutex<Inner>,
}

impl CheckpointStore {
    /// Create a store for the given checkpoint dir and workspace root, loading
    /// any checkpoints already persisted under `dir`. `dir = None` disables
    /// persistence (in-memory only for the session).
    pub fn new(dir: Option<PathBuf>, root: PathBuf) -> Self {
        let store = Self {
            dir,
            root,
            inner: Mutex::new(Inner {
                done: Vec::new(),
                cur: None,
                seen: HashSet::new(),
            }),
        };
        store.load();
        store
    }

    fn load(&self) {
        let Some(dir) = &self.dir else { return };
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut guard = self.inner.lock();
        for ent in entries.flatten() {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if let Ok(cp) = serde_json::from_slice::<Checkpoint>(&bytes) {
                guard.done.push(cp);
            }
        }
        guard.done.sort_by_key(|c| c.turn);
    }

    /// Open a checkpoint for a new user turn, finalizing the previous one. The
    /// prompt labels it in the picker.
    pub fn begin(&self, turn: usize, prompt: impl Into<String>) {
        let prompt = prompt.into();
        let cp = Checkpoint {
            turn,
            time: chrono::Local::now().to_rfc3339(),
            prompt,
            files: Vec::new(),
        };
        let mut guard = self.inner.lock();
        if let Some(cur) = guard.cur.take() {
            guard.done.push(cur);
        }
        guard.cur = Some(cp.clone());
        guard.seen.clear();
        drop(guard);
        self.persist(&cp);
    }

    /// Snapshot the pre-edit state of the file a writer is about to change.
    /// Async — reads the file's current content. Only the first touch of a path
    /// in the current turn is kept (its turn-start content). A no-op before the
    /// first `begin`, and for paths that escape the workspace root.
    pub async fn snapshot(&self, change: &FileChange) {
        if change.path.is_empty() {
            return;
        }
        // Read current content (resolved against root) BEFORE taking the lock.
        let content = match change.kind {
            FileChangeKind::Create => None, // did not exist (by definition)
            FileChangeKind::Modify | FileChangeKind::Delete => match self.safe_path(&change.path) {
                Some(abs) => tokio::fs::read_to_string(&abs).await.ok(),
                None => None,
            },
        };

        let mut guard = self.inner.lock();
        if guard.cur.is_none() {
            return;
        }
        if !guard.seen.insert(change.path.clone()) {
            return; // already snapshotted this turn
        }
        let cur = guard.cur.as_mut().expect("checked non-none above");
        cur.files.push(FileSnap {
            path: change.path.clone(),
            content,
        });
        let cp = cur.clone();
        drop(guard);
        self.persist(&cp);
    }

    /// Restore the workspace to its state at the start of turn `from_turn`:
    /// for every file touched at or after `from_turn`, write back the earliest
    /// recorded content (delete if the earliest snapshot was `None`). Returns
    /// (written, deleted) paths.
    pub async fn restore_code(&self, from_turn: usize) -> (Vec<String>, Vec<String>) {
        // Collect earliest snapshot per path across checkpoints >= from_turn.
        let earliest: Vec<(String, Option<String>)> = {
            let guard = self.inner.lock();
            let mut all: Vec<&Checkpoint> = guard.done.iter().collect();
            if let Some(cur) = guard.cur.as_ref() {
                all.push(cur);
            }
            all.sort_by_key(|c| c.turn);

            let mut out: Vec<(String, Option<String>)> = Vec::new();
            for c in all {
                if c.turn < from_turn {
                    continue;
                }
                for f in &c.files {
                    if out.iter().any(|(p, _)| p == &f.path) {
                        continue;
                    }
                    out.push((f.path.clone(), f.content.clone()));
                }
            }
            out
        };

        let mut written = Vec::new();
        let mut deleted = Vec::new();
        for (path, content) in earliest {
            let Some(abs) = self.safe_path(&path) else { continue };
            match content {
                None => {
                    if tokio::fs::remove_file(&abs).await.is_ok() {
                        deleted.push(path);
                    }
                }
                Some(body) => {
                    if let Some(parent) = abs.parent() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                    if tokio::fs::write(&abs, body).await.is_ok() {
                        written.push(path);
                    }
                }
            }
        }
        (written, deleted)
    }

    /// Metadata for all checkpoints (oldest turn first), for the rewind picker.
    pub fn list_meta(&self) -> Vec<CheckpointMeta> {
        let guard = self.inner.lock();
        let mut all: Vec<&Checkpoint> = guard.done.iter().collect();
        if let Some(cur) = guard.cur.as_ref() {
            all.push(cur);
        }
        all.sort_by_key(|c| c.turn);
        all.into_iter()
            .map(|c| CheckpointMeta {
                turn: c.turn,
                time: c.time.clone(),
                prompt: c.prompt.clone(),
                paths: c.files.iter().map(|f| f.path.clone()).collect(),
            })
            .collect()
    }

    /// Discard checkpoints at or after `from_turn` (a conversation rewind removes
    /// those future turns, so their snapshots must not remain/collide).
    pub fn truncate_from(&self, from_turn: usize) {
        {
            let mut guard = self.inner.lock();
            guard.done.retain(|c| c.turn < from_turn);
            if guard.cur.as_ref().map_or(false, |c| c.turn >= from_turn) {
                guard.cur = None;
                guard.seen.clear();
            }
        }
        if let Some(dir) = &self.dir {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for ent in entries.flatten() {
                    let name = ent.file_name().to_string_lossy().to_string();
                    if let Some(rest) = name
                        .strip_prefix("turn-")
                        .and_then(|s| s.strip_suffix(".json"))
                    {
                        if let Ok(t) = rest.parse::<usize>() {
                            if t >= from_turn {
                                let _ = std::fs::remove_file(ent.path());
                            }
                        }
                    }
                }
            }
        }
    }

    fn persist(&self, cp: &Checkpoint) {
        let Some(dir) = &self.dir else { return };
        let bytes = match serde_json::to_vec_pretty(cp) {
            Ok(b) => b,
            Err(_) => return,
        };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let path = dir.join(format!("turn-{}.json", cp.turn));
        let _ = std::fs::write(path, bytes);
    }

    /// Resolve `p` against the workspace root, rejecting traversal escapes.
    /// Restore must never write outside the workspace, even if a snapshot path is
    /// hostile or the project moved since the snapshot was taken.
    fn safe_path(&self, p: &str) -> Option<PathBuf> {
        if p.contains("..") {
            return None;
        }
        let abs = if Path::new(p).is_absolute() {
            PathBuf::from(p)
        } else {
            self.root.join(p)
        };
        Some(abs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn snapshot_modify(store: &CheckpointStore, rel: &str, body: &str) {
        tokio::fs::write(store.root.join(rel), body)
            .await
            .unwrap();
        let change = FileChange {
            path: rel.to_string(),
            kind: FileChangeKind::Modify,
        };
        store.snapshot(&change).await;
    }

    #[tokio::test]
    async fn restore_reverts_modified_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let store = CheckpointStore::new(None, root.clone());

        store.begin(0, "edit the file");
        snapshot_modify(&store, "a.txt", "original").await;
        // Simulate the tool modifying it.
        tokio::fs::write(root.join("a.txt"), "CHANGED")
            .await
            .unwrap();

        let (written, deleted) = store.restore_code(0).await;
        assert_eq!(written, vec!["a.txt".to_string()]);
        assert!(deleted.is_empty());
        let restored = tokio::fs::read_to_string(root.join("a.txt"))
            .await
            .unwrap();
        assert_eq!(restored, "original");
    }

    #[tokio::test]
    async fn restore_deletes_created_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let store = CheckpointStore::new(None, root.clone());

        store.begin(0, "create the file");
        // Create kind → snapshot records None (file did not exist).
        let change = FileChange {
            path: "new.txt".to_string(),
            kind: FileChangeKind::Create,
        };
        store.snapshot(&change).await;
        // Simulate the tool creating it.
        tokio::fs::write(root.join("new.txt"), "fresh")
            .await
            .unwrap();
        assert!(root.join("new.txt").exists());

        let (_, deleted) = store.restore_code(0).await;
        assert_eq!(deleted, vec!["new.txt".to_string()]);
        assert!(!root.join("new.txt").exists());
    }

    #[tokio::test]
    async fn per_turn_dedup_keeps_turn_start_content() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let store = CheckpointStore::new(None, root.clone());

        store.begin(0, "two edits same file");
        snapshot_modify(&store, "f.txt", "v1").await;
        // Second touch same turn — should NOT overwrite the v1 snapshot.
        let change2 = FileChange {
            path: "f.txt".to_string(),
            kind: FileChangeKind::Modify,
        };
        store.snapshot(&change2).await;
        tokio::fs::write(root.join("f.txt"), "v2-changed")
            .await
            .unwrap();

        store.restore_code(0).await;
        let restored = tokio::fs::read_to_string(root.join("f.txt"))
            .await
            .unwrap();
        assert_eq!(restored, "v1", "dedup should keep turn-start content");
    }

    #[tokio::test]
    async fn persistence_reloads_across_instances() {
        let dir = tempfile::tempdir().unwrap();
        let ckpt_dir = dir.path().join(".ck");
        let root = dir.path().to_path_buf();

        {
            let store = CheckpointStore::new(Some(ckpt_dir.clone()), root.clone());
            store.begin(0, "persisted turn");
            snapshot_modify(&store, "p.txt", "orig").await;
        }
        // New instance reloads the persisted checkpoint.
        let store2 = CheckpointStore::new(Some(ckpt_dir), root.clone());
        tokio::fs::write(root.join("p.txt"), "modified")
            .await
            .unwrap();
        let (written, _) = store2.restore_code(0).await;
        assert_eq!(written, vec!["p.txt".to_string()]);
        let restored = tokio::fs::read_to_string(root.join("p.txt"))
            .await
            .unwrap();
        assert_eq!(restored, "orig");
    }

    // ----- boundary /异常 tests -----

    #[tokio::test]
    async fn snapshot_before_begin_is_noop() {
        // Boundary: snapshot without begin (no active turn) must not panic.
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(None, dir.path().to_path_buf());
        let change = FileChange {
            path: "x.txt".into(),
            kind: FileChangeKind::Modify,
        };
        store.snapshot(&change).await; // must not panic
        assert!(store.list_meta().is_empty());
    }

    #[tokio::test]
    async fn restore_nonexistent_turn_returns_empty() {
        // Boundary: rewinding a turn with no checkpoints must return empty, not panic.
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(None, dir.path().to_path_buf());
        let (w, d) = store.restore_code(99).await;
        assert!(w.is_empty() && d.is_empty());
    }

    #[tokio::test]
    async fn path_escape_is_rejected_on_restore() {
        // Boundary: a snapshot path containing ".." must never be written/deleted
        // outside the workspace root on restore (safe_path returns None → skipped).
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let store = CheckpointStore::new(None, root.clone());
        store.begin(0, "evil");
        let change = FileChange {
            path: "../outside.txt".into(),
            kind: FileChangeKind::Delete,
        };
        store.snapshot(&change).await;
        let (w, d) = store.restore_code(0).await;
        assert!(w.is_empty(), "escape path must not be written: {:?}", w);
        assert!(d.is_empty(), "escape path must not be deleted: {:?}", d);
        // And nothing was created outside the workspace.
        assert!(!dir.path().parent().unwrap().join("outside.txt").exists());
    }

    #[tokio::test]
    async fn corrupted_persisted_checkpoint_is_skipped() {
        // Boundary: a malformed turn-N.json must not break loading of the others.
        let dir = tempfile::tempdir().unwrap();
        let ckpt_dir = dir.path().join(".ck");
        std::fs::create_dir_all(&ckpt_dir).unwrap();
        std::fs::write(ckpt_dir.join("turn-0.json"), b"NOT VALID JSON {{{").unwrap();
        let good = serde_json::to_vec(&Checkpoint {
            turn: 1,
            time: "t".into(),
            prompt: "good".into(),
            files: vec![],
        })
        .unwrap();
        std::fs::write(ckpt_dir.join("turn-1.json"), good).unwrap();

        let store = CheckpointStore::new(Some(ckpt_dir), dir.path().to_path_buf());
        let metas = store.list_meta();
        assert_eq!(
            metas.len(),
            1,
            "corrupted turn-0 must be skipped, turn-1 kept"
        );
        assert_eq!(metas[0].turn, 1);
    }

    #[tokio::test]
    async fn empty_path_snapshot_is_ignored() {
        // Boundary: empty path must be ignored (no panic, no snapshot).
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(None, dir.path().to_path_buf());
        store.begin(0, "empty");
        let change = FileChange {
            path: String::new(),
            kind: FileChangeKind::Modify,
        };
        store.snapshot(&change).await;
        let meta = store.list_meta();
        assert_eq!(meta.len(), 1);
        assert!(meta[0].paths.is_empty(), "empty path must not be snapshotted");
    }
}
