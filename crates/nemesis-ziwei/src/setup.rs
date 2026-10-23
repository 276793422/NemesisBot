//! setup —— 配置期引擎 SetupManager（M3，总控 §5.1 / `docs/PLAN/2026-10-23_紫微系统M3配置期引擎技术方案.md`）。
//!
//! 六步向导的 bot 侧状态机：选目标 → 快照 → 分析 → 候选/差分 → 标注 → 提交。
//! 分析计算全在 [`nemesis_ziwei_rev`]（纯函数、进程无关）；本模块只做编排：
//! 会话管道读内存 → 喂给 rev → 结果落 draft（`workspace/ziwei/drafts/<id>/`）。
//!
//! draft 纪律（不走 `ProfileStore::save`）：draft 允许中途形态（tools 空、
//! functions 空），全量校验只在 commit 时跑；commit = validate → 迁移到
//! `workspace/ziwei/profiles/<id>/`（ProfileStore 写 profile.json + anchors.json）
//! → setup 快照迁移到 profile 名下（M5 找回消费）→ 删 draft 目录。
//!
//! 单向导会话：同一 gateway 进程同时只允许一个活跃 draft（`select_target`
//! 幂等拒绝）。状态在内存；gateway 重启后 draft 目录仍在——`analyze` 可重算
//! 续走，`abort` 可清理孤儿 draft（status 以磁盘态兜底呈现）。
//!
//! 非 Windows：会话/管道层天然诚实报错（session stub），annotate/
//! define_struct/define_tool/commit 等纯文件操作跨平台可用。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use nemesis_ziwei_profile::model::parse_byte_pattern;
use nemesis_ziwei_profile::{
    Anchor, AnchorSet, AnchorType, Conv, DataStructure, FieldDef, LocateKind, LocateSpec, Machine,
    Profile, ProfileStore,
};
// 指纹/版本/attach 判定仅活体路径消费（windows-only；Linux 形态 unused）。
#[cfg(windows)]
use nemesis_ziwei_profile::{
    AttachInfo, ProfileStatus, SCHEMA_VERSION, TargetFingerprint, Verification, VerifyPolicy,
};
use nemesis_ziwei_rev::candidates;
use nemesis_ziwei_rev::crtsigs;
#[cfg(windows)]
use nemesis_ziwei_rev::diff;
use nemesis_ziwei_rev::disasm::Arch;
use nemesis_ziwei_rev::funcs;
use nemesis_ziwei_rev::pe::{self, ImageMap, PeInfo};
#[cfg(windows)]
use nemesis_ziwei_rev::snapshot::SnapshotMeta;
use nemesis_ziwei_rev::snapshot::SnapshotStore;
use nemesis_ziwei_rev::xref;
use serde_json::json;
#[cfg(windows)]
use sha2::Digest;

/// 分析引擎信息转发（保持 `setup::engine_info` 旧路径稳定）。
pub use nemesis_ziwei_rev::engine::{EngineInfo, engine_info};

// ── 常量（PE 分析/活体读消费——windows-only；Linux 形态诚实缺席）────────

/// 头页读取量（PE 头 + 节表足够）。
#[cfg(windows)]
const HEADERS_BYTES: u64 = 0x1000;
/// 单次管道读分块（1MB；MAX_FRAME 4MB 留 4 倍余）。
#[cfg(windows)]
const CHUNK_BYTES: u64 = 1 << 20;
/// 单节快照上限（64MB——正常用户态模块节远小于此；超出诚实截断并注记）。
#[cfg(windows)]
const SECTION_CAP: u64 = 64 << 20;
/// 候选清单上限（向导展示面）。
const CANDIDATE_CAP: usize = 32;
/// 差分报告页数上限（超出截断注记）。
#[cfg(windows)]
const DIFF_PAGE_CAP: usize = 64;
/// 快照保留（TTL 7 天 / 最多 8 份）。
#[cfg(windows)]
const SNAP_TTL: std::time::Duration = std::time::Duration::from_secs(7 * 86400);
#[cfg(windows)]
const SNAP_MAX_KEPT: usize = 8;

// ── 状态 ────────────────────────────────────────────────────────────────

/// 内存分析缓存（analyze 产物；candidates/标注提示直接消费，避免重跑反汇编）。
struct AnalysisCache {
    img: ImageMap,
    set: funcs::FunctionSet,
    idx: xref::XrefIndex,
    crt: Vec<crtsigs::CrtSuspect>,
    #[allow(dead_code)] // M5 找回实装时消费（重扫归因需要导入面）
    imports: Vec<pe::ImportDll>,
    #[allow(dead_code)] // M5 找回实装时消费（rescan 需要 arch）
    arch: Arch,
}

/// 一个活跃 draft 的向导状态（标量面；Clone 供锁外使用）。
#[derive(Debug, Clone)]
struct SetupInfo {
    profile_id: String,
    pid: u32,
    module_base: u64,
    module_size: u32,
    machine: Machine,
    exe_path: PathBuf,
    exe_name: String,
    file_sha256: String,
}

impl SetupInfo {
    fn to_json(&self, has_analysis: bool) -> serde_json::Value {
        json!({
            "profile_id": self.profile_id,
            "pid": self.pid,
            "module_base": format!("0x{:X}", self.module_base),
            "module_size": self.module_size,
            "machine": self.machine.payload_suffix(),
            "exe_path": self.exe_path.display().to_string(),
            "exe_name": self.exe_name,
            "file_sha256": self.file_sha256,
            "analysis_cached": has_analysis,
        })
    }
}

/// 完整向导状态 = 标量 + 分析缓存（缓存非 Clone，整体不 Clone）。
struct SetupState {
    info: SetupInfo,
    analysis: Option<AnalysisCache>,
}

/// 配置期向导管理器（gateway 装配一个全局实例；CLI/bridge 经全局槽访问）。
pub struct SetupManager {
    state: Mutex<Option<SetupState>>,
}

impl SetupManager {
    pub fn new() -> SetupManager {
        SetupManager {
            state: Mutex::new(None),
        }
    }

    /// 当前活跃 draft 状态（status 面；无 = None）。
    pub fn status(&self, workspace: &Path) -> Option<serde_json::Value> {
        if let Some(st) = self.state.lock().unwrap().as_ref() {
            return Some(st.info.to_json(st.analysis.is_some()));
        }
        // gateway 重启后内存态丢失，但 draft 目录还在 → 以磁盘态兜底呈现
        latest_draft_id(workspace).map(|id| {
            json!({
                "profile_id": id,
                "resumed": true,
                "note": "gateway 重启后内存态已丢；从 draft 目录恢复——analyze 可重建，abort 可清理",
            })
        })
    }

