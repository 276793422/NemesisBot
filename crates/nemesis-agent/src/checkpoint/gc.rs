//! M6（2026-10-27 整改批次五）：checkpoint gc —— 影子库/JSON 快照保留集
//! 清扫。
//!
//! **默认关**（`config.json` `checkpoint.gc.enabled=false`；删错 = 回退网
//! 失效，宁默认关、开关在用户手里——长任务开发建议显式开启）。
//!
//! - **保留集（三合一）**：最近 `keep_turns` 个 turn ∪ `keep_days` 天内 ∪
//!   undo/redo 栈触达的 tree（经 `set_gc` 的下界闭包查询——栈存
//!   forward/post/undo 盘面 tree hex，gc 掉栈里引用的 tree = redo 文件恢复
//!   必炸）。栈是内存态、重启即丢，重启后无栈约束属设计语义；进行中的
//!   cur turn 恒保护（永不清）。
//! - **git 影子形态——重建式**：不动 `.git/objects`（主仓对象零风险）、
//!   不区分 loose/pack（影子库全 loose；发现 pack 文件 = 有人手动打包过，
//!   fail-closed 诚实放弃）。新建临时库（alternates 指向主仓 objects 零拷
//!   贝复用）→ 把保留 tree 闭包里主仓没有的对象 raw 拷进去 → **换名前完
//!   整性自检**（每个保留 tree 在新库做闭包校验；旧库本来就坏的 tree 记
//!   broken_before 基线，不因 gc 变得更坏即可）→ 原子换名 `objects/` 子树
//!   （影子库路径不变：同一 store 的常驻句柄、集群共享 store 的句柄、
//!   refs、index 全部继续有效）。任一步失败诚实放弃，零破坏。
//! - **JSON 形态**：窗口外 turn-N.json 直删（fsutil 韧性包装）。
//! - **触发**：`truncate_from_scoped` 后 + gateway 周期拍（runtime 装配，
//!   6h 与 uploads/logs 同拍）；每轮打 INFO。
//!
//! 锁纪律：inner 快照（短锁）→ 释放 → git 操作（repo 锁只在换名窗口
//! 短持）→ inner 清账（短锁）。绝不嵌套。

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{info, warn};

use super::CheckpointStore;

/// gc 钩子：config 开关 + undo/redo 栈下界查询闭包（loop 弱引用捕获，
/// 由 agent_factory 在装配点 `set_gc` 注入）。
pub(crate) struct GcHooks {
    pub(crate) cfg: nemesis_config::CheckpointGcConfig,
    pub(crate) extra_trees: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
}

/// gc 运行报告（测试断言面 + INFO 日志素材）。
#[derive(Debug, Default, Clone)]
pub struct CheckpointGcReport {
    /// "git" / "json" / ""（skipped 时可能未判定）。
    pub backend: String,
    /// 快照时索引条目总数（done+cur）。
    pub entries_total: usize,
    /// 保留的 turn 数。
    pub kept_turns: usize,
    /// 清出窗口的 turn 数。
    pub dropped_turns: usize,
    /// 删除的 turn-N.json 镜像数（两形态都有——git 形态索引与对象集对齐）。
    pub deleted_mirrors: usize,
    /// 释放字节（JSON 镜像 + git 形态旧 objects 子树目录体积）。
    pub bytes_freed: u64,
    /// git 形态：新库校验通过的保留 tree 数。
    pub trees_kept: usize,
    /// git 形态：新库 raw 拷贝的对象数（主仓 alternates 未覆盖的部分）。
    pub objects_copied: usize,
    /// undo/redo 栈触达的 tree 引用数。
    pub extra_trees: usize,
    /// 旧库已缺失/解析失败的 tree 数（既有损坏，gc 不使其更坏）。
    pub unavailable_trees: usize,
    /// 诚实放弃/跳过原因（未装配、未启用、并发跳过、重建失败……）。
    pub skipped: Option<String>,
}

impl CheckpointGcReport {
    fn skipped(reason: impl Into<String>) -> Self {
        Self {
            skipped: Some(reason.into()),
            ..Default::default()
        }
    }
}

/// 保留集计算的输入条目（纯函数面，测试直构造）。
pub(crate) struct RetentionEntry {
    pub turn: usize,
    /// RFC3339；解析失败 = 拿不到时间窗豁免（仍可凭 turn 窗/extra tree 保留）。
    pub time: String,
    pub tree: Option<String>,
}

