//! 项目注册表（`<workspace>/config/projects.json`）。
//!
//! **注册不拥有**：删除项目只解除分组，不删会话与项目目录内的任何文件
//! （行业共识，设计档）。纯文件操作：load（缺席=空表首用 / 损坏=Err，
//! 写路径 loud 拒绝——绝不把损坏文件用空表覆盖，那等于静默清空用户数据）+
//! save（原子写 tmp+rename，同 `eval_assessor::save_rules` 形态）+
//! create/remove/rename。
//!
//! 路径 canonicalize 一次（`canonicalize_for_compare`，剥 verbatim + 归一
//! 8.3 短名），与围栏比较层同表示——「构建时 canonicalize 一次」的地基（
//! 项目工厂直接消费 entry.path，不再二次 canonicalize）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use nemesis_path::paths::canonicalize_for_compare;
use nemesis_path::resolve_projects_registry_path_in_workspace;

/// 注册表写串行锁（进程级）：create/remove/rename/set_trust 都是
/// load→改→save 的 RMW，WSAPI 各命令臂（trust 翻转 / create / rename /
/// remove）可在 dashboard 并发到达，无锁时后写者会以自己的快照整体覆盖
/// 先写者的更新（save 虽原子，丢的是先者的条目/信任位——trust 被冲回
/// true = 指令信任闸静默失效）。只串行写者——读路径吃 rename 原子性。
static REGISTRY_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// id 前缀（`p-{8hex}`）。
const ID_PREFIX: &str = "p-";
/// id 的 hex 段长度。
const ID_HEX_LEN: usize = 8;
/// 项目名称最大长度（防滥用；超限拒绝）。
const NAME_MAX: usize = 100;
/// 每项目附加放行目录上限（防滥用；超限拒绝）。
const EXTRA_DIRS_MAX: usize = 8;

/// `#[serde(default = "default_true")]` 的默认值函数（老条目零迁移——缺键
/// 反序列化为 true，保持主循环同款行为）。
fn default_true() -> bool {
    true
}

/// 单条项目注册。创建时烧入：`path` 不可改（改路径 = 移除后重建，
/// 语义）；`name` 可改（rename）；`instructions_trusted` 可改
/// （`projects.trust` WSAPI）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectEntry {
    pub id: String,
    pub name: String,
    /// canonical（剥 verbatim）后的项目目录绝对路径。
    pub path: PathBuf,
    /// RFC3339 创建时间。
    pub created_at: String,
    /// 项目工作区指令（AGENTS.md/CLAUDE.md）是否受信。
    /// 缺省 true——注册即信任行为（与主循环一致，老条目零迁移）；false 时
    /// 该项目 loop 按 config `projects.instruction_trust` 全局策略降级
    /// （annotate=注入但加不可信来源横幅 / isolated=整段跳过）。
    #[serde(default = "default_true")]
    pub instructions_trusted: bool,
    /// 附加放行根（围栏多根，2026-11-19 D 批）：canonical 绝对路径字符串
    /// （与 `path` 同表示——剥 verbatim + 归一 8.3）。项目 loop 的
    /// `WorkspaceBoundary.extra_roots` 注入（主根之外 bot 可读写的目录——
    /// 资产库/交付盘等）。serde default 零迁移；无重叠由 `add_project_dir`
    /// 校验保证（对主 workspace / 任何项目主根 / 任何已有 extra_dir 双向
    /// 不重叠）。
    /// **诚实边界**：extra_dirs 不进 checkpoint 快照（影子库枚举走
    /// workspace root）、不进前端文件树（fs.tree root=项目目录）——rewind/
    /// undo 对 extra_dirs 的变更无回退网，文件树可见性不在多根范围。
    #[serde(default)]
    pub extra_dirs: Vec<String>,
    /// B38 孤儿组老化：**首次观测到目录缺失**的时间戳（RFC3339，本地时区
    /// 同 created_at）。`#[serde(default)]` 老条目/存活条目零迁移（None）。
    /// 语义：sweep 只在启动装配跑——目录存在 = 清除（复活面是目录存在性
    /// 检查，非一次性快照：TTL 内恢复自动归零）；目录缺失 + None = 记现在；
    /// 缺失 + 超 TTL = 条目老化解除分组（与手动 remove 同语义）。
    #[serde(default)]
    pub missing_since: Option<String>,
}

/// projects.json 根结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProjectsFile {
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
}

/// 注册表路径（`<workspace>/config/projects.json`；真相源在 nemesis-path，
/// 本封装为 nemesisbot 侧便捷入口）。
pub fn registry_path(workspace: &Path) -> PathBuf {
    resolve_projects_registry_path_in_workspace(workspace)
}