    fn state_mut(&self) -> std::sync::MutexGuard<'_, Option<SetupState>> {
        self.state.lock().unwrap()
    }

    /// 取活跃状态标量面。内存态不在（gateway 重启）时从 draft.json 重建：
    /// pid/模块基址未知 → 0——文件面步骤（annotate/struct/tool/commit）照常
    /// 可用；快照/差分依赖真实基址，见 [`Self::require_live_info`]。
    fn require_info(&self, workspace: &Path) -> Result<SetupInfo, String> {
        if let Some(st) = self.state.lock().unwrap().as_ref() {
            return Ok(st.info.clone());
        }
        let id = latest_draft_id(workspace).ok_or("无活跃配置向导：先 select_target")?;
        let profile = load_draft(workspace, &id)?;
        Ok(SetupInfo {
            exe_path: profile
                .target
                .exe_path_hint
                .clone()
                .map(PathBuf::from)
                .unwrap_or_default(),
            exe_name: profile.target.exe_name.clone(),
            file_sha256: profile.target.file_sha256.clone().unwrap_or_default(),
            machine: profile.target.machine,
            profile_id: id,
            pid: 0,
            module_base: 0,
            module_size: 0,
        })
    }

    /// 快照/差分专用：还要求模块基址真实（gateway 重启后基址可能因 ASLR
    /// 变化——旧值不可信，必须重新 select_target）。
    #[cfg(windows)]
    fn require_live_info(&self, workspace: &Path) -> Result<SetupInfo, String> {
        let info = self.require_info(workspace)?;
        if info.pid == 0 || info.module_base == 0 {
            return Err(
                "向导内存态不在（gateway 重启后 ASLR 基址不可信）：重新 select_target 后再快照/差分"
                    .to_string(),
            );
        }
        Ok(info)
    }

    // ── Step 1：select_target ───────────────────────────────────────────

    /// 选目标进程：定位主模块 → 读盘上 exe 指纹 → 建 draft → attach 会话。
    /// `pid` 与 `exe_hint`（exe 文件名，大小写不敏感）二选一。
    pub async fn select_target(
        &self,
        workspace: &Path,
        pid: Option<u32>,
        exe_hint: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        #[cfg(not(windows))]
        {
            let _ = (pid, exe_hint, workspace);
            Err("配置期目标选择仅 Windows 支持（payload/注入链未编译）".to_string())
        }
        #[cfg(windows)]
        {
            if self.state.lock().unwrap().is_some() {
                return Err("已有活跃向导 draft（先 commit 或 abort）".to_string());
            }
            if let Some(cur) = latest_draft_id(workspace) {
                return Err(format!(
                    "存在未收尾的 draft {cur}：先 commit/abort（CLI ziwei setup abort）后再开新向导"
                ));
            }
            let pid = match (pid, exe_hint) {
                (Some(p), _) => p,
                (None, Some(name)) => find_pid_by_name(name)?,
                (None, None) => return Err("缺 pid 或 exe（二选一）".to_string()),
            };
            let (base, size, path, _name) = main_module(pid)?;

            // 盘上 exe：指纹素材 + 节表真相源
            let exe_path = PathBuf::from(&path);
            let file = std::fs::read(&exe_path)
                .map_err(|e| format!("读 exe 失败（{}）：{e}", exe_path.display()))?;
            let pe = PeInfo::parse(&file).map_err(|e| format!("exe PE 头解析失败：{e}"))?;
            let machine = match pe.machine {
                0x8664 => Machine::X64,
                0x14C => Machine::X86,
                m => return Err(format!("machine 0x{m:04X} 不支持（只支持 x64/x86）")),
            };
            let mut hasher = sha2::Sha256::new();
            hasher.update(&file);
            let sha = hex::encode(hasher.finalize());
            let exe_name = exe_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if exe_name.is_empty() {
                return Err(format!("exe 路径无文件名：{}", exe_path.display()));
            }

            // draft profile（中途形态直落盘，不走 ProfileStore::save 的全量校验）
            let now = chrono::Utc::now();
            let id = format!("zw-pro-d{}", now.timestamp_millis());
            let profile = Profile {
                schema_version: SCHEMA_VERSION,
                profile_id: id.clone(),
                name: format!("draft: {exe_name}"),
                created_at: now,
                last_verified_at: now,
                status: ProfileStatus::Active,
                target: TargetFingerprint {
                    exe_name: exe_name.clone(),
                    exe_path_hint: Some(exe_path.display().to_string()),
                    machine,
                    file_sha256: Some(sha.clone()),
                    version_resource: Default::default(),
                    verify_policy: VerifyPolicy::HashOrVersion,
                },
                attach: AttachInfo::default(),
                functions: Vec::new(),
                data_structures: Vec::new(),
                tools: Vec::new(),
                verification: Verification {
                    on_attach: vec!["file_sha256".into(), "anchors_vote>=2".into()],
                    stale_action: Some("notify_user".into()),
                },
            };
            let draft = draft_dir(workspace, &id);
            std::fs::create_dir_all(&draft).map_err(|e| format!("建 draft 目录失败：{e}"))?;
            save_draft(workspace, &profile)?;
            save_draft_anchors(
                workspace,
                &AnchorSet {
                    profile_id: id.clone(),
                    anchors: Vec::new(),
                },
            )?;

            // attach 会话（指纹校验在 attach 链内；失败则回滚 draft）
            let manager = crate::session::global().ok_or_else(|| {
                let _ = nemesis_utils::fsutil::remove_dir_all_retry(&draft);
                "紫微会话管理器未装配（gateway 未启用或非 Windows 平台）".to_string()
            })?;
            let session = match manager.attach(&profile, pid).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&draft);
                    return Err(format!("attach 失败（draft 已回滚）：{e}"));
                }
            };

            let info = SetupInfo {
                profile_id: id.clone(),
                pid,
                module_base: base,
                module_size: size,
                machine,
                exe_path,
                exe_name: exe_name.clone(),
                file_sha256: sha,
            };
            *self.state_mut() = Some(SetupState {
                info: info.clone(),
                analysis: None,
            });
            tracing::info!(
                "[ziwei][setup] draft {id} 选定目标 pid={pid} base=0x{base:X} {exe_name}（{}）",
                session.arch
            );
            Ok(json!({
                "draft": info.to_json(false),
                "session": session.snapshot(),
                "next": "snapshot",
            }))
        }
    }

    // ── Step 2：snapshot ────────────────────────────────────────────────

    /// 内存快照：头页 + .text/.rdata/.data 按 ≤1MB 分块读（MAX_FRAME 留余）。
    pub async fn snapshot(&self, workspace: &Path) -> Result<serde_json::Value, String> {
        #[cfg(not(windows))]
        {
            let _ = workspace;
            Err("内存快照仅 Windows 支持（payload 管道未编译）".to_string())
        }
        #[cfg(windows)]
        {
            let info = self.require_live_info(workspace)?;
            let manager =
                crate::session::global().ok_or("紫微会话管理器未装配（gateway 未启用）")?;
            let session = manager
                .get(&info.profile_id)
                .ok_or("会话未就绪（select_target 的 attach 已失效？重新 select_target）")?;

            // 节表从盘上 exe 取（vsize 是内存布局的真实上限）
            let file = std::fs::read(&info.exe_path).map_err(|e| e.to_string())?;
            let pe = PeInfo::parse(&file).map_err(|e| e.to_string())?;
            let headers = read_addr(&session, info.module_base, HEADERS_BYTES).await?;
            let mut blobs: Vec<(String, Vec<u8>)> = Vec::new();
            let mut truncated: Vec<String> = Vec::new();
            for sec in &pe.sections {
                // 只取常驻三节（其余节按需后补）
                if !matches!(sec.name.as_str(), ".text" | ".rdata" | ".data") || sec.vsize == 0 {
                    continue;
                }
                let want = (sec.vsize as u64).min(SECTION_CAP);
                let bytes = read_addr(&session, info.module_base + sec.rva as u64, want).await?;
                if (bytes.len() as u64) < sec.vsize as u64 {
                    truncated.push(format!(
                        "{}: 读到 {}/{} 字节（上限或读失败截断）",
                        sec.name,
                        bytes.len(),
                        sec.vsize
                    ));
                }
                blobs.push((sec.name.clone(), bytes));
            }
            if blobs.is_empty() {
                return Err("快照为空：目标模块无可读的 .text/.rdata/.data 节".to_string());
            }

            let meta = SnapshotMeta {
                profile_id: info.profile_id.clone(),
                ts_rfc3339: chrono::Utc::now().to_rfc3339(),
                pid: info.pid,
                module_base: info.module_base,
                machine: pe.machine,
                exe_path: info.exe_path.display().to_string(),
                sections: pe.sections.clone(),
            };
            let ts = chrono::Utc::now().timestamp_millis().max(0) as u64;
            let store = SnapshotStore::new(workspace, &info.profile_id);
            let dir = store
                .save(&meta, &headers, &blobs, ts)
                .map_err(|e| e.to_string())?;
            let sweep = store.sweep(SNAP_TTL, SNAP_MAX_KEPT);
            tracing::info!(
                "[ziwei][setup] 快照落盘 {}（{} 节，清扫 expired={} kept={}）",
                dir.display(),
                blobs.len(),
                sweep.expired,
                sweep.kept
            );
            Ok(json!({
                "snapshot": dir.display().to_string(),
                "sections": blobs.iter().map(|(n, b)| json!({
                    "name": n, "bytes": b.len(),
                })).collect::<Vec<_>>(),
                "headers_bytes": headers.len(),
                "truncated": truncated,
                "sweep": { "expired": sweep.expired, "kept": sweep.kept },
                "next": "analyze",
            }))
        }
    }

    // ── Step 3：analyze ─────────────────────────────────────────────────

    /// 分析：最新快照 → 递归下降恢复函数 → xref → CRT 折叠 → 函数表落盘。
    /// 产物缓存进内存（candidates 消费）+ analysis.json（向导展示面）。
    /// 幂等：重复 analyze 重算并覆盖缓存。
    pub fn analyze(&self, workspace: &Path) -> Result<serde_json::Value, String> {
        let info = self.require_info(workspace)?;
        let store = SnapshotStore::new(workspace, &info.profile_id);
        let (meta, headers, section_blobs) = store
            .latest()
            .map_err(|e| e.to_string())?
            .ok_or("无快照：先 snapshot")?;
        let pe = PeInfo::parse(&headers).map_err(|e| format!("快照头解析失败：{e}"))?;
        let arch = Arch::from_machine(pe.machine)
            .ok_or_else(|| format!("machine 0x{:04X} 无法反汇编", pe.machine))?;
        let img = ImageMap::new(pe, section_blobs).map_err(|e| e.to_string())?;

        // 种子：入口 + 导出函数（落在可执行节内的）
        let text = img
            .pe
            .sections
            .iter()
            .find(|s| s.is_executable())
            .ok_or("无可执行节（.text）")?;
        let bounds = (text.rva, text.rva.saturating_add(text.vsize));
        let mut seeds: Vec<u32> = Vec::new();
        if img.pe.entry_rva >= bounds.0 && img.pe.entry_rva < bounds.1 {
            seeds.push(img.pe.entry_rva);
        }
        let exe_file = std::fs::read(&info.exe_path).map_err(|e| e.to_string())?;
        let exports = pe::parse_exports_file(&exe_file, &img.pe).unwrap_or_default();
        for e in &exports {
            if e.rva >= bounds.0 && e.rva < bounds.1 {
                seeds.push(e.rva);
            }
        }
        // x64：.pdata 异常目录 = 函数起点权威表（用户 main 经 std
        // lang_start 间接进入，递归下降够不到）；x86 无 .pdata → 预埋
        // nopsled 标记扫描兜底（夹具契约：锚点函数入口 8×0x90）。
        if let Some(pdata) = pe::parse_pdata_file(&exe_file, &img.pe) {
            for (start, _end) in pdata {
                if start >= bounds.0 && start < bounds.1 {
                    seeds.push(start);
                }
            }
        }
        for rva in funcs::nopsled_seed_rvas(&img) {
            if rva >= bounds.0 && rva < bounds.1 {
                seeds.push(rva);
            }
        }
        seeds.sort_unstable();
        seeds.dedup();
        if seeds.is_empty() {
            return Err("无分析种子（入口与导出都不在可执行节内）".to_string());
        }

        let set = funcs::recover(arch, &img, &seeds, bounds);
        let imports = pe::parse_imports_file(&exe_file, &img.pe).unwrap_or_default();
        let idx = xref::build(&img, &set, &imports);
        let crt = crtsigs::fold(&img, &set, &imports);

        let rows = nemesis_ziwei_rev::package::function_table(&set, &idx, &crt, 128);
        let table = function_table_json(&rows);
        let mut summary = json!({
            "profile_id": info.profile_id,
            "machine": format!("0x{:04X}", img.pe.machine),
            "arch": match arch { Arch::X64 => "x64", Arch::X86 => "x86" },
            "seeds": seeds.iter().map(|r| format!("0x{r:X}")).collect::<Vec<_>>(),
            "functions": set.functions.len(),
            "crt_folded": crt.len(),
            "import_dlls": imports.len(),
            "strings": idx.str_refs.len(),
            "snapshot_ts": meta.ts_rfc3339,
            "table": table,
        });
        // T3.6 LLM 建议标注：hook 已装才尝试（gateway ctx.rs 装桥）。
        // 仅建议不落库——候选与注释进 profile 的唯一通路 = 显式 annotate；
        // 失败诚实注记，分析照常成功。
        if let Some(hook) = nemesis_ziwei_rev::package::llm_hook() {
            let arch_label = match arch {
                Arch::X64 => "x64",
                Arch::X86 => "x86",
            };
            let prompt = nemesis_ziwei_rev::package::build_prompt(
                &info.exe_name,
                arch_label,
                &rows,
                &seeds,
                12_288,
            );
            match hook(&prompt) {
                Ok(reply) => {
                    let sugg: Vec<serde_json::Value> =
                        nemesis_ziwei_rev::package::parse_annotations(&reply)
                            .iter()
                            .map(|a| {
                                json!({
                                    "rva": format!("0x{:X}", a.rva),
                                    "name": a.name,
                                    "desc": a.desc,
                                })
                            })
                            .collect();
                    if let Some(obj) = summary.as_object_mut() {
                        obj.insert("llm_suggestions".into(), json!(sugg));
                    }
                }
                Err(e) => {
                    if let Some(obj) = summary.as_object_mut() {
                        obj.insert("llm_suggest_error".into(), json!(e));
                    }
                }
            }
        }
        std::fs::write(
            draft_dir(workspace, &info.profile_id).join("analysis.json"),
            serde_json::to_vec_pretty(&summary).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("analysis.json 落盘失败：{e}"))?;

        // 缓存进状态（保留 pid 等；重建即覆盖旧缓存）
        let mut guard = self.state_mut();
        if let Some(st) = guard.as_mut() {
            st.info = info.clone();
            st.analysis = Some(AnalysisCache {
                img,
                set,
                idx,
                crt,
                imports,
                arch,
            });
        }
        let analysis_path = draft_dir(workspace, &info.profile_id)
            .join("analysis.json")
            .display()
            .to_string();
        let mut out = json!({
            "functions": summary["functions"],
            "crt_folded": summary["crt_folded"],
            "import_dlls": summary["import_dlls"],
            "strings": summary["strings"],
            "analysis_json": analysis_path,
            "next": "candidates / diff_window / annotate",
        });
        for key in ["llm_suggestions", "llm_suggest_error"] {
            if let Some(v) = summary.get(key)
                && let Some(o) = out.as_object_mut()
            {
                o.insert(key.into(), v.clone());
            }
        }
        Ok(out)
    }

    /// 分析缓存闭包消费（缺席则先跑 analyze——向导步骤可跳步的诚实实现）。
    fn with_analysis<T>(
        &self,
        workspace: &Path,
        f: impl FnOnce(&AnalysisCache) -> T,
    ) -> Result<T, String> {
        {
            let guard = self.state.lock().unwrap();
            if let Some(cache) = guard.as_ref().and_then(|s| s.analysis.as_ref()) {
                return Ok(f(cache));
            }
        }
        self.analyze(workspace)?;
        let guard = self.state.lock().unwrap();
        let cache = guard
            .as_ref()
            .and_then(|s| s.analysis.as_ref())
            .ok_or_else(|| "分析缓存仍缺席（analyze 异常）".to_string())?;
        Ok(f(cache))
    }

    // ── Step 4a：candidates ─────────────────────────────────────────────

    /// 定位候选清单（nopsled / 独特立即数 / 字符串引用 / 导入调用序列）。
    pub fn candidates(&self, workspace: &Path) -> Result<serde_json::Value, String> {
        let info = self.require_info(workspace)?;
        let list = self.with_analysis(workspace, |cache| {
            candidates::generate(
                &cache.img,
                &cache.set,
                &cache.idx,
                &cache.crt,
                CANDIDATE_CAP,
            )
        })?;
        Ok(json!({
            "profile_id": info.profile_id,
            "count": list.len(),
            "candidates": list,
            "note": "候选均可作锚：annotate 传 anchors:[{kind:..., ...payload}]",
        }))
    }

    // ── Step 4b：diff_window ────────────────────────────────────────────

    /// 差分窗口：各节当前内存 vs 最新快照（等待 `wait_secs` 秒后取现值）。
    /// 可写节（.data）的页单独归入 data_pages（DataArea 线索）。
    /// T4.4 增强：`hook=true` 时观测窗并入临时 hook 计数——draft 已标注
    /// 函数逐个武装，窗末拉取命中数并退订（无函数/锚未命中诚实注记不阻断）。
    pub async fn diff_window(
        &self,
        workspace: &Path,
        wait_secs: Option<u64>,
        hook: Option<bool>,
    ) -> Result<serde_json::Value, String> {
        #[cfg(not(windows))]
        {
            let _ = (workspace, wait_secs, hook);
            Err("差分窗口仅 Windows 支持（payload 管道未编译）".to_string())
        }
        #[cfg(windows)]
        {
            let info = self.require_live_info(workspace)?;
            let wait = wait_secs.unwrap_or(2).min(60);
            let manager =
                crate::session::global().ok_or("紫微会话管理器未装配（gateway 未启用）")?;
            let session = manager
                .get(&info.profile_id)
                .ok_or("会话未就绪（重新 select_target）")?;
            let store = SnapshotStore::new(workspace, &info.profile_id);
            let (meta, _headers, blobs) = store
                .latest()
                .map_err(|e| e.to_string())?
                .ok_or("无快照：先 snapshot")?;

            // T4.4：观测窗前武装临时 hook（已标注函数；失败诚实注记不阻断）
            let hook_on = hook.unwrap_or(false);
            let mut hook_notes: Vec<String> = Vec::new();
            let mut armed: Vec<String> = Vec::new();
            if hook_on {
                match crate::profile::ProfileStore::new(workspace).load(&info.profile_id) {
                    Ok(profile) if !profile.functions.is_empty() => {
                        if let Ok(rep) = manager.resolve_functions(&info.profile_id).await {
                            for n in rep["notes"].as_array().cloned().unwrap_or_default() {
                                if let Some(s) = n.as_str() {
                                    hook_notes.push(s.to_string());
                                }
                            }
                        }
                        for f in &profile.functions {
                            match manager.hook_start(&info.profile_id, &f.semantic).await {
                                Ok(_) => armed.push(f.semantic.clone()),
                                Err(e) => hook_notes.push(format!("{}：{e}", f.semantic)),
                            }
                        }
                        if armed.is_empty() {
                            hook_notes.push("无函数武装成功（锚未命中或解析失败）".to_string());
                        }
                    }
                    Ok(_) => hook_notes
                        .push("draft 尚无已标注函数（先 annotate 再 hook 计数）".to_string()),
                    Err(e) => hook_notes.push(format!("draft profile 不可用：{e}")),
                }
            }

            if wait > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            }

            // T4.4：窗末拉取命中计数 + 逐个退订
            let mut hook_counts: Vec<serde_json::Value> = Vec::new();
            if hook_on {
                let bindings = session.hooks();
                if let Ok(pull) = manager.poll_captures(&info.profile_id).await {
                    let mut hits: std::collections::HashMap<u32, u64> =
                        std::collections::HashMap::new();
                    if let Some(evs) = pull["events"].as_array() {
                        for ev in evs {
                            if let Some(id) = ev["id"].as_u64() {
                                *hits.entry(id as u32).or_insert(0) += 1;
                            }
                        }
                    }
                    for sem in &armed {
                        let binding = bindings.iter().find(|b| b.semantic == *sem);
                        hook_counts.push(json!({
                            "semantic": sem,
                            "hook_id": binding.map(|b| b.id),
                            "hits": binding.and_then(|b| hits.get(&b.id).copied()).unwrap_or(0),
                        }));
                    }
                } else {
                    hook_notes.push("事件拉取失败（命中计数不可得）".to_string());
                }
                for sem in &armed {
                    if let Err(e) = manager.hook_stop(&info.profile_id, sem).await {
                        hook_notes.push(format!("退订失败 {sem}：{e}"));
                    }
                }
            }

            let mut pages: Vec<serde_json::Value> = Vec::new();
            let mut truncated = false;
            for (name, before) in &blobs {
                let sec = match meta.sections.iter().find(|s| &s.name == name) {
                    Some(i) => i,
                    None => continue,
                };
                let current = read_addr(
                    &session,
                    info.module_base + sec.rva as u64,
                    before.len() as u64,
                )
                .await?;
                for pg in diff::page_diff(before, &current, sec.rva, 0x1000) {
                    if pages.len() >= DIFF_PAGE_CAP {
                        truncated = true;
                        break;
                    }
                    pages.push(json!({
                        "section": name,
                        "rva": format!("0x{:X}", pg.rva),
                        "bytes_changed": pg.bytes_changed,
                        "ranges": pg.ranges.iter().map(|(o, l)| json!(format!("0x{o:X}+{l}"))).collect::<Vec<_>>(),
                        "writable": sec.is_writable(),
                    }));
                }
                if truncated {
                    break;
                }
            }
            let data_pages: Vec<_> = pages
                .iter()
                .filter(|p| p["writable"].as_bool() == Some(true))
                .cloned()
                .collect();
            let mut result = json!({
                "profile_id": info.profile_id,
                "wait_secs": wait,
                "pages_changed": pages.len(),
                "truncated": truncated,
                "pages": pages,
                "data_pages": data_pages,
                "note": "data_pages 是 DataArea 定位线索：周期性变化的可写页通常是状态/记录区",
            });
            if hook_on && let Some(o) = result.as_object_mut() {
                o.insert("hook_counts".into(), json!(hook_counts));
                o.insert("hook_notes".into(), json!(hook_notes));
            }
            Ok(result)
        }
    }

    // ── Step 5a：annotate ───────────────────────────────────────────────

    /// 标注函数 + 强制锚（≥1）：落 draft.functions + anchors.json。
    /// data：{rva, semantic, conv?, note?, anchors:[{kind, pattern?/text?/
    /// value?/imports?/distance?/note?}]}；函数 locate 取首个 byte_pattern
    /// 锚，无则退 RVA 定位（锚照记）。级联注记：函数级 note 补进无注记的锚。
    pub fn annotate(
        &self,
        workspace: &Path,
        data: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let info = self.require_info(workspace)?;
        let rva_u = data
            .get("rva")
            .and_then(|v| v.as_u64())
            .ok_or("缺 rva（u64）")?;
        if rva_u > u32::MAX as u64 {
            return Err(format!("rva 0x{rva_u:X} 超出 32 位"));
        }
        let rva = rva_u as u32;
        let semantic = data
            .get("semantic")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("缺 semantic（函数语义名）")?
            .to_string();
        if semantic.len() > 64 {
            return Err("semantic 过长（≤64 字符）".to_string());
        }
        let conv = match data.get("conv").and_then(|v| v.as_str()) {
            None | Some("ms_x64") => Conv::MsX64,
            Some("stdcall") => Conv::Stdcall,
            Some("cdecl") => Conv::Cdecl,
            Some(other) => return Err(format!("conv {other:?} 不支持（ms_x64/stdcall/cdecl）")),
        };
        let fn_note = data
            .get("note")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let anchors_in = data
            .get("anchors")
            .and_then(|v| v.as_array())
            .ok_or("缺 anchors（≥1 条——标注强制带锚）")?;
        if anchors_in.is_empty() {
            return Err("anchors 为空：标注必须带 ≥1 条锚（byte_pattern/str_ref/imm_const/import_call_seq）".to_string());
        }

        let mut profile = load_draft(workspace, &info.profile_id)?;
        if profile.functions.iter().any(|f| f.semantic == semantic) {
            return Err(format!(
                "函数 {semantic:?} 已标注（v1 不做覆盖；换语义名或 abort 重来）"
            ));
        }
        if profile
            .functions
            .iter()
            .any(|f| f.locate.rva == Some(rva_u))
        {
            return Err(format!("RVA 0x{rva:X} 已被其他函数占用"));
        }

        let mut anchor_set = load_draft_anchors(workspace, &info.profile_id)?;
        let mut locate: Option<LocateSpec> = None;
        let mut converted: Vec<Anchor> = Vec::new();
        for (i, a) in anchors_in.iter().enumerate() {
            let kind_str = a
                .get("kind")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("anchors[{i}] 缺 kind"))?;
            let kind = match kind_str {
                "byte_pattern" => AnchorType::BytePattern,
                "str_ref" => AnchorType::StrRef,
                "imm_const" => AnchorType::ImmConst,
                "import_call_seq" => AnchorType::ImportCallSeq,
                "bb_hash" => return Err("bb_hash 锚 v1 未实装（M5 找回期补）".to_string()),
                other => return Err(format!("anchors[{i}] kind {other:?} 未知")),
            };
            let mut anchor = Anchor {
                function: semantic.clone(),
                kind,
                module: Some(info.exe_name.clone()),
                pattern: a
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                text: a.get("text").and_then(|v| v.as_str()).map(str::to_string),
                distance: a.get("distance").and_then(|v| v.as_u64()).map(|v| v as u32),
                value: a.get("value").and_then(|v| v.as_u64()),
                imports: a
                    .get("imports")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                bb_hash: None,
                note: a.get("note").and_then(|v| v.as_str()).map(str::to_string),
            };
            // 按类型校验载荷（parse_byte_pattern 同时归一化十六进制形态）
            match kind {
                AnchorType::BytePattern => {
                    let pat = anchor
                        .pattern
                        .as_deref()
                        .ok_or_else(|| format!("anchors[{i}] byte_pattern 缺 pattern"))?;
                    parse_byte_pattern(pat)
                        .map_err(|e| format!("anchors[{i}] pattern 非法：{e}"))?;
                    anchor.pattern = Some(normalize_pattern(pat));
                    if locate.is_none() {
                        locate = Some(LocateSpec {
                            kind: LocateKind::BytePattern,
                            module: Some(info.exe_name.clone()),
                            pattern: anchor.pattern.clone(),
                            // 发现地 RVA 作提示一并记录（M5 找回以 pattern 为准；
                            // rva 字段让重复标注检查有据可依）
                            rva: Some(rva_u),
                        });
                    }
                }
                AnchorType::StrRef => {
                    if anchor.text.as_deref().map(str::is_empty) != Some(false) {
                        return Err(format!("anchors[{i}] str_ref 缺 text"));
                    }
                }
                AnchorType::ImmConst => {
                    if anchor.value.is_none() {
                        return Err(format!("anchors[{i}] imm_const 缺 value"));
                    }
                }
                AnchorType::ImportCallSeq => {
                    if anchor.imports.as_deref().map(str::is_empty) != Some(false) {
                        return Err(format!("anchors[{i}] import_call_seq 缺 imports"));
                    }
                }
                AnchorType::BbHash => unreachable!("上方已拒绝"),
            }
            if anchor.note.is_none() {
                anchor.note = fn_note.clone(); // 函数级注记级联
            }
            converted.push(anchor);
        }
        if converted.is_empty() {
            return Err("没有可用的锚（全部转换失败）".to_string());
        }

        profile.functions.push(nemesis_ziwei_profile::FunctionDef {
            semantic: semantic.clone(),
            locate: locate.unwrap_or(LocateSpec {
                kind: LocateKind::Rva,
                module: Some(info.exe_name.clone()),
                pattern: None,
                rva: Some(rva_u),
            }),
            conv,
            args: Vec::new(),
            capture: None,
        });
        // 轻校验（全量校验留 commit；这里即时拦手误）
        profile
            .validate()
            .map_err(|e| format!("标注后 draft 校验失败：{e}"))?;
        save_draft(workspace, &profile)?;
        anchor_set.anchors.extend(converted);
        save_draft_anchors(workspace, &anchor_set)?;
        tracing::info!(
            "[ziwei][setup] draft {} 标注 {}@0x{rva:X}（锚累计 {}）",
            info.profile_id,
            semantic,
            anchor_set.anchors.len()
        );
        Ok(json!({
            "annotated": semantic,
            "rva": format!("0x{rva:X}"),
            "functions": profile.functions.len(),
            "anchors": anchor_set.anchors.len(),
            "next": "继续 annotate / define_struct / define_tool 后 commit",
        }))
    }

    // ── Step 5b：define_struct ──────────────────────────────────────────

    /// 定义/覆盖数据结构：data = {name, size, fields:[{offset,name,type,decode?}]}。
    pub fn define_struct(
        &self,
        workspace: &Path,
        data: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let info = self.require_info(workspace)?;
        let name = data
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("缺 name")?
            .to_string();
        let size = data
            .get("size")
            .and_then(|v| v.as_u64())
            .ok_or("缺 size（字节）")? as u32;
        if size == 0 || size > 1 << 20 {
            return Err(format!("size {size} 越界（1B..1MB）"));
        }
        let fields_in = data
            .get("fields")
            .and_then(|v| v.as_array())
            .ok_or("缺 fields")?;
        let mut fields = Vec::with_capacity(fields_in.len());
        for (i, f) in fields_in.iter().enumerate() {
            let offset = f
                .get("offset")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| format!("fields[{i}] 缺 offset"))? as u32;
            let fname = f
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("fields[{i}] 缺 name"))?
                .to_string();
            let ty_raw = f
                .get("type")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("fields[{i}] 缺 type"))?;
            let type_ = nemesis_ziwei_profile::FieldType::parse(ty_raw).ok_or_else(|| {
                format!("fields[{i}] type {ty_raw:?} 无法解析（如 u32/f64/char[16]）")
            })?;
            if offset as u64 + type_.size_of() as u64 > size as u64 {
                return Err(format!(
                    "fields[{i}] {} 越界（offset {offset} + {} > size {size}）",
                    fname,
                    type_.size_of()
                ));
            }
            let decode = match f.get("decode").and_then(|v| v.as_str()) {
                None => None,
                Some("utf8") => Some(nemesis_ziwei_profile::FieldDecode::Utf8),
                Some("utf16") => Some(nemesis_ziwei_profile::FieldDecode::Utf16),
                Some("unix_ms") => Some(nemesis_ziwei_profile::FieldDecode::UnixMs),
                Some("unix_sec") => Some(nemesis_ziwei_profile::FieldDecode::UnixSec),
                Some("hex") => Some(nemesis_ziwei_profile::FieldDecode::Hex),
                Some(other) => return Err(format!("fields[{i}] decode {other:?} 未知")),
            };
            fields.push(FieldDef {
                offset,
                name: fname,
                type_,
                decode,
            });
        }
        let ds = DataStructure {
            name: name.clone(),
            size,
            fields,
        };
        let mut profile = load_draft(workspace, &info.profile_id)?;
        match profile.data_structures.iter_mut().find(|s| s.name == name) {
            Some(slot) => *slot = ds, // 覆盖式重定义（向导迭代语义）
            None => profile.data_structures.push(ds),
        }
        profile
            .validate()
            .map_err(|e| format!("结构定义校验失败：{e}"))?;
        save_draft(workspace, &profile)?;
        Ok(json!({
            "struct": name,
            "structs": profile.data_structures.len(),
            "next": "继续 define_struct / annotate / define_tool 后 commit",
        }))
    }

    // ── Step 5c：define_tool（draft 侧工具定义；commit 前任意次）─────────

    /// 定义固化子工具：data = {tool_name, description, action:{primitive,
    /// addr, mode?, interval_secs?, max_records?, timeout_ms?, notify?},
    /// output?{dest}, run_policy?}。run_policy：call_fn 默认 confirm（决议 1），
    /// poll/hook 默认 auto。
    pub fn define_tool(
        &self,
        workspace: &Path,
        data: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        use nemesis_ziwei_profile::{
            NotifyRule, OutputSpec, PollMode, Primitive, RunPolicy, ToolAction, ToolDef,
        };
        let info = self.require_info(workspace)?;
        let tool_name = data
            .get("tool_name")
            .and_then(|v| v.as_str())
            .ok_or("缺 tool_name")?
            .to_string();
        let description = data
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let action_v = data.get("action").ok_or("缺 action")?;
        let primitive = match action_v.get("primitive").and_then(|v| v.as_str()) {
            Some("poll_report") => Primitive::PollReport,
            Some("call_fn") => Primitive::CallFn,
            Some("hook_capture") => Primitive::HookCapture,
            Some(other) => return Err(format!("primitive {other:?} 未知")),
            None => return Err("action 缺 primitive".to_string()),
        };
        let addr = action_v
            .get("addr")
            .and_then(|v| v.as_str())
            .ok_or("action 缺 addr（struct:Name / fn:语义名）")?
            .to_string();
        let mode = match action_v.get("mode").and_then(|v| v.as_str()) {
            None => None,
            Some("on_change") => Some(PollMode::OnChange),
            Some("every") => Some(PollMode::Every),
            Some(other) => return Err(format!("mode {other:?} 未知（on_change/every）")),
        };
        let notify = match action_v.get("notify") {
            None => None,
            Some(n) => Some(
                serde_json::from_value::<NotifyRule>(n.clone())
                    .map_err(|e| format!("notify 非法：{e}"))?,
            ),
        };
        let action = ToolAction {
            primitive,
            addr,
            mode,
            interval_secs: action_v.get("interval_secs").and_then(|v| v.as_u64()),
            max_records: action_v
                .get("max_records")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32),
            timeout_ms: action_v.get("timeout_ms").and_then(|v| v.as_u64()),
            notify,
        };
        let run_policy = match data.get("run_policy").and_then(|v| v.as_str()) {
            None if primitive == Primitive::CallFn => RunPolicy::Confirm,
            None => RunPolicy::Auto,
            Some("auto") => RunPolicy::Auto,
            Some("confirm") => RunPolicy::Confirm,
            Some(other) => return Err(format!("run_policy {other:?} 未知")),
        };
        let output = data
            .get("output")
            .and_then(|v| v.get("dest"))
            .and_then(|v| v.as_str())
            .map(|dest| OutputSpec {
                dest: dest.to_string(),
            });
        let tool = ToolDef {
            tool_name: tool_name.clone(),
            description,
            input_schema: data
                .get("input_schema")
                .cloned()
                .unwrap_or(serde_json::Value::Object(Default::default())),
            action,
            output,
            run_policy,
        };
        let mut profile = load_draft(workspace, &info.profile_id)?;
        match profile
            .tools
            .iter_mut()
            .find(|t| t.tool_name == tool.tool_name)
        {
            Some(slot) => *slot = tool, // 覆盖式重定义
            None => profile.tools.push(tool),
        }
        profile
            .validate()
            .map_err(|e| format!("工具定义校验失败：{e}"))?;
        save_draft(workspace, &profile)?;
        Ok(json!({
            "tool": tool_name,
            "tools": profile.tools.len(),
            "next": "commit",
        }))
    }

    // ── Step 6：commit ──────────────────────────────────────────────────

    /// 提交：全量校验 → draft → profiles/（ProfileStore 写盘）→ setup 快照
    /// 迁移到 profile 名下 → 删 draft 目录。
    /// 工具重注册钩子由调用方（WSAPI/CLI handler）触发。
    pub fn commit(&self, workspace: &Path) -> Result<serde_json::Value, String> {
        let info = self.require_info(workspace)?;
        let profile = load_draft(workspace, &info.profile_id)?;
        let anchor_set = load_draft_anchors(workspace, &info.profile_id)?;
        if profile.functions.is_empty() {
            return Err("draft 无已标注函数：commit 前至少 annotate 一个函数（带锚）".to_string());
        }
        // 全量校验（ProfileStore::save 内部还会再跑一次；这里提前给出可读错误）
        profile
            .validate()
            .map_err(|e| format!("profile 校验失败：{e}"))?;

        let store = ProfileStore::new(workspace);
        store
            .save(&profile, Some(&anchor_set))
            .map_err(|e| format!("profile 落盘失败：{e}"))?;

        // setup 快照迁到 profile 名下（M5 找回消费）
        let setup_dir = workspace.join("ziwei").join("setup").join(&info.profile_id);
        let setup_snaps = setup_dir.join("snapshots");
        let target_snaps = store.snapshots_dir(&info.profile_id);
        if setup_snaps.is_dir() {
            let _ = std::fs::create_dir_all(&target_snaps);
            if let Err(e) = std::fs::rename(&setup_snaps, &target_snaps) {
                tracing::warn!(
                    "[ziwei][setup] 快照迁移失败（{} → {}）：{e}；setup 目录保留",
                    setup_snaps.display(),
                    target_snaps.display()
                );
            } else {
                let _ = nemesis_utils::fsutil::remove_dir_all_retry(&setup_dir);
            }
        }

        // draft 目录清除（anchors/profile 已在 profiles/ 落盘）
        let draft = draft_dir(workspace, &info.profile_id);
        let _ = nemesis_utils::fsutil::remove_dir_all_retry(&draft);
        *self.state_mut() = None;
        tracing::info!(
            "[ziwei][setup] draft {} 已提交为正式 profile（{} 函数 / {} 工具 / {} 锚）",
            info.profile_id,
            profile.functions.len(),
            profile.tools.len(),
            anchor_set.anchors.len()
        );
        Ok(json!({
            "committed": info.profile_id,
            "functions": profile.functions.len(),
            "tools": profile.tools.len(),
            "anchors": anchor_set.anchors.len(),
            "profile_dir": store.dir(&info.profile_id).display().to_string(),
        }))
    }

    /// 放弃向导：detach 会话 + 删 draft 与 setup 快照目录（幂等；
    /// gateway 重启后的孤儿 draft 也允许清理）。
    pub async fn abort(&self, workspace: &Path) -> Result<serde_json::Value, String> {
        let info = match self.state.lock().unwrap().as_ref() {
            Some(st) => st.info.clone(),
            None => match latest_draft_id(workspace) {
                Some(id) => SetupInfo {
                    profile_id: id,
                    pid: 0,
                    module_base: 0,
                    module_size: 0,
                    machine: Machine::X64,
                    exe_path: PathBuf::new(),
                    exe_name: String::new(),
                    file_sha256: String::new(),
                },
                None => return Err("无活跃向导".to_string()),
            },
        };
        if info.pid != 0
            && let Some(manager) = crate::session::global()
        {
            let _ = manager.detach(&info.profile_id).await; // 已卸载=幂等成功
        }
        let _ = nemesis_utils::fsutil::remove_dir_all_retry(draft_dir(workspace, &info.profile_id));
        let _ = nemesis_utils::fsutil::remove_dir_all_retry(
            workspace.join("ziwei").join("setup").join(&info.profile_id),
        );
        *self.state_mut() = None;
        tracing::info!(
            "[ziwei][setup] draft {} 已放弃（会话卸载 + 目录清理）",
            info.profile_id
        );
        Ok(json!({ "aborted": info.profile_id }))
    }
}

