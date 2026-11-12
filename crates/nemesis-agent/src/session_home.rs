//! 会话资产区（session home）解析 —— 单一解析点（2026-11-12）。
//!
//! 每会话可绑定一个专属资产区 `agents/<home_agent_id>/home/`（分配点 =
//! sessions.create / fork copy-on-fork），plans/uploads/images/memory_vector
//! 按会话解析落自己的 home。绑定真相源 = chat_log sidecar meta 的
//! `home_agent_id`；None = 未绑定（存量会话 lenient，资产落全局旧位置，
//! 行为零变化）。
//!
//! 解析无缓存：meta 是小 JSON 文件（单读 <1ms），消息处理每轮 2~4 次调用
//! 远非瓶颈；不缓存换「fork 写 meta 后立即生效」的零失效成本。消费方 =
//! uploads 预取（admission/run_loop）、generate_image、plans 放行目录、
//! memory 选域、hydrate 回退查找——全部经本模块，禁止各自读 meta。

use std::path::{Path, PathBuf};

use crate::chat_log;

/// 会话绑定的资产区 agent id（None = 未绑定/会话不存在/meta 损坏）。
/// meta 侧车经 path 单例定位，无需 workspace 参数。
pub fn resolve_home_agent_id(session_key: &str) -> Option<String> {
    chat_log::read_session_meta_full(session_key)
        .and_then(|m| m.home_agent_id)
        .filter(|id| !id.is_empty())
}

/// 会话资产区 home 绝对路径（`<workspace>/agents/<id>/home`）。未绑定 =
/// None，调用方回落全局旧位置。
pub fn resolve_session_home(workspace: &Path, session_key: &str) -> Option<PathBuf> {
    let id = resolve_home_agent_id(session_key)?;
    Some(nemesis_path::resolve_agent_home_in_workspace(
        workspace, &id,
    ))
}

/// fork 产物独立记忆域（2026-11-12 拍板「只在分叉时分叉」）：meta 有 fork
/// 血缘（`parent` 在场）且 home 已绑定 → `Some(<home>/memory/memory_vector)`
/// ——分支间与对父会话记忆隔离，域随 home 树存在；普通会话 / 未绑定 /
/// 无血缘 = None（维持现状：对话组 → 主域、项目组 → 项目 agent 域，行为
/// 零变化）。装配侧经 `MemoryToolExecutor::with_session_domain_resolver`
/// 注入，运行时按 session_key 懒构造。
pub fn resolve_session_memory_domain(workspace: &Path, session_key: &str) -> Option<PathBuf> {
    let meta = chat_log::read_session_meta_full(session_key)?;
    meta.parent.as_ref().filter(|p| !p.trim().is_empty())?;
    let home = resolve_session_home(workspace, session_key)?;
    Some(home.join("memory").join("memory_vector"))
}

