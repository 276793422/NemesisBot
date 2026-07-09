//! Pending-file enumeration + commit (box virtual FS → real disk) + box clear.
//!
//! The box virtual FS lives under `FileRootPath` (=`<home>/workspace/tools/sandboxie/box/NemesisBox`)
//! and mirrors real paths in two forms (confirmed `user/current` empirically;
//! `drive/<L>` is the standard Sandboxie layout for non-userprofile paths):
//!   <box_root>/user/<marker>/<rest>  ↔  %USERPROFILE%\<rest>
//!   <box_root>/drive/<L>/<rest>      ↔  <L>:\<rest>
//! Box metadata (RegHive*, DONT-USE.TXT, desktop.ini) is not under `user`/`drive`
//! so it's naturally excluded — only mirrored real files are returned.
//!
//! **SAFETY (containment backstop):** pending/commit are scoped to the WORKSPACE
//! subtree. Files the box wrote OUTSIDE the workspace (exec damage, system paths)
//! are never offered for commit — the user only ever sees their own workspace
//! writes, never the contained damage. See sandboxie-integration plan §3.5.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// A pending file: its path inside the box + the real path it would restore to.
#[derive(Debug, Clone)]
pub struct PendingFile {
    pub box_path: PathBuf,
    pub real_path: PathBuf,
    pub size: u64,
}

/// Map a box file path to its real (outside-box) path, or `None` if it isn't a
/// mirrored real path (box metadata, or an unmapped location).
pub fn real_path_for_box(box_path: &Path, box_root: &Path, user_profile: &Path) -> Option<PathBuf> {
    let rel = box_path.strip_prefix(box_root).ok()?;
    let mut comps = rel.components();
    let first = comps.next()?.as_os_str().to_string_lossy().to_string();
    match first.as_str() {
        "user" => {
            // user/<marker>/<rest>  →  %USERPROFILE%\<rest>  (marker is "current" / username)
            let _marker = comps.next()?;
            let rest: PathBuf = comps.collect();
            Some(user_profile.join(rest))
        }
        "drive" => {
            // drive/<L>/<rest>  →  <L>:\<rest>
            let letter = comps.next()?.as_os_str().to_string_lossy().to_string();
            let rest: PathBuf = comps.collect();
            Some(PathBuf::from(format!("{}:\\", letter)).join(rest))
        }
        _ => None, // RegHive*, DONT-USE.TXT, desktop.ini, anything else
    }
}

/// Walk the box virtual FS, returning every mirrored real file (no filter).
pub fn enumerate_box(box_root: &Path, user_profile: &Path) -> Result<Vec<PendingFile>> {
    let mut out = Vec::new();
    if !box_root.exists() {
        return Ok(out);
    }
    walk(box_root, box_root, user_profile, &mut out)?;
    Ok(out)
}

/// Maximum files to enumerate. Prevents slowness when the box has huge subtrees
/// (e.g. cargo build output, browser caches). The user can browse beyond this
/// via explorer (the 打开沙箱 button).
const MAX_BOX_FILES: usize = 5000;

fn walk(dir: &Path, box_root: &Path, user_profile: &Path, out: &mut Vec<PendingFile>) -> Result<()> {
    if out.len() >= MAX_BOX_FILES {
        return Ok(());
    }
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("read_dir {}", dir.display())),
    };
    for entry in rd {
        if out.len() >= MAX_BOX_FILES {
            return Ok(());
        }
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, box_root, user_profile, out)?;
        } else if let Some(real) = real_path_for_box(&path, box_root, user_profile) {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(PendingFile {
                box_path: path,
                real_path: real,
                size,
            });
        }
    }
    Ok(())
}

/// Pending files whose real path is under `workspace_real` (the safety-scoped set
/// the user is allowed to commit). `user_profile` is %USERPROFILE% (for the
/// user/ mapping).
///
/// NOTE: comparison is a raw case-sensitive `starts_with` on the constructed
/// real path — the real paths share the workspace's root + case (both derive
/// from the same system env), so this is correct for the common case. Symlink /
/// mixed-case edge cases are a refinement.
pub fn pending_workspace(
    box_root: &Path,
    workspace_real: &Path,
    user_profile: &Path,
) -> Result<Vec<PendingFile>> {
    let all = enumerate_box(box_root, user_profile)?;
    let mut filtered: Vec<PendingFile> = all
        .into_iter()
        .filter(|f| f.real_path.starts_with(workspace_real))
        .collect();
    filtered.sort_by(|a, b| a.real_path.cmp(&b.real_path));
    Ok(filtered)
}

/// Copy one pending file from the box to its real path (creates parent dirs,
/// overwrites). Returns the bytes copied.
pub fn commit_file(pending: &PendingFile) -> Result<u64> {
    if let Some(parent) = pending.real_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create real dir {}", parent.display()))?;
    }
    let n = std::fs::copy(&pending.box_path, &pending.real_path)
        .with_context(|| format!("commit {} -> {}", pending.box_path.display(), pending.real_path.display()))?;
    Ok(n)
}

/// Delete the box's virtual-FS contents (discard pending). Uses Sandboxie's own
/// `Start.exe /box:<name> delete_sandbox`. The box should have no running
/// processes (the per-call executor exits between calls).
pub fn delete_box_contents(start_exe: &Path, box_name: &str) -> Result<()> {
    let out = std::process::Command::new(start_exe)
        .arg(format!("/box:{box_name}"))
        .arg("delete_sandbox")
        .output()
        .with_context(|| format!("spawn Start.exe delete_sandbox ({})", start_exe.display()))?;
    if !out.status.success() {
        bail!(
            "delete_sandbox failed (status {}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}