impl Default for SetupManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── 全局槽（gateway ctx.rs 装配；同 SessionManager 模式）────────────────

static SETUP_GLOBAL: OnceLock<std::sync::Arc<SetupManager>> = OnceLock::new();

/// 全局实例（惰性自建——SetupManager 无装配依赖；WSAPI/CLI/bridge 共享
/// 同一向导状态）。
pub fn global() -> std::sync::Arc<SetupManager> {
    SETUP_GLOBAL
        .get_or_init(|| std::sync::Arc::new(SetupManager::new()))
        .clone()
}

// ── draft 文件 IO ───────────────────────────────────────────────────────

/// draft 目录：`<workspace>/ziwei/drafts/<profile_id>/`。
fn draft_dir(workspace: &Path, profile_id: &str) -> PathBuf {
    workspace.join("ziwei").join("drafts").join(profile_id)
}

fn draft_json_path(workspace: &Path, profile_id: &str) -> PathBuf {
    draft_dir(workspace, profile_id).join("draft.json")
}

fn load_draft(workspace: &Path, profile_id: &str) -> Result<Profile, String> {
    let bytes = std::fs::read(draft_json_path(workspace, profile_id))
        .map_err(|e| format!("draft.json 读取失败（{profile_id}）：{e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("draft.json 解析失败：{e}"))
}

fn save_draft(workspace: &Path, profile: &Profile) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(profile).map_err(|e| e.to_string())?;
    std::fs::write(draft_json_path(workspace, &profile.profile_id), bytes)
        .map_err(|e| format!("draft.json 写盘失败：{e}"))
}

fn load_draft_anchors(workspace: &Path, profile_id: &str) -> Result<AnchorSet, String> {
    let p = draft_dir(workspace, profile_id).join("anchors.json");
    let bytes = std::fs::read(&p).map_err(|e| format!("anchors.json 读取失败：{e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("anchors.json 解析失败：{e}"))
}

fn save_draft_anchors(workspace: &Path, set: &AnchorSet) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(set).map_err(|e| e.to_string())?;
    std::fs::write(
        draft_dir(workspace, &set.profile_id).join("anchors.json"),
        bytes,
    )
    .map_err(|e| format!("anchors.json 写盘失败：{e}"))
}

/// 磁盘上现存 draft id（字典序最大者；同前缀数字段字典序=时间序）。无 = None。
fn latest_draft_id(workspace: &Path) -> Option<String> {
    let root = workspace.join("ziwei").join("drafts");
    let mut ids: Vec<String> = std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("zw-pro-d") && draft_json_path(workspace, n).exists())
        .collect();
    ids.sort(); // read_dir 顺序是平台细节——显式排序
    ids.pop()
}

// ── 辅助 ────────────────────────────────────────────────────────────────

/// byte_pattern 归一化（十六进制对大小写/分隔统一为 "AA BB ??" 空格形态）。
fn normalize_pattern(pat: &str) -> String {
    pat.split_whitespace()
        .map(|t| t.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}

/// 函数表（package::function_table 的 JSON 投影）。
fn function_table_json(rows: &[nemesis_ziwei_rev::package::FuncRow]) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|r| {
            json!({
                "rva": format!("0x{:X}", r.rva),
                "size": r.size,
                "insns": r.insns,
                "calls_out": r.calls_out,
                "imports": r.imports,
                "strings": r.strings,
                "crt_folded": r.crt_folded,
            })
        })
        .collect()
}