/// 读注册表。`Ok(None)` = 文件不存在（首次使用）；`Err` = 存在但损坏或不可
/// 读——**写路径必须 loud 拒绝**（用空表覆盖损坏文件 = 静默清空用户数据）。
pub fn try_load_projects(path: &Path) -> Result<Option<ProjectsFile>> {
    // BUG-20261109-036：注册表是用户可编辑资产（save_projects 落 pretty
    // JSON）——读侧走 BOM 剥离 helper。裸读时 Windows 记事本 utf8BOM 保存
    // 会解析失败，触发下方「已损坏请手动修复或删除」误导文案（文件完好，
    // 用户按提示删除 = 恰是本函数注释要防的静默清空用户数据）。
    let data = match nemesis_utils::fsutil::read_config_string(path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow!("读取项目注册表失败 {}: {}", path.display(), e));
        }
    };
    match serde_json::from_str::<ProjectsFile>(&data) {
        Ok(f) => Ok(Some(f)),
        Err(e) => {
            tracing::warn!(
                "[projects] 注册表解析失败 {}: {}（写操作拒绝，避免覆盖损坏文件）",
                path.display(),
                e
            );
            Err(anyhow!(
                "项目注册表已损坏: {}（{e}）；请手动修复或删除该文件后重试",
                path.display()
            ))
        }
    }
}

/// 读注册表（list 语义）：缺席/损坏都给空表 + warn——读路径不炸。
pub fn load_projects_lenient(path: &Path) -> ProjectsFile {
    match try_load_projects(path) {
        Ok(f) => f.unwrap_or_default(),
        Err(_) => ProjectsFile::default(),
    }
}

/// 原子写 — 起委托统一 helper
/// `nemesis_utils::write_file_atomic`（唯一临时名 + sync_all + 失败清理）。
pub fn save_projects(path: &Path, file: &ProjectsFile) -> Result<()> {
    let content = serde_json::to_string_pretty(file).context("serialize projects")?;
    // helper 自身错误已带 "atomic write {path}: {step}" 前缀，无需再包一层 context。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600)
        .map_err(anyhow::Error::msg)?;
    Ok(())
}

/// `p-{8hex}`：uuid v4 前 4 字节 hex（uuid 已在依赖树——nemesis-web
/// sessions.create 同源形态）。
pub fn new_project_id() -> String {
    let u = uuid::Uuid::new_v4();
    let hex: String = u.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    format!("{ID_PREFIX}{}", &hex[..ID_HEX_LEN])
}

/// 规范化 + 校验项目路径：非空、绝对、存在且为目录、canonical 后不与主
/// workspace 或已有项目重叠（相等或任一为另一的祖先——防围栏嵌套歧义）；
/// 与已有项目 extra_dirs 精确相等放行（共享）、嵌套拒绝。返回 canonical
/// 路径（剥 verbatim，围栏比较层同表示）。
pub fn canonicalize_project_path(
    raw: &str,
    main_workspace: &Path,
    existing: &[ProjectEntry],
) -> Result<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("项目路径不能为空");
    }
    let p = Path::new(raw);
    if !p.is_absolute() {
        bail!("项目路径必须是绝对路径: {raw}");
    }
    let canon = canonicalize_for_compare(p);
    if !canon.is_dir() {
        bail!("项目目录不存在或不是目录: {}", canon.display());
    }
    let ws = canonicalize_for_compare(main_workspace);
    if paths_overlap(&ws, &canon) {
        bail!(
            "项目目录与主工作区重叠: {}（围栏嵌套歧义，禁止）",
            canon.display()
        );
    }
    for e in existing {
        if paths_overlap(&e.path, &canon) {
            bail!(
                "项目目录与项目「{}」重叠: {} vs {}",
                e.name,
                e.path.display(),
                canon.display()
            );
        }
        // 已有 extra_dirs：精确相等 = 共享（项目主根挂为别人附加目录，
        // 与 add_dir 侧同规则——建项顺序无关）；嵌套拒绝（围栏嵌套歧义）。
        for d in &e.extra_dirs {
            let d_canon = canonicalize_for_compare(Path::new(d));
            if d_canon != canon && paths_overlap(&d_canon, &canon) {
                bail!(
                    "项目目录与项目「{}」的附加目录嵌套重叠: {} vs {}（围栏嵌套歧义，禁止）",
                    e.name,
                    d,
                    canon.display()
                );
            }
        }
    }
    Ok(canon)
}