/// 保留集判定结果。
pub(crate) struct RetentionDecision {
    pub kept_turns: HashSet<usize>,
    /// 需要保留对象的 tree hex（保留条目的 tree ∪ extras 解析成功者）。
    pub kept_trees: HashSet<String>,
}

/// 保留集纯函数：turn 窗 ∪ 时间窗 ∪ extra tree ∪ 保护集。
///
/// - turn 窗：turn 序号降序前 `keep_turns` 个（0 = 该维关）。
/// - 时间窗：`now - time <= keep_days` 天（0 = 该维关；解析失败无豁免）。
/// - extras：tree ∈ extras 的条目连同 tree 保对象。
/// - protected：进行中 cur turn，恒保留。
pub(crate) fn compute_retention(
    entries: &[RetentionEntry],
    protected: &HashSet<usize>,
    extra_trees: &[String],
    now_unix: i64,
    keep_turns: usize,
    keep_days: u64,
) -> RetentionDecision {
    let extras: HashSet<&str> = extra_trees.iter().map(|s| s.as_str()).collect();
    let mut kept_turns: HashSet<usize> = HashSet::new();
    let mut kept_trees: HashSet<String> = HashSet::new();

    // turn 窗：按 turn 序号取最近 N 个。
    if keep_turns > 0 {
        let mut turns: Vec<usize> = entries.iter().map(|e| e.turn).collect();
        turns.sort_unstable_by(|a, b| b.cmp(a));
        turns.dedup();
        for t in turns.into_iter().take(keep_turns) {
            kept_turns.insert(t);
        }
    }

    for e in entries {
        if protected.contains(&e.turn) {
            kept_turns.insert(e.turn);
        }
        if extras.contains(e.tree.as_deref().unwrap_or("")) {
            kept_turns.insert(e.turn);
        }
        let kept = kept_turns.contains(&e.turn);
        if kept && let Some(tree) = &e.tree {
            kept_trees.insert(tree.clone());
        }
        if keep_days > 0
            && !kept
            && let Ok(t) = chrono::DateTime::parse_from_rfc3339(&e.time)
        {
            let age = now_unix - t.timestamp();
            if age <= (keep_days as i64) * 86_400 {
                kept_turns.insert(e.turn);
                if let Some(tree) = &e.tree {
                    kept_trees.insert(tree.clone());
                }
            }
        }
    }

    // extras 本身（可能不对应任何索引条目，如 undo 时刻的实时盘面 tree）。
    for t in extra_trees {
        kept_trees.insert(t.clone());
    }

    RetentionDecision {
        kept_turns,
        kept_trees,
    }
}

impl CheckpointStore {
    /// M6：装配 gc（config 开关 + undo/redo 栈下界闭包）。未装配 = gc 全
    /// 路径零触碰。主 loop / 项目 loop 各自装配。**集群闸（BUG-95）**：
    /// cluster agent 的 checkpoint store 与主 store 是独立实例**共享同一影
    /// 子库目录**——GitBackend 的 repo Mutex 是实例级的，本 store 换名窗口
    /// 锁不住集群写入者（旧注释「对共享句柄无害」失实，回头检查已纠）；
    /// 且保留集闭包只接主 loop 栈，集群栈引用的 tree 会被误清。故主 store
    /// 的装配点（agent_factory）在 config.cluster.enabled 时**不调用本方法**。
    pub fn set_gc(
        &self,
        cfg: nemesis_config::CheckpointGcConfig,
        extra_trees: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    ) {
        *self.gc_hooks.lock() = Some(GcHooks { cfg, extra_trees });
    }

    /// gc 是否已装配且启用（诚实自省面：factory 集群闸测试 / 运维面用）。
    pub fn gc_armed(&self) -> bool {
        self.gc_hooks.lock().as_ref().is_some_and(|h| h.cfg.enabled)
    }