// ── 管道分块读（windows）────────────────────────────────────────────────

/// 分块读目标内存（≤1MB/帧；读不到/提前 EOF 诚实截断返回已有部分）。
#[cfg(windows)]
async fn read_addr(
    session: &crate::session::Session,
    addr: u64,
    len: u64,
) -> Result<Vec<u8>, String> {
    if len == 0 {
        return Ok(Vec::new());
    }
    let mut link = session.link().lock().await;
    let mut out = Vec::with_capacity(len.min(SECTION_CAP) as usize);
    let mut off = 0u64;
    while off < len {
        let n = (len - off).min(CHUNK_BYTES) as u32;
        let part = crate::pipe::read_mem(&mut link, addr + off, n)
            .await
            .map_err(|e| format!("读内存 0x{addr:X}+{off} 失败：{e}"))?;
        if part.is_empty() {
            break; // 读不到就停（诚实截断）
        }
        off += part.len() as u64;
        out.extend_from_slice(&part);
        if part.len() < n as usize {
            break;
        }
    }
    Ok(out)
}

// ── Toolhelp 目标定位（windows）────────────────────────────────────────

/// 主模块（exe）定位：(base, size, exe_path, module_name)。
/// Module32FirstW 首条即主可执行模块（Windows Toolhelp 语义）。
#[cfg(windows)]
pub(crate) fn main_module(pid: u32) -> Result<(u64, u32, String, String), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE,
        TH32CS_SNAPMODULE32,
    };

    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
        if snap == INVALID_HANDLE_VALUE {
            return Err(format!(
                "pid {pid} 模块快照失败（进程不存在或权限不足；64 位进程由 32 位 bot 枚举也会失败）"
            ));
        }
        let mut entry: MODULEENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        let mut result = Err("模块枚举为空".to_string());
        if Module32FirstW(snap, &mut entry) != 0 {
            let name = String::from_utf16_lossy(
                &entry.szModule[..entry.szModule.iter().position(|&c| c == 0).unwrap_or(0)],
            );
            let path = String::from_utf16_lossy(
                &entry.szExePath[..entry.szExePath.iter().position(|&c| c == 0).unwrap_or(0)],
            );
            result = Ok((entry.modBaseAddr as u64, entry.modBaseSize, path, name));
        }
        while Module32NextW(snap, &mut entry) != 0 {}
        let _ = CloseHandle(snap);
        result
    }
}

/// 按 exe 文件名找 pid（大小写不敏感；多实例取最小 pid 并诚实注记）。
#[cfg(windows)]
fn find_pid_by_name(exe_name: &str) -> Result<u32, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    let want = exe_name.to_lowercase();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Err("进程快照创建失败".to_string());
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found: Vec<u32> = Vec::new();
        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                let name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)],
                );
                if name.to_lowercase() == want {
                    found.push(entry.th32ProcessID);
                }
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        match found.len() {
            0 => Err(format!("未找到进程 {exe_name:?}")),
            1 => Ok(found[0]),
            _ => {
                found.sort_unstable();
                tracing::warn!(
                    "[ziwei][setup] {exe_name} 有 {} 个实例（{:?}）→ 取最小 pid",
                    found.len(),
                    found
                );
                Ok(found[0])
            }
        }
    }
}

// 测试与实现分离（内联测试防回归纪律）。
#[cfg(test)]
mod tests;