/// 相等或祖先关系（双向组件级 starts_with——canonical 后可直接比较）。
fn paths_overlap(a: &Path, b: &Path) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
}

/// 校验项目名（create/rename 共用；统一定义）。
fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("项目名称不能为空");
    }
    if name.chars().count() > NAME_MAX {
        bail!("项目名称过长（>{NAME_MAX} 字符）");
    }
    Ok(name.to_string())
}

/// 构造新条目（纯函数，不落盘）：名字校验 → 上限校验 → 路径校验 → 烧入
/// id/时间。
pub fn build_new_entry(
    file: &ProjectsFile,
    main_workspace: &Path,
    name: &str,
    raw_path: &str,
    max: usize,
) -> Result<ProjectEntry> {
    let name = validate_name(name)?;
    if file.projects.len() >= max {
        bail!(
            "项目数已达上限（{max}）：每项目一个常驻执行引擎，如需更多请调大 config projects.max"
        );
    }
    let path = canonicalize_project_path(raw_path, main_workspace, &file.projects)?;
    Ok(ProjectEntry {
        id: new_project_id(),
        name,
        path,
        created_at: chrono::Local::now().to_rfc3339(),
        instructions_trusted: true,
        extra_dirs: Vec::new(),
        missing_since: None,
    })
}

/// 创建项目（读-校验-写；损坏文件 loud 拒绝）。返回新条目（调用方 spawn loop）。
///
/// 写侧全程持有 [`REGISTRY_WRITE_LOCK`]——RMW 与并发写者互斥（见锁注释）。
pub fn create_project(
    path: &Path,
    main_workspace: &Path,
    name: &str,
    raw_path: &str,
    max: usize,
) -> Result<ProjectEntry> {
    let _serial = REGISTRY_WRITE_LOCK.lock();
    let mut file = try_load_projects(path)?.unwrap_or_default();
    let entry = build_new_entry(&file, main_workspace, name, raw_path, max)?;
    file.projects.push(entry.clone());
    save_projects(path, &file)?;
    Ok(entry)
}

/// 移除项目（只解除分组——不删会话/文件；不存在报错）。返回被移除条目
/// （调用方负责 stop loop + 路由摘除）。
///
/// 写侧全程持有 [`REGISTRY_WRITE_LOCK`]（同 [`create_project`]）。
pub fn remove_project(path: &Path, id: &str) -> Result<ProjectEntry> {
    let _serial = REGISTRY_WRITE_LOCK.lock();
    let mut file = try_load_projects(path)?.unwrap_or_default();
    let idx = file
        .projects
        .iter()
        .position(|p| p.id == id)
        .ok_or_else(|| anyhow!("项目不存在: {id}"))?;
    let entry = file.projects.remove(idx);
    save_projects(path, &file)?;
    Ok(entry)
}

/// 重命名（`path` 不可改——：改路径 = 移除后重建）。返回更新后条目。
///
/// 写侧全程持有 [`REGISTRY_WRITE_LOCK`]（同 [`create_project`]）。
pub fn rename_project(path: &Path, id: &str, new_name: &str) -> Result<ProjectEntry> {
    let _serial = REGISTRY_WRITE_LOCK.lock();
    let new_name = validate_name(new_name)?;
    let mut file = try_load_projects(path)?.unwrap_or_default();
    let entry = file
        .projects
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| anyhow!("项目不存在: {id}"))?;
    entry.name = new_name;
    let updated = entry.clone();
    save_projects(path, &file)?;
    Ok(updated)
}

/// 翻转项目指令信任位（读-改-写，损坏文件 loud 拒绝）。返回更新后条目。
///
/// 写侧全程持有 [`REGISTRY_WRITE_LOCK`]——trust 翻转与 create/rename/remove
/// 并发时若不互斥，翻转会被对方快照整体覆盖回 true（指令信任闸静默失效）。
pub fn set_project_trust(path: &Path, id: &str, trusted: bool) -> Result<ProjectEntry> {
    let _serial = REGISTRY_WRITE_LOCK.lock();
    let mut file = try_load_projects(path)?.unwrap_or_default();
    let entry = file
        .projects
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| anyhow!("项目不存在: {id}"))?;
    entry.instructions_trusted = trusted;
    let updated = entry.clone();
    save_projects(path, &file)?;
    Ok(updated)
}