/// 为新会话分配资产区 id（uuid simple）+ 物化空 home 目录 + 落 meta。
/// 幂等：已有绑定 = 原样返回不覆盖（fork 已写 / 并发分配窗口的赢家优先）。
/// 返回生效的 id。失败（meta 写不进 / 目录建不了）= Err，调用方决定整体
/// 失败或 lenient（create 主流程建议 lenient——资产区是增量能力，不得
/// 绑架会话创建）。
pub fn assign_session_home(workspace: &Path, session_key: &str) -> Result<String, String> {
    if let Some(existing) = resolve_home_agent_id(session_key) {
        return Ok(existing);
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    chat_log::write_session_home_agent(session_key, &id);
    // 写后回读校验（meta 写失败静默会产出「绑定假成功」——资产落全局而
    // 消费方按 home 解析，读空目录比落全局更糟）。
    match resolve_home_agent_id(session_key) {
        Some(v) if v == id => {}
        _ => {
            return Err(format!(
                "session home 绑定写入失败（meta 回读不符）: {session_key}"
            ));
        }
    }
    // 目录物化（幂等 create_dir_all）：分配即物化——fork ghost 分支、
    // 消费方落盘、验收面都无需再特判「绑定在目录不在」。
    let home = nemesis_path::resolve_agent_home_in_workspace(workspace, &id);
    std::fs::create_dir_all(&home)
        .map_err(|e| format!("session home 目录创建失败（{home:?}）: {e}"))?;
    Ok(id)
}

/// 复制源会话资产区整树到新会话（fork copy-on-fork）。返回新绑定 id。
///
/// 契约（2026-11-12 计划）：**先复制树，成功后才写新会话 meta**——复制
/// 失败时新会话 meta 尚未写绑定、半残树已在函数内部 best-effort 清扫，
/// 调用方按 fork 整体失败回退即可（无半绑定会话、无孤儿资产树；仅血缘
/// meta 可能残留为孤儿——无 jsonl，sessions.list 不可见）。
///
/// 源未绑定 = lenient no-op（返回 None，新会话同样不绑定，行为对齐源）。
/// 源绑定在但目录缺席 = 新会话绑定全新空 home（继承绑定关系不继承资产）。
/// 目标已存在（极端并发 uuid 撞车）= 诚实 Err。
pub fn fork_copy_session_home(
    workspace: &Path,
    source_key: &str,
    new_key: &str,
) -> Result<Option<String>, String> {
    let Some(src_home) = resolve_session_home(workspace, source_key) else {
        return Ok(None);
    };
    if !src_home.is_dir() {
        // 绑定在但目录缺席（人工删除/迁移残留）= 无资产可复制，新会话
        // 绑定全新空 home（继承绑定关系但不继承资产——诚实语义）。
        let new_id = assign_session_home(workspace, new_key)?;
        return Ok(Some(new_id));
    }
    let new_id = uuid::Uuid::new_v4().simple().to_string();
    let dst_home = nemesis_path::resolve_agent_home_in_workspace(workspace, &new_id);
    if dst_home.exists() {
        return Err(format!(
            "fork home 目标目录已存在（uuid 撞车）: {dst_home:?}"
        ));
    }
    if let Err(e) = copy_dir_recursive(&src_home, &dst_home) {
        // 半残树清扫：复制失败 = 调用方按 fork 失败整体回退，dst 不留孤儿。
        cleanup_fork_dst(&dst_home);
        return Err(e);
    }
    chat_log::write_session_home_agent(new_key, &new_id);
    match resolve_home_agent_id(new_key) {
        Some(v) if v == new_id => Ok(Some(new_id)),
        _ => {
            // 绑定写失败/回读不符 = 新 home 无指向（不可信）→ 整树清扫，
            // 对齐复制失败臂的「无孤儿资产树」契约。
            cleanup_fork_dst(&dst_home);
            Err(format!(
                "fork home 绑定写入失败（meta 回读不符）: {new_key}"
            ))
        }
    }
}

/// fork 失败臂清扫：home 树韧性删除 + `agents/<id>/` 空壳父目录摘除。
/// create_dir_all(dst_home) 建了两层（父壳 + home），只删 home 层会留下
/// 空壳父目录（真场景压测实锚：独占锁打断复制后 `agents/<uuid>/` 滞留）。
/// 父壳摘除 = best-effort `remove_dir`（只删空目录）：非空（不该发生）或
/// Windows 瞬时句柄失败均静默跳过——空壳无绑定指向、sessions.list 不可见，
/// 残留无害。
fn cleanup_fork_dst(dst_home: &Path) {
    if !nemesis_utils::fsutil::remove_dir_all_resilient(dst_home) {
        tracing::warn!("[session_home] fork 失败后目标树清扫未净: {:?}", dst_home);
    }
    if let Some(parent) = dst_home.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

/// 递归复制目录树（文件 + 子目录，symlink 跳过——资产树内不应有且跨平台
/// 语义分裂）。错误带现场（源→目标路径），fork 回退方按 Err 处理。
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|e| format!("home 复制：创建目标目录 {dst:?} 失败: {e}"))?;
    let entries = std::fs::read_dir(src)
        .map_err(|e| format!("home 复制：读源目录 {src:?} 失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("home 复制：枚举源目录 {src:?} 失败: {e}"))?;
    // 有序消费 read_dir（跨平台顺序是实现细节，遍历序不影响复制正确性，
    // 但排序让错误现场可复现）。
    let mut entries = entries;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let target = dst.join(&name);
        let meta = entry
            .file_type()
            .map_err(|e| format!("home 复制：读 {path:?} 类型失败: {e}"))?;
        if meta.is_symlink() {
            continue;
        }
        if meta.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)
                .map_err(|e| format!("home 复制：{path:?} → {target:?} 失败: {e}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
