//! Loop 状态文件存取（`<workspace>/loops/{id}.json`，原子写）。
//!
//! 容错语义与 events.json 同族：坏文件（解析失败）list 时 warn + 跳过
//! 不炸整表；load 单文件 Err 透传（调用方诚实处理）。

use crate::state::LoopState;
use std::io::Write;
use std::path::{Path, PathBuf};

/// loops 状态目录（`<workspace>/loops`）。
pub fn loops_dir(workspace: &Path) -> PathBuf {
    workspace.join("loops")
}

fn loop_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// 原子写单条 loop 状态（tmp + rename，掉电不留半文件）。
pub fn save_loop(dir: &Path, st: &LoopState) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create loops dir failed: {e}"))?;
    let path = loop_path(dir, &st.id);
    let tmp = dir.join(format!(".{}.tmp", st.id));
    let body = serde_json::to_string_pretty(st).map_err(|e| format!("serialize failed: {e}"))?;
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("open tmp failed: {e}"))?;
        f.write_all(body.as_bytes())
            .map_err(|e| format!("write tmp failed: {e}"))?;
        f.sync_all().ok();
    }
    // Windows 上目标被占 rename 失败是常态（AV/索引器瞬时句柄；std rename
    // 带 REPLACE_EXISTING 语义）——短退避重试后放弃并报错（轻量内联版，
    // 单文件 rename 不引 fsutil）。
    let mut attempt = 0;
    loop {
        match std::fs::rename(&tmp, &path) {
            Ok(()) => return Ok(()),
            Err(_e) if attempt < 5 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(20 * attempt));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!("rename failed after {attempt} attempts: {e}"));
            }
        }
    }
}

/// 读单条（None = 文件不存在）。
pub fn load_loop(dir: &Path, id: &str) -> Result<Option<LoopState>, String> {
    let path = loop_path(dir, id);
    if !path.exists() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(&path)
        .map_err(|e| format!("read {} failed: {e}", path.display()))?;
    serde_json::from_str(&body)
        .map(Some)
        .map_err(|e| format!("parse {} failed: {e}", path.display()))
}

/// 列全部（按 id 字典序；坏文件 warn + 跳过——单条损坏不炸整表）。
pub fn list_loops(dir: &Path) -> Vec<LoopState> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    // 显式排序：read_dir 顺序是平台实现细节（NTFS 字典序 vs ext4 哈希序）。
    paths.sort();
    for p in paths {
        match std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_str::<LoopState>(&b).map_err(|e| e.to_string()))
        {
            Ok(st) => out.push(st),
            Err(e) => {
                tracing::warn!("[Loop] skip corrupt state file {}: {e}", p.display());
            }
        }
    }
    out
}

/// 删除状态文件（返回是否确实存在）。
pub fn remove_loop(dir: &Path, id: &str) -> Result<bool, String> {
    let path = loop_path(dir, id);
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path).map_err(|e| format!("remove {} failed: {e}", path.display()))?;
    Ok(true)
}

/// 分配下一个 id（扫描 `L<n>.json` 取 max+1；空目录 = L1）。文本复用
/// （如 L2 删除后新建仍取 max+1=L3）——id 单调不复用，台账/审计不留歧义。
pub fn alloc_id(dir: &Path) -> String {
    let mut max = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.filter_map(|e| e.ok()) {
            let name = e.file_name();
            let name = name.to_string_lossy();
            let Some(stem) = name.strip_suffix(".json") else {
                continue;
            };
            let Some(num) = stem.strip_prefix('L') else {
                continue;
            };
            if let Ok(n) = num.parse::<u64>() {
                max = max.max(n);
            }
        }
    }
    format!("L{}", max + 1)
}
