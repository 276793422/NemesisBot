//! Goals 存取层（注册表 + goal.md + evals.jsonl）。
//!
//! 容错语义（nemesis-loop 同族）：坏文件 load 时 Err 透传（调用方诚实
//! 处理）；evals.jsonl 单行损坏跳过不炸台账。注册表写入走 [`update_registry`]
//! ——进程级互斥锁内 load-modify-save（WSAPI 人操作与评估闸引擎并发写
//! 同一文件的串行点），原子写 tmp + rename 短退避重试。

use crate::state::{EvalRecord, GoalEntry};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// goals 目录名（`<workspace>/goals`，目标内容 + 评估台账落此）。
pub const GOALS_DIR_NAME: &str = "goals";
/// 注册表文件（`<workspace>/config/goals.json`）。
pub const GOALS_REGISTRY_FILE: &str = "goals.json";

/// 进程级注册表单写者锁。静态全局而非 per-workspace——临界区只有毫秒级
/// 文件读写，跨 workspace 串行无害；换来消费方零接线成本（不会漏锁）。
static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

pub fn goals_dir(workspace: &Path) -> PathBuf {
    workspace.join(GOALS_DIR_NAME)
}

pub fn registry_path(workspace: &Path) -> PathBuf {
    workspace.join("config").join(GOALS_REGISTRY_FILE)
}

fn goal_md_path_in(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.goal.md"))
}

/// goal.md 落盘路径（`goals/{id}.goal.md`）。
pub fn goal_md_path(workspace: &Path, id: &str) -> PathBuf {
    goal_md_path_in(&goals_dir(workspace), id)
}

fn evals_path_in(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.evals.jsonl"))
}

fn evals_path(workspace: &Path, id: &str) -> PathBuf {
    evals_path_in(&goals_dir(workspace), id)
}

/// 原子写（tmp + rename，掉电不留半文件；Windows 目标被占 rename 失败是
/// 常态——短退避重试后放弃）。
fn atomic_write(path: &Path, body: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create dir failed: {e}"))?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("open tmp failed: {e}"))?;
        f.write_all(body.as_bytes())
            .map_err(|e| format!("write tmp failed: {e}"))?;
        f.sync_all().ok();
    }
    let mut attempt = 0;
    loop {
        match std::fs::rename(&tmp, path) {
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

/// 读注册表（文件不存在 = 空表；损坏 = Err 透传诚实报错）。
pub fn load_registry(workspace: &Path) -> Result<Vec<GoalEntry>, String> {
    let path = registry_path(workspace);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let body = std::fs::read_to_string(&path).map_err(|e| format!("read registry failed: {e}"))?;
    serde_json::from_str(&body).map_err(|e| format!("parse registry failed: {e}"))
}

/// 写注册表（整表覆盖；调用方通常走 [`update_registry`] 而非直写）。
pub fn save_registry(workspace: &Path, entries: &[GoalEntry]) -> Result<(), String> {
    let body =
        serde_json::to_string_pretty(entries).map_err(|e| format!("serialize failed: {e}"))?;
    atomic_write(&registry_path(workspace), &body)
}

/// 注册表读-改-写（进程级互斥 + 原子落盘）。`f` 返回 Err 时整表放弃不落盘。
pub fn update_registry<R>(
    workspace: &Path,
    f: impl FnOnce(&mut Vec<GoalEntry>) -> Result<R, String>,
) -> Result<R, String> {
    let _guard = REGISTRY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut entries = load_registry(workspace)?;
    let out = f(&mut entries)?;
    save_registry(workspace, &entries)?;
    Ok(out)
}

/// goal.md 写入（评估的单一真相源；人建/agent 自建同落此）。
pub fn write_goal_md(workspace: &Path, id: &str, content: &str) -> Result<(), String> {
    atomic_write(&goal_md_path(workspace, id), content)
}

/// goal.md 读取（None = 文件缺失——注册表在而目标文件被手删 = 诚实缺料）。
pub fn read_goal_md(workspace: &Path, id: &str) -> Result<Option<String>, String> {
    let path = goal_md_path(workspace, id);
    if !path.exists() {
        return Ok(None);
    }
    std::fs::read_to_string(&path)
        .map(Some)
        .map_err(|e| format!("read goal.md failed: {e}"))
}

/// 追加一条评估记录（jsonl 逐行；文件不存在即建）。
pub fn append_eval(workspace: &Path, id: &str, rec: &EvalRecord) -> Result<(), String> {
    let dir = goals_dir(workspace);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create goals dir failed: {e}"))?;
    let line = serde_json::to_string(rec).map_err(|e| format!("serialize failed: {e}"))?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(evals_path_in(&dir, id))
        .map_err(|e| format!("open evals failed: {e}"))?;
    writeln!(f, "{line}").map_err(|e| format!("append eval failed: {e}"))
}

/// 读评估台账尾部最近 `n` 条（原序旧→新；单行损坏 warn + 跳过）。
pub fn read_evals_tail(workspace: &Path, id: &str, n: usize) -> Vec<EvalRecord> {
    let mut out = Vec::new();
    let Ok(body) = std::fs::read_to_string(evals_path(workspace, id)) else {
        return out;
    };
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<EvalRecord>(line) {
            Ok(rec) => out.push(rec),
            Err(e) => tracing::warn!("[Goals] skip corrupt eval line ({e})"),
        }
    }
    let start = out.len().saturating_sub(n);
    out.split_off(start)
}

/// 分配下一个 id（扫描 `G<n>.*` 文件**与注册表条目**取 max+1；id 单调
/// 不复用——done 后新建仍前进，台账/审计不留歧义）。并入注册表：手删
/// 文件/部分恢复造成的文件-注册表错位下，不与残留条目撞 id（撞 id 会让
/// 按 id 查找的更新/删除命中错行）。在 `update_registry` 闭包内调用安全：
/// `load_registry` 不取 `REGISTRY_LOCK`，无重入。
pub fn alloc_id(workspace: &Path) -> String {
    let mut max = 0u64;
    if let Ok(entries) = std::fs::read_dir(goals_dir(workspace)) {
        for e in entries.filter_map(|e| e.ok()) {
            let name = e.file_name();
            let name = name.to_string_lossy();
            let Some(stem) = name.strip_prefix('G') else {
                continue;
            };
            let Some(num) = stem.split(['.', '_']).next() else {
                continue;
            };
            if let Ok(n) = num.parse::<u64>() {
                max = max.max(n);
            }
        }
    }
    if let Ok(entries) = load_registry(workspace) {
        for e in entries {
            let Some(n) = e.id.strip_prefix('G').and_then(|s| s.parse::<u64>().ok()) else {
                continue;
            };
            max = max.max(n);
        }
    }
    format!("G{}", max + 1)
}

/// 删除 goal 三清：goal.md + evals.jsonl + 注册表行。文件已被手删时幂等
/// 跳过（best-effort）；注册表行不存在 = Err（调用方先校验在场）。
pub fn remove_goal(workspace: &Path, id: &str) -> Result<(), String> {
    let dir = goals_dir(workspace);
    update_registry(workspace, |entries| {
        let before = entries.len();
        entries.retain(|e| e.id != id);
        if entries.len() == before {
            return Err(format!("goal {id} not found in registry"));
        }
        Ok(())
    })?;
    for path in [goal_md_path_in(&dir, id), evals_path_in(&dir, id)] {
        if path.exists() {
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!("[Goals] remove {} failed: {e}", path.display());
            }
        }
    }
    Ok(())
}
