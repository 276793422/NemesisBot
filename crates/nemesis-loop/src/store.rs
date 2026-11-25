//! Loop 状态文件存取（`<workspace>/loops/{id}.json`，原子写）。
//!
//! 容错语义与 events.json 同族：坏文件（解析失败）list 时 warn + 跳过
//! 不炸整表；load 单文件 Err 透传（调用方诚实处理）。

use crate::state::LoopState;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// loops 状态目录（`<workspace>/loops`）。
pub fn loops_dir(workspace: &Path) -> PathBuf {
    workspace.join("loops")
}

/// loop id 路径安全校验（BUG-20261121-032）：WSAPI/CLI 的 id 是外部输入，
/// 含 `..`、`\`、盘符形态（`c:foo` 在 join 语义下整段换基）时 load/save/
/// remove 可达 loops 目录外（remove 面 = 删任意 .json 文件；load 面 =
/// 目录外 JSON 存在性/可解析性探针）。合法 id 由 [`alloc_id`] 分配为
/// `L<digits>`；手放状态文件名限同字符集 `[a-zA-Z0-9_-]{1,64}`（点也拒
/// ——文件名形态恒为 `{id}.json`，id 本身无需带点）。
fn validate_loop_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err(format!("loop id `{id}` 长度非法（1..=64）"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "loop id `{id}` 含非法字符（只允许字母/数字/-/_——拒绝路径分隔符与越界形态）"
        ));
    }
    Ok(())
}

fn loop_path(dir: &Path, id: &str) -> Result<PathBuf, String> {
    validate_loop_id(id)?;
    Ok(dir.join(format!("{id}.json")))
}

/// 原子写单条 loop 状态——委托全仓唯一权威 helper
/// [`nemesis_utils::write_file_atomic`]（BUG-20261118-152 横向同族：此前
/// 自制 tmp+rename 用**固定临时名** `.{id}.tmp`——同 id 并发 save（项目
/// loop 与主 loop 并发 / 多实例部署跨进程）在同名 tmp 上交错，先完成者
/// 的 rename 可把另一方写一半的半截 JSON 提交成正式状态文件（goals
/// store 同款复制粘贴源，同轮同修）。helper 唯一临时名 + create_new +
/// sync_all 先于 rename + 失败清理结构性消除该家族；Windows 目标被占
/// rename 失败由调用方诚实上报，不再自研重试。unix 0o644（workspace
/// 资产）。
pub fn save_loop(dir: &Path, st: &LoopState) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create loops dir failed: {e}"))?;
    // st.id 同闸（validate_loop_id）：状态文件内容里的 id 也可能是手放/
    // 伪造的越界形态（update 命令 load 后原样 save 即写回该路径）。
    let path = loop_path(dir, &st.id)?;
    let body = serde_json::to_string_pretty(st).map_err(|e| format!("serialize failed: {e}"))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), body.as_bytes(), 0o644)
}

/// 读单条（None = 文件不存在）。
pub fn load_loop(dir: &Path, id: &str) -> Result<Option<LoopState>, String> {
    let path = loop_path(dir, id)?;
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

/// 删除状态文件（返回是否确实存在）。进程锁内执行（BUG-20261125-003）：
/// 与 [`update_loop`] 的 load→save 串行——否则并发交错下 remove 删掉文件后
/// update 的 save 又把它写回（已删 loop 复活）。
pub fn remove_loop(dir: &Path, id: &str) -> Result<bool, String> {
    let _guard = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let path = loop_path(dir, id)?;
    if !path.exists() {
        return Ok(false);
    }
    // BUG-20261123-018：状态文件刚被 save 重写的概率高（AV/索引器瞬时句柄
    // Windows 常态），删除走 fsutil 退避重试，耗尽才报错。
    nemesis_utils::fsutil::remove_file_retry(&path)
        .map_err(|e| format!("remove {} failed: {e}", path.display()))?;
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

/// 进程级状态互斥锁（BUG-20261125-003）：全部「读→改→写」状态文件的操作
/// （create 的 alloc+save、update_loop 的 load+mutate+save、remove 的
/// 删除）在本锁内串行。缺本锁时并发写者（gateway 驱动器每 10s sweep、
/// WSAPI/CLI pause·resume·update、agent 三工具 wakeup·finish）各自持快照
/// 交错落盘 = 丢失更新：人 pause 的 loop 被驱动器旧快照写回静默复活继续
/// 消耗 token、agent finish 置 done 被写回 active 再被唤醒、预算改动被
/// 覆盖——同族前案 BUG-20261118-152（原子写临时名互踩）与
/// BUG-20261119-186（并发创建同 id 互覆）修了写原子性与分配串行化，本锁
/// 补上第三漏网面（RMW 丢失更新）。goals store 的 REGISTRY_LOCK +
/// update_registry 闭包纪律是同款先例。跨 workspace 串行无害（临界区
/// 毫秒级文件读写）；跨进程（CLI vs gateway）不在本锁射程——与 create
/// 同款诚实边界（进程级锁换消费方零接线成本，文件锁属更大改动）。
static STATE_LOCK: Mutex<()> = Mutex::new(());

/// 创建新 loop（锁内 alloc+save 串行化，防并发创建同 id 互覆）。`build`
/// 在锁内以已分配 id 构造状态；返回 Err = 放弃不落盘（id 作废——id 单调
/// 语义下作废号留空档无害，下次分配跳过）。
pub fn create_loop(
    dir: &Path,
    build: impl FnOnce(String) -> Result<LoopState, String>,
) -> Result<LoopState, String> {
    let _guard = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let id = alloc_id(dir);
    let st = build(id.clone())?;
    save_loop(dir, &st)?;
    Ok(st)
}

/// 锁内 load→mutate→save（RMW 串行化，BUG-20261125-003）。`f` 在锁内以
/// **盘上最新**状态裁决并原地修改；返回 `Ok(true)` = 已改动，写盘落盘；
/// `Ok(false)` = 无需落盘（无改动/幂等命中，省写——驱动器 sweep 未命中
/// 时不产生每 10s 无意义重写）；`Err` = 放弃不写盘（状态机校验失败等，
/// 校验必须用锁内状态重做——快照预检只作提前失败优化）。不存在 = Err。
/// 返回锁内终态（调用方按返回值生成消息，勿用预检快照）。
pub fn update_loop(
    dir: &Path,
    id: &str,
    f: impl FnOnce(&mut LoopState) -> Result<bool, String>,
) -> Result<LoopState, String> {
    let _guard = STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(mut st) = load_loop(dir, id)? else {
        return Err(format!("loop {id} 不存在"));
    };
    if f(&mut st)? {
        save_loop(dir, &st)?;
    }
    Ok(st)
}