    /// 跑一轮 gc（装配且 enabled 才真正执行）。同步、可重入安全（并发
    /// 触发后到者诚实跳过）；git 重建失败零破坏（不清索引不删镜像）。
    pub fn gc_now(&self) -> CheckpointGcReport {
        // 钩子读取（短锁）：配置与闭包句柄克隆出来，锁不跨 gc 主体。
        let (cfg, extra_fn) = {
            let guard = self.gc_hooks.lock();
            let Some(h) = guard.as_ref() else {
                return CheckpointGcReport::skipped("gc 未装配（set_gc 未调用，默认形态）");
            };
            if !h.cfg.enabled {
                return CheckpointGcReport::skipped("checkpoint.gc.enabled=false（默认关）");
            }
            (h.cfg.clone(), h.extra_trees.clone())
        };
        if self
            .gc_running
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return CheckpointGcReport::skipped("另一轮 gc 正在进行（并发触发跳过）");
        }
        let report = self.gc_run(&cfg, &extra_fn);
        self.gc_running.store(false, Ordering::Release);
        report
    }

    fn gc_run(
        &self,
        hooks_cfg: &nemesis_config::CheckpointGcConfig,
        extra_fn: &Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    ) -> CheckpointGcReport {
        let mut report = CheckpointGcReport::default();
        report.backend = if self.git.is_some() {
            "git".to_string()
        } else {
            "json".to_string()
        };

        let extras = extra_fn();
        report.extra_trees = extras.len();

        // 段一（inner 短锁）：索引快照 + 保护集。
        let (entries, protected, max_seen) = {
            let guard = self.inner.lock();
            let mut entries: Vec<RetentionEntry> = guard
                .done
                .iter()
                .chain(guard.cur.values())
                .map(|c| RetentionEntry {
                    turn: c.turn,
                    time: c.time.clone(),
                    tree: c.tree.clone(),
                })
                .collect();
            entries.sort_by_key(|e| e.turn);
            entries.dedup_by(|a, b| a.turn == b.turn);
            let protected: HashSet<usize> = guard.cur.values().map(|c| c.turn).collect();
            let max_seen = entries.last().map(|e| e.turn).unwrap_or(0);
            (entries, protected, max_seen)
        };
        report.entries_total = entries.len();

        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let decision = compute_retention(
            &entries,
            &protected,
            &extras,
            now_unix,
            hooks_cfg.keep_turns,
            hooks_cfg.keep_days,
        );
        report.kept_turns = decision.kept_turns.len();
        report.dropped_turns = entries
            .iter()
            .filter(|e| !decision.kept_turns.contains(&e.turn))
            .count();

        // 早退：没有可清条目（频繁触发下常态路径，零 git 开销）。
        if report.dropped_turns == 0 {
            report.skipped = Some("保留集覆盖全部条目，无需清扫".to_string());
            return report;
        }

        // 段二：git 形态先重建对象集（失败 = 整轮放弃，索引/镜像零破坏）。
        #[cfg(feature = "checkpoint-git")]
        if self.git.is_some() {
            let keep_trees: Vec<String> = decision.kept_trees.iter().cloned().collect();
            match self.gc_rebuild_objects(&keep_trees, &mut report) {
                Ok(()) => {}
                Err(e) => {
                    report.skipped = Some(format!("git 重建放弃（零破坏）: {e}"));
                    return report;
                }
            }
        }

        // 段三（inner 短锁）：清账——内存索引对齐保留集。并发新 turn
        //（> max_seen）不在快照里，恒豁免（保留集不可能覆盖 gc 期间
        // begin 出来的新条目）。
        {
            let mut guard = self.inner.lock();
            guard
                .done
                .retain(|c| decision.kept_turns.contains(&c.turn) || c.turn > max_seen);
            guard
                .cur
                .retain(|_, c| decision.kept_turns.contains(&c.turn) || c.turn > max_seen);
            let cur_keys: HashSet<Option<String>> = guard.cur.keys().cloned().collect();
            guard.seen.retain(|k, _| cur_keys.contains(k));
        }

        // 段四：磁盘镜像对齐（fsutil 韧性删除；并发新镜像 > max_seen 豁免）。
        if let Some(dir) = &self.dir
            && let Ok(rd) = std::fs::read_dir(dir)
        {
            for ent in rd.flatten() {
                let name = ent.file_name().to_string_lossy().to_string();
                if let Some(rest) = name
                    .strip_prefix("turn-")
                    .and_then(|s| s.strip_suffix(".json"))
                    && let Ok(t) = rest.parse::<usize>()
                    && t <= max_seen
                    && !decision.kept_turns.contains(&t)
                {
                    let (size, existed) = match std::fs::metadata(ent.path()) {
                        Ok(m) => (m.len(), true),
                        Err(_) => (0, false),
                    };
                    if !existed {
                        continue;
                    }
                    match nemesis_utils::fsutil::remove_file_retry(ent.path()) {
                        Ok(()) => {
                            report.deleted_mirrors += 1;
                            report.bytes_freed += size;
                        }
                        Err(e) => {
                            warn!(
                                file = %ent.path().display(),
                                error = %e,
                                "[checkpoint gc] turn 镜像删除重试耗尽（下轮 gc 重试）"
                            );
                        }
                    }
                }
            }
        }

        info!(
            backend = %report.backend,
            entries = report.entries_total,
            kept = report.kept_turns,
            dropped = report.dropped_turns,
            mirrors = report.deleted_mirrors,
            bytes = report.bytes_freed,
            trees = report.trees_kept,
            copied = report.objects_copied,
            extras = report.extra_trees,
            unavailable = report.unavailable_trees,
            "[checkpoint gc] 完成"
        );
        report
    }

    /// git 形态重建式 gc：保留 tree 闭包 → 临时库 → 完整性自检 → 原子换
    /// 名 `objects/` 子树。任何失败返回 Err（调用方整轮放弃，零破坏）。
    #[cfg(feature = "checkpoint-git")]
    fn gc_rebuild_objects(
        &self,
        keep_trees: &[String],
        report: &mut CheckpointGcReport,
    ) -> Result<(), String> {
        let gb = self
            .git
            .as_ref()
            .ok_or_else(|| "影子库未启用".to_string())?;
        // 影子库路径从常驻句柄读（store 不存该字段）。
        let shadow = gb.repo.lock().path().to_path_buf();
        let root = self.root.clone();
        let open_flags = git2::RepositoryOpenFlags::NO_SEARCH;

        // BUG-96（2026-10-28 二次复查）：残留 `.gc-old` 处置——上轮收尾删除
        // 重试耗尽的遗留（Windows AV/索引器瞬时句柄）。不清则本轮换名
        // rename#1 目标非空必败（Windows 不替换非空目录），而清它的唯一
        // 路径在换名成功之后——永远执行不到，gc 永久卡死（重启不自愈）。
        // 两分支：
        // - objects 在场（常态残留）：预清（best-effort；清不掉 = 继续走，
        //   rename#1 失败臂诚实报错，下轮再来）。
        // - objects 缺席（上轮 rename#2 失败且回滚也失败的破损态）：`.gc-old`
        //   是对象集唯一副本——先搬回自愈（兑现错误信息「下轮自愈」承诺），
        //   失败诚实报错；**绝不预清**（会销毁唯一对象集）。
        let objects_old = shadow.with_file_name(format!(
            "{}.gc-old",
            shadow
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "checkpoints.git".to_string())
        ));
        if objects_old.exists() {
            if shadow.join("objects").exists() {
                if let Err(e) = nemesis_utils::fsutil::remove_dir_all_retry(&objects_old) {
                    tracing::warn!(
                        dir = %objects_old.display(),
                        error = %e,
                        "[checkpoint gc] 残留 .gc-old 预清失败（继续；rename 失败臂兜底）"
                    );
                }
            } else {
                let _heal_guard = gb.repo.lock();
                std::fs::rename(&objects_old, shadow.join("objects"))
                    .map_err(|e| format!("破损态自愈失败（.gc-old 搬回 objects）: {e}"))?;
                tracing::warn!(
                    "[checkpoint gc] 检出上轮回滚失败的破损态，已自愈（.gc-old 搬回 objects）"
                );
            }
        }

        // 前置 fail-closed：影子库被手动打包过（pack 文件在场）→ 重建式不
        // 支持，诚实放弃（宁可不清，不赌对象搬运的完整性）。
        let pack_dir = shadow.join("objects").join("pack");
        if let Ok(rd) = std::fs::read_dir(&pack_dir) {
            for ent in rd.flatten() {
                let name = ent.file_name().to_string_lossy().to_string();
                if name.ends_with(".pack") {
                    return Err(format!("影子库含 pack 文件（{name}），重建式 gc 不支持"));
                }
            }
        }

        let old = git2::Repository::open_ext(&shadow, open_flags, Vec::<&std::ffi::OsStr>::new())
            .map_err(|e| format!("旧影子库打开失败: {e}"))?;
        // 主仓句柄：alternates 覆盖判定（主仓有的对象新库经 alternates 零拷
        // 贝可达，不用拷）。
        let main_repo = git2::Repository::open_ext(
            root.join(".git"),
            open_flags,
            Vec::<&std::ffi::OsStr>::new(),
        )
        .map_err(|e| format!("主仓 .git 打开失败: {e}"))?;
        let main_odb = main_repo
            .odb()
            .map_err(|e| format!("主仓 odb 打开失败: {e}"))?;

        // 种子：保留 tree ∪ 全部 refs 闭包（交付分支 refs/heads/nemesis/*
        // 指向 commit——交付 patch 重算面，gc 不使其失效）。
        let mut seeds: Vec<git2::Oid> = Vec::new();
        let mut unavailable = 0usize;
        for hex in keep_trees {
            match git2::Oid::from_str(hex) {
                Ok(oid) => seeds.push(oid),
                Err(_) => unavailable += 1,
            }
        }
        match old.references() {
            Ok(refs) => {
                for ref_ in refs {
                    let Ok(r) = ref_ else { continue };
                    let name = r.name().unwrap_or_default().to_string();
                    if let Ok(commit) = r.peel_to_commit() {
                        seeds.push(commit.id());
                    } else if let Ok(tree) = r.peel_to_tree() {
                        seeds.push(tree.id());
                    } else {
                        // 既有坏 ref（交付 patch 本就已坏）：debug 留痕，不阻 gc。
                        tracing::debug!(
                            ref = %name,
                            "[checkpoint gc] ref 不可 peel（既有损坏，跳过）"
                        );
                    }
                }
            }
            Err(e) => return Err(format!("refs 枚举失败: {e}")),
        }

        // 闭包行走：主仓覆盖的跳过（alternates 达），只收集影子库自有的。
        let old_odb = old.odb().map_err(|e| format!("旧库 odb 打开失败: {e}"))?;
        let mut needed: Vec<(git2::ObjectType, git2::Oid)> = Vec::new();
        let mut visited: HashSet<git2::Oid> = HashSet::new();
        let mut stack: Vec<git2::Oid> = seeds.clone();
        while let Some(oid) = stack.pop() {
            if !visited.insert(oid) || main_odb.exists(oid) {
                continue;
            }
            let Ok(obj) = old_odb.read(oid) else {
                // 既不在主仓也不在旧库 = 既有缺失；上行 tree 引用它的条目
                // 本就 restore 失败，gc 不使其更坏。计数留痕。
                unavailable += 1;
                continue;
            };
            match obj.kind() {
                git2::ObjectType::Tree => {
                    let tree = old
                        .find_tree(oid)
                        .map_err(|e| format!("tree {oid} 读取失败: {e}"))?;
                    needed.push((obj.kind(), oid));
                    for ent in tree.iter() {
                        match ent.kind() {
                            Some(git2::ObjectType::Tree) | Some(git2::ObjectType::Blob) => {
                                stack.push(ent.id());
                            }
                            // gitlink（子模块）不是本库对象，跳过。
                            _ => {}
                        }
                    }
                }
                git2::ObjectType::Blob => needed.push((obj.kind(), oid)),
                git2::ObjectType::Commit => {
                    let commit = old
                        .find_commit(oid)
                        .map_err(|e| format!("commit {oid} 读取失败: {e}"))?;
                    needed.push((obj.kind(), oid));
                    stack.push(commit.tree_id());
                }
                _ => {}
            }
        }

        // 换名前基线：旧库上本来就坏的保留 tree（gc 后允许同等坏，不因 gc
        // 变得更坏即可）。
        let mut broken_before: HashSet<String> = HashSet::new();
        for hex in keep_trees {
            if let Ok(oid) = git2::Oid::from_str(hex)
                && Self::gc_verify_tree_closure(&old, oid).is_err()
            {
                broken_before.insert(hex.clone());
            }
        }
        report.unavailable_trees = unavailable;

        // 临时库（同卷相邻目录 → 换名原子性）。崩溃残留先清（幂等）。
        let tmp = shadow.with_file_name(format!(
            "{}.gc-tmp",
            shadow
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "checkpoints.git".to_string())
        ));
        let _ = nemesis_utils::fsutil::remove_dir_all_retry(&tmp);
        let mut init_opts = git2::RepositoryInitOptions::new();
        init_opts.bare(true).mkpath(true);
        let new_repo = git2::Repository::init_opts(&tmp, &init_opts)
            .map_err(|e| format!("临时影子库创建失败: {e}"))?;
        new_repo
            .set_workdir(&root, false)
            .map_err(|e| format!("临时影子库 workdir 附加失败: {e}"))?;
        Self::ensure_workdir(&new_repo, &root)?;
        Self::ensure_alternates(&tmp, &root)
            .map_err(|e| format!("临时库 alternates 写失败: {e}"))?;

        // raw 对象拷贝（类型与字节原样，tree 不需重建——含 refs 的 commit）。
        {
            let new_odb = new_repo
                .odb()
                .map_err(|e| format!("新库 odb 打开失败: {e}"))?;
            for (kind, oid) in &needed {
                let obj = old_odb
                    .read(*oid)
                    .map_err(|e| format!("对象 {oid} 回读失败: {e}"))?;
                new_odb
                    .write(*kind, obj.data())
                    .map_err(|e| format!("对象 {oid} 写入新库失败: {e}"))?;
                report.objects_copied += 1;
            }
        }

        // 完整性自检（换名前，零风险窗口）：每个保留 tree 在新库做闭包校
        // 验；旧库本来就坏的（broken_before）放行同坏，其余必须全通。
        let mut trees_kept = 0usize;
        for hex in keep_trees {
            let Ok(oid) = git2::Oid::from_str(hex) else {
                continue;
            };
            if broken_before.contains(hex) {
                // 旧库已坏：不拷不验（对象缺失是既有状态）。
                continue;
            }
            let new_odb = new_repo
                .odb()
                .map_err(|e| format!("新库 odb 打开失败: {e}"))?;
            Self::gc_verify_tree_closure(&new_repo, oid)
                .map_err(|missing| format!("完整性自检失败（tree {hex} 缺对象 {missing}）"))?;
            let _ = new_odb; // odb 句柄仅证明可达链完整
            trees_kept += 1;
        }
        report.trees_kept = trees_kept;
        drop(new_repo); // 换名窗口前落袋新库句柄（Windows 不钉 tmp/objects）

        // 原子换名 objects/ 子树：影子库路径不变 → 常驻句柄 / 集群共享
        // store 句柄 / refs / index 全部继续有效（loose 对象按路径按需读，
        // 无持久句柄钉住）。objects_old 已在函数头构造（BUG-96 残留处置）。
        // 换名窗口串行本 store 的并发 git 操作。
        let _repo_guard = gb.repo.lock();
        std::fs::rename(shadow.join("objects"), &objects_old)
            .map_err(|e| format!("旧 objects 换名失败（瞬时句柄占用，下轮重试）: {e}"))?;
        if let Err(e) = std::fs::rename(tmp.join("objects"), shadow.join("objects")) {
            // 回滚：旧 objects 换回原位（影子库保持原样）。
            let rb = std::fs::rename(&objects_old, shadow.join("objects"));
            return Err(format!(
                "新 objects 换入失败（回滚 {}）: {e}",
                if rb.is_ok() {
                    "成功"
                } else {
                    "失败——影子库 objects 缺席，下轮 gc/自愈重试"
                }
            ));
        }
        drop(_repo_guard);

        // 收尾（best-effort）：旧 objects 子树删除（体积计入释放；瞬时句
        // 柄耗尽重试也耗尽 → 残留下轮 gc 重试）。
        let old_bytes = dir_size(&objects_old);
        match nemesis_utils::fsutil::remove_dir_all_retry(&objects_old) {
            Ok(()) => report.bytes_freed += old_bytes,
            Err(e) => warn!(
                dir = %objects_old.display(),
                error = %e,
                "[checkpoint gc] 旧 objects 子树删除重试耗尽（残留下轮重试，不影响正确性）"
            ),
        }
        let _ = nemesis_utils::fsutil::remove_dir_all_retry(&tmp);
        Ok(())
    }

    /// 保留 tree 闭包校验：新库（含 alternates 链）上每个可达 Tree/Blob
    /// 对象必须存在。返回 Err(首个缺失 oid)。
    #[cfg(feature = "checkpoint-git")]
    fn gc_verify_tree_closure(repo: &git2::Repository, root: git2::Oid) -> Result<(), git2::Oid> {
        let odb = repo.odb().map_err(|_| root)?;
        let mut visited: HashSet<git2::Oid> = HashSet::new();
        let mut stack = vec![root];
        while let Some(oid) = stack.pop() {
            if !visited.insert(oid) {
                continue;
            }
            if !odb.exists(oid) {
                return Err(oid);
            }
            if let Ok(tree) = repo.find_tree(oid) {
                for ent in tree.iter() {
                    match ent.kind() {
                        Some(git2::ObjectType::Tree) => stack.push(ent.id()),
                        Some(git2::ObjectType::Blob) if !odb.exists(ent.id()) => {
                            return Err(ent.id());
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }
}

/// 目录体积快算（gc 报告素材；读不到 = 0）。
fn dir_size(dir: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(m) = ent.metadata() {
                total += m.len();
            }
        }
    }
    total
}
