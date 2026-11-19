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

/// 受控形态校验（BUG-20261112-116）：绑定 id 只接受分配点的生成形态——
/// uuid simple（32 位小写 hex）。meta 是 workspace 内文件（LLM 经
/// write_file 可触达），无校验的 id 直接拼 `<workspace>/agents/<id>/home`
/// 会把 meta 污染升级成路径越界：`..`/分隔符注入的 id 让 plans 写放行
/// 锚点（`plan_products_dir_for`）、uploads/images 落盘、memory 域全部
/// 同源越界——后三者是内部 fs 写路径不经 WorkspaceBoundary 兜底。
/// nemesis-path 侧「agent_id 必须是受控生成的 uuid」的调用方契约在本
/// 解析点落地为代码强制：形态不符 = None（lenient 降级，资产落全局旧
/// 位置）——与「未绑定」同路径，零新增失败面；后续 `assign_session_home`
/// 幂等分支走同一校验，污染值会被下次分配自然覆盖自愈。
fn is_controlled_home_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// 会话绑定的资产区 agent id（None = 未绑定/会话不存在/meta 损坏/形态
/// 不受控）。meta 侧车经 path 单例定位，无需 workspace 参数。
pub fn resolve_home_agent_id(session_key: &str) -> Option<String> {
    chat_log::read_session_meta_full(session_key)
        .and_then(|m| m.home_agent_id)
        .filter(|id| !id.is_empty() && is_controlled_home_id(id))
}

/// 会话资产区 home 绝对路径（`<workspace>/agents/<id>/home`）。未绑定 =
/// None，调用方回落全局旧位置。
pub fn resolve_session_home(workspace: &Path, session_key: &str) -> Option<PathBuf> {
    let id = resolve_home_agent_id(session_key)?;
    Some(nemesis_path::resolve_agent_home_in_workspace(
        workspace, &id,
    ))
}

/// 存量项目会话资产树惰性迁移（2026-11-16 污染修复配套）。
///
/// 背景：修复前的分配点把项目会话的 `agents/<id>/home` 锚在**项目目录**
/// （用户的真实工作树）——修复后根恒主 workspace，但设备上已长出的旧树
/// 不会自己搬家。本函数把「主 workspace 树缺席 + meta 带 project_path +
/// 项目侧旧树在场」的会话整树搬回主 workspace。
///
/// 搬移策略：同卷 rename 优先（原子、瞬时）；失败（跨卷 / Windows 句柄
/// 占用）回落 copy + 韧性删——copy 臂先复核主侧目标仍缺席（并发窗口里
/// 另一调用者可能已完成迁移，此时放弃且**绝不清扫主侧**——那是赢家刚
/// 搬好的树）；copy 半残失败才清主侧残迹。搬后 best-effort 摘除项目侧
/// 空壳父目录（`remove_dir` 只删空目录，用户自己的 `agents/` 内容永不
/// 触碰）。幂等：主树已在 / 项目侧缺席 / meta 无项目路径 / 未绑定 =
/// no-op（稳态调用成本 = 一次 meta 读 + 一次 is_dir）。失败 = lenient
/// （WARN 不炸调用方——迁移失败会话降级为新空 home 继续用，旧树残留
/// 项目侧可人工搬）。
pub fn migrate_stale_project_session_home(workspace: &Path, session_key: &str) {
    let Some(id) = resolve_home_agent_id(session_key) else {
        return;
    };
    let main_home = nemesis_path::resolve_agent_home_in_workspace(workspace, &id);
    if main_home.is_dir() {
        return; // 已在正确位置（或已迁移过）
    }
    let Some(project_path) = chat_log::read_session_meta_full(session_key)
        .and_then(|m| m.project_path)
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
    else {
        return;
    };
    let legacy_home = nemesis_path::resolve_agent_home_in_workspace(Path::new(&project_path), &id);
    if !legacy_home.is_dir() {
        return; // 项目侧无旧树（未受污染的会话 / project_path 已失效）
    }
    // 防呆：project_path == 主 workspace 时 legacy_home == main_home，
    // 上方 is_dir 已拦截，走不到这——不会出现「自己搬给自己」。
    if let Some(parent) = main_home.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!("[session_home] 迁移中止：父目录创建失败 {parent:?}: {e}");
        return;
    }
    if std::fs::rename(&legacy_home, &main_home).is_err() {
        // rename 失败（跨卷 / 源树有打开句柄 / 并发竞争）。复核并发窗口：
        // 目标已在 = 另一调用者迁移完成，诚实放弃（不碰主侧）。
        if main_home.is_dir() {
            return;
        }
        if copy_dir_recursive(&legacy_home, &main_home).is_err() {
            // copy 半残：主侧目录必是本次 copy 臂所建（上方复核保证），
            // 清残迹避免半截树被误认为已迁移。
            let _ = nemesis_utils::fsutil::remove_dir_all_resilient(&main_home);
            tracing::warn!(
                "[session_home] 存量资产树迁移失败（copy 臂），会话继续用新空 home: \
                 {legacy_home:?} → {main_home:?}"
            );
            return;
        }
        if !nemesis_utils::fsutil::remove_dir_all_resilient(&legacy_home) {
            tracing::warn!("[session_home] 迁移 copy 完成但项目侧旧树删除未净: {legacy_home:?}");
        }
    }
    // 项目侧空壳摘除（best-effort，只删空目录；Windows 瞬时句柄静默跳过）。
    // `<project>/agents/<id>` → `<project>/agents` 两层都试——修复版 bug
    // 会话在项目目录留下完整 `agents/` 骨架，搬空后应归位。
    if let Some(shell) = legacy_home.parent() {
        let _ = std::fs::remove_dir(shell);
        if let Some(grand) = shell.parent() {
            let _ = std::fs::remove_dir(grand);
        }
    }
    tracing::info!("[session_home] 存量项目资产树已迁回主 workspace: {id}");
}