/// 校验附加放行目录（D3 add_dir 入参）：非空、绝对、存在且为目录、canonical
/// 后与主 workspace 任何重叠拒绝；与项目主根/已有 extra_dir **精确相等 =
/// 共享放行**、**嵌套（祖先/后代）拒绝**（围栏嵌套歧义禁令）。规则单一化
/// （2026-11-25 用户裁决）：精确相等共享是合法工作流（同一资产目录挂多个
/// 项目），旧「跨项目相等防呆拒绝（先摘再加）」只能搬不能共享、挡住真实
/// 用法且报错误导（`C:\Zoo vs C:\Zoo`），弃用；围栏执行面 per-loop 判界，
/// 共享不产生越权。返回 canonical 字符串（与 [`ProjectEntry::extra_dirs`]
/// 存储同表示）。自己的已有 extra_dir 精确相等在下方跳过后由
/// [`add_project_dir`] 以「已存在」诚实报错。
pub fn canonicalize_extra_dir(
    raw: &str,
    main_workspace: &Path,
    existing: &[ProjectEntry],
) -> Result<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("附加目录路径不能为空");
    }
    let p = Path::new(raw);
    if !p.is_absolute() {
        bail!("附加目录必须是绝对路径: {raw}");
    }
    let canon = canonicalize_for_compare(p);
    if !canon.is_dir() {
        bail!("附加目录不存在或不是目录: {}", canon.display());
    }
    let ws = canonicalize_for_compare(main_workspace);
    if paths_overlap(&ws, &canon) {
        bail!(
            "附加目录与主工作区重叠: {}（围栏嵌套歧义，禁止）",
            canon.display()
        );
    }
    for e in existing {
        // 项目主根：精确相等 = 共享（把别的项目的主根挂为自己的放行目录）；
        // 嵌套拒绝（围栏嵌套歧义）。
        if e.path != canon && paths_overlap(&e.path, &canon) {
            bail!(
                "附加目录与项目「{}」主根嵌套重叠: {} vs {}（围栏嵌套歧义，禁止）",
                e.name,
                e.path.display(),
                canon.display()
            );
        }
        for d in &e.extra_dirs {
            let d_canon = canonicalize_for_compare(Path::new(d));
            if d_canon == canon {
                // 精确相等：自己的 = 重复添加（add_project_dir 报「已存在」）；
                // 别人的 = 跨项目共享，放行。
                continue;
            }
            if paths_overlap(&d_canon, &canon) {
                bail!(
                    "附加目录与项目「{}」的附加目录嵌套重叠: {} vs {}（围栏嵌套歧义，禁止）",
                    e.name,
                    d,
                    canon.display()
                );
            }
        }
    }
    Ok(canon.to_string_lossy().into_owned())
}

/// 给项目添加附加放行目录（读-校验-写；损坏文件 loud 拒绝）。返回更新后
/// 条目（调用方负责热更新对应 loop 的 boundary——重启前注册表是真相源）。
///
/// 写侧全程持有 [`REGISTRY_WRITE_LOCK`]（同 [`create_project`]）。
pub fn add_project_dir(
    path: &Path,
    id: &str,
    raw: &str,
    main_workspace: &Path,
) -> Result<ProjectEntry> {
    let _serial = REGISTRY_WRITE_LOCK.lock();
    let mut file = try_load_projects(path)?.unwrap_or_default();
    let canon = canonicalize_extra_dir(raw, main_workspace, &file.projects)?;
    let entry = file
        .projects
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| anyhow!("项目不存在: {id}"))?;
    if entry.extra_dirs.iter().any(|d| d == &canon) {
        bail!("附加目录已存在: {canon}");
    }
    if entry.extra_dirs.len() >= EXTRA_DIRS_MAX {
        bail!("附加目录数已达上限（{EXTRA_DIRS_MAX}）");
    }
    entry.extra_dirs.push(canon);
    let updated = entry.clone();
    save_projects(path, &file)?;
    Ok(updated)
}

/// 移除项目的附加放行目录（**摘列表不动磁盘**——注册不拥有；入参按
/// canonical 比较匹配，不匹配=不存在报错）。返回更新后条目。
///
/// 写侧全程持有 [`REGISTRY_WRITE_LOCK`]（同 [`create_project`]）。
pub fn remove_project_dir(path: &Path, id: &str, dir: &str) -> Result<ProjectEntry> {
    let _serial = REGISTRY_WRITE_LOCK.lock();
    let mut file = try_load_projects(path)?.unwrap_or_default();
    let canon = canonicalize_for_compare(Path::new(dir.trim()))
        .to_string_lossy()
        .into_owned();
    let entry = file
        .projects
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| anyhow!("项目不存在: {id}"))?;
    let idx = entry
        .extra_dirs
        .iter()
        .position(|d| d == &canon)
        .ok_or_else(|| anyhow!("附加目录不存在: {dir}"))?;
    entry.extra_dirs.remove(idx);
    let updated = entry.clone();
    save_projects(path, &file)?;
    Ok(updated)
}

/// 列出全部（lenient：损坏给空 + warn——读路径不炸）。
pub fn list_projects(path: &Path) -> Vec<ProjectEntry> {
    load_projects_lenient(path).projects
}

/// 按 id 查单条（lenient 语义）。
pub fn find_project(path: &Path, id: &str) -> Option<ProjectEntry> {
    list_projects(path).into_iter().find(|p| p.id == id)
}

/// sweep 结果：`(本轮新标记 missing_since 的条目, 老化解除分组的条目)`。
pub type MissingSweep = (Vec<ProjectEntry>, Vec<ProjectEntry>);

/// B38 孤儿组老化 sweep（启动装配期一次）：按**目录存在性**逐条判定——
/// 存在 = 清除 `missing_since`（复活）；缺失 + 无戳 = 记现在（首次观测）；
/// 缺失 + 超 `ttl_secs` = 条目从注册表移除（与手动 remove 同语义：只解除
/// 分组，会话/项目目录内文件零删除；重新注册同名项目自动重新收编）。
/// `ttl_secs == 0` = 关闭（外置盘长期离线场景），直通零改动。
///
/// `Err` = 注册表缺席之外读取失败（损坏）——**loud 拒绝**（同写路径纪律，
/// 绝不拿 sweep 覆盖损坏文件）；调用方 warn 跳过即可。
///
/// 写侧全程持有 [`REGISTRY_WRITE_LOCK`]；无任何状态变化时不重写盘
/// （字节零变化，日常启动零成本）。
pub fn sweep_missing_projects(path: &Path, ttl_secs: u64) -> Result<MissingSweep> {
    if ttl_secs == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let _serial = REGISTRY_WRITE_LOCK.lock();
    let mut file = try_load_projects(path)?.unwrap_or_default();
    let mut marked = Vec::new();
    let mut aged_out = Vec::new();
    let mut dirty = false;
    let now = chrono::Local::now();
    // BUG-20261127-021：ttl_secs 是 u64 无上界校验，裸 `as i64` 双缺陷面
    // ① ≥2^63 回绕成负 TTL → `age >= ttl` 恒真 → 全部缺失条目立即老化
    // （「永不老化」意图反转为「马上全解除分组」）；② 正值超 chrono 毫秒
    // i64 表示域（>~9.2e15 秒）Duration::seconds panic → gateway 装配期
    // 崩溃。按 time_bounds 统一界（R735 数字恐慌面同族）钳到 100 年——
    // 对 sane 值恒等，巨值 = 实际等于永不，意图不反转。
    let ttl = chrono::Duration::seconds(
        ttl_secs.min(nemesis_utils::time_bounds::MAX_AGE_SECS as u64) as i64,
    );
    for entry in &mut file.projects {
        if entry.path.is_dir() {
            // 复活臂：目录回来了，清戳（dirty 仅在真有戳可清时置）。
            if entry.missing_since.take().is_some() {
                dirty = true;
            }
            continue;
        }
        match &entry.missing_since {
            None => {
                // 首次观测：只记时间戳，不解除分组。
                entry.missing_since = Some(now.to_rfc3339());
                marked.push(entry.clone());
                dirty = true;
            }
            Some(ts) => {
                // parse 先产自有值（结束对 entry 的借用），match 再分派——
                // Err 臂要写回 entry。
                match chrono::DateTime::parse_from_rfc3339(ts) {
                    Ok(t) if now.signed_duration_since(t.with_timezone(&chrono::Local)) >= ttl => {
                        aged_out.push(entry.clone());
                    }
                    Ok(_) => {} // TTL 内：保持现状（不重置计时）
                    Err(_) => {
                        // 戳损坏（手编/异源写入）：re-anchor 到现在——从零
                        // 重数，绝不因坏戳直接老化（从严）。
                        entry.missing_since = Some(now.to_rfc3339());
                        marked.push(entry.clone());
                        dirty = true;
                    }
                }
            }
        }
    }
    if !aged_out.is_empty() {
        file.projects
            .retain(|e| !aged_out.iter().any(|a| a.id == e.id));
        dirty = true;
    }
    if dirty {
        save_projects(path, &file)?;
    }
    Ok((marked, aged_out))
}

#[cfg(test)]
mod tests;