/// 存量项目会话 todo 文件惰性迁移（2026-11-17 污染修复配套）。
///
/// 背景：2026-11-16 资产区修复漏网——todo 文件（bot 域会话态，锚
/// `{主 ws}/sessions/todo_{safe}.json`）的写侧（TodoToolConfig /
/// TodoCloseoutHook）此前被误判「合法项目根消费者」，项目会话的 todo 一直
/// 落 `{项目目录}/sessions/`；而删侧（回收站 / sessions 删除清空 /
/// logs_sweeper 孤儿清扫）全按主 workspace 寻址——删项目会话留孤儿文件在
/// 用户项目目录。修复后写侧改锚主 workspace，本函数把设备上已长出的旧
/// 文件搬回。
///
/// 搬移策略：同卷 rename 优先；失败（跨卷 / 句柄占用 / 并发）复核主侧
/// 目标仍缺席后 copy + 删源（单文件，无需递归）。**主侧已在 = 主侧胜**：
/// 修复后的写入恒落主侧，项目侧文件必是修复前旧态，best-effort 删除
/// （顺带清用户仓库里的污染残留；删除失败 WARN 不炸）。幂等：主侧已在
/// （无项目侧旧文件）/ 项目侧缺席 / meta 无项目路径 = no-op（稳态成本 =
/// 一次 meta 读 + 两次 exists）。失败 = lenient（WARN 不炸调用方——迁移
/// 失败会话降级为新空 todo 继续用，旧文件残留项目侧可人工搬）。
pub fn migrate_stale_project_session_todo(workspace: &Path, session_key: &str) {
    let Some(project_path) = chat_log::read_session_meta_full(session_key)
        .and_then(|m| m.project_path)
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
    else {
        return;
    };
    let main_todo = nemesis_path::session_todo_file_in_workspace(workspace, session_key);
    let legacy_todo =
        nemesis_path::session_todo_file_in_workspace(Path::new(&project_path), session_key);
    if main_todo.exists() {
        // 主侧已在（修复后写入落主侧）：项目侧若有旧文件必是修复前旧态，
        // 顺手清掉（用户仓库不容残留）；失败 WARN 留待人工。
        if legacy_todo.exists() {
            if let Err(e) = std::fs::remove_file(&legacy_todo) {
                tracing::warn!(
                    "[session_home] 项目侧旧 todo 清理失败（主侧已生效，不影响功能）: \
                     {legacy_todo:?}: {e}"
                );
            } else {
                remove_project_sessions_shell(&legacy_todo);
            }
        }
        return;
    }
    if !legacy_todo.exists() {
        return; // 项目侧无旧文件（未受污染的会话 / project_path 已失效 / 已迁移过）
    }
    // 防呆：project_path == 主 workspace 时 legacy_todo == main_todo，
    // 上方两个 exists 分支已拦截，走不到这——不会「自己删自己」。
    if let Some(parent) = main_todo.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!("[session_home] todo 迁移中止：父目录创建失败 {parent:?}: {e}");
        return;
    }
    if std::fs::rename(&legacy_todo, &main_todo).is_err() {
        // rename 失败：复核并发窗口（目标已在 = 另一调用者完成，诚实放弃）。
        if main_todo.exists() {
            return;
        }
        if std::fs::copy(&legacy_todo, &main_todo).is_err() {
            tracing::warn!(
                "[session_home] 存量 todo 迁移失败，会话降级为新空 todo 继续: \
                 {legacy_todo:?} → {main_todo:?}"
            );
            return;
        }
        if let Err(e) = std::fs::remove_file(&legacy_todo) {
            tracing::warn!(
                "[session_home] todo 迁移 copy 完成但项目侧旧文件删除未净: {legacy_todo:?}: {e}"
            );
        }
    }
    remove_project_sessions_shell(&legacy_todo);
    tracing::info!("[session_home] 存量项目 todo 已迁回主 workspace: {session_key}");
}

/// 项目侧空壳摘除（best-effort `remove_dir` 只删空目录）：todo 搬走后
/// `{project}/sessions/` 若已被搬空则归位；用户自己的内容永不触碰
/// （非空 / Windows 瞬时句柄失败均静默跳过）。
fn remove_project_sessions_shell(legacy_todo: &Path) {
    if let Some(sessions_dir) = legacy_todo.parent() {
        let _ = std::fs::remove_dir(sessions_dir);
    }
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
        // 幂等分支顺带惰性迁移（2026-11-16）：绑定在但主侧树缺席时，
        // 存量项目树搬回主 workspace——消息路径（admission/binding）与
        // create 重入都经这里，无需各自感知迁移。（2026-11-17 加）todo
        // 文件同族迁移——写侧改锚主 ws 后旧文件搬回，挂同一消息路径。
        migrate_stale_project_session_home(workspace, session_key);
        migrate_stale_project_session_todo(workspace, session_key);
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
    // 源会话可能还是存量项目树形态（升级后未经 sessions.list/消息路径
    // 触发迁移）——先迁移再解析，fork 复制才不会因「主侧缺席」把源资产
    // 误判成空 home。
    migrate_stale_project_session_home(workspace, source_key);
    migrate_stale_project_session_todo(workspace, source_key);
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
