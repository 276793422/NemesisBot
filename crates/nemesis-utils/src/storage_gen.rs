//! L15（2026-11-03 存储世代化迁移链骨架）：四条铁律的代码化模板。
//!
//! 设计文档：`docs/PLAN/2026-11-03_存储世代化迁移链_设计文档.md`（协议细节
//! 与中断恢复语义的真相源）。**零接线**——现行 chat_log / board / config
//! 读写路径不消费本模块；未来任何存储做破坏性格式变更时按设计文档 §4 契约
//! 接线（传 stem + 迁移链 + 接线方版本常量）。
//!
//! 四条铁律的代码对应物（原文见
//! `docs/REPORT/2026-11-01_改进项落地执行清单.md` L15 节）：
//! ① 物理文件按世代命名 `{stem}.vN.jsonl`（v1 = 无世代段裸名基线）→
//!    [`detect_generations`]；
//! ② 相邻迁移各 own 一步，链式组合 → [`MigrationStep`]；
//! ③ 写升级=独占发布：encode → 临时名落盘 → 盘上回读校验 → rename 发布 →
//!    源世代保持原样 → [`migrate_to_latest`] 执行体；
//! ④ 已提交世代永不改名/替换/删除；中断尾部（`.migrating` 残留）清扫重走，
//!    已发布世代从 detect 最高世代续走——迁移器对半行坏文件不猜。

use serde_json::Value;
use std::io;
use std::path::{Path, PathBuf};

/// 世代文件名（铁律①命名协议）。v1 = 裸名基线（现网存量天然是 v1）；
/// vN（N≥2）= 版本段在扩展名前（`*.jsonl` glob 兼容——现行扫描器以
/// extension 过滤，对 v2+ 文件仍命中；接线时扫描器按剥段后的 stem 分组，
/// 见设计文档 §3）。
fn generation_file_name(stem: &str, generation: u32) -> String {
    if generation <= 1 {
        format!("{stem}.jsonl")
    } else {
        format!("{stem}.v{generation}.jsonl")
    }
}

/// 世代探测结果（即时投影，不缓存——目录是唯一权威）。
#[derive(Debug, Clone)]
pub struct Detection {
    /// 升序 `(generation, path)`。
    pub generations: Vec<(u32, PathBuf)>,
    /// 目录无任何世代文件（新会话或已清理——语义由调用方自定）。
    pub empty: bool,
}

impl Detection {
    /// 最高世代及其路径（空探测返回 None）。
    pub fn latest(&self) -> Option<(u32, &PathBuf)> {
        self.generations.last().map(|(g, p)| (*g, p))
    }
}

/// 目录扫描取世代清单（铁律①的探测半边）。
///
/// - `{stem}.jsonl` 存在 → v1；`{stem}.vN.jsonl`（N≥2 可解析）→ vN；
///   显式 `{stem}.v1.jsonl` / `{stem}.vX.jsonl` / 别的 stem = 非协议形态，
///   忽略不猜。
/// - 目录不存在 = 空探测（不 Err）。
pub fn detect_generations(dir: impl AsRef<Path>, stem: &str) -> io::Result<Detection> {
    let dir = dir.as_ref();
    let mut generations: Vec<(u32, PathBuf)> = Vec::new();

    let v1 = dir.join(format!("{stem}.jsonl"));
    if v1.exists() {
        generations.push((1, v1));
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // 目录不存在：裸名已查过不存在 → 空探测。
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Detection {
                generations: Vec::new(),
                empty: true,
            });
        }
        Err(e) => return Err(e),
    };
    let version_prefix = format!("{stem}.v");
    for ent in entries {
        let path = ent?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(rest) = name.strip_prefix(&version_prefix) else {
            continue;
        };
        let Some(num) = rest.strip_suffix(".jsonl") else {
            continue;
        };
        // N ≥ 2 才是协议世代（显式 v1 文件非协议形态，忽略）。
        if let Ok(n) = num.parse::<u32>()
            && n >= 2
        {
            generations.push((n, path));
        }
    }
    generations.sort_by_key(|(g, _)| *g);
    let empty = generations.is_empty();
    Ok(Detection { generations, empty })
}

/// 迁移单元（铁律②）：`from → to` 一步 own（协议纪律 to == from+1，由链
/// 定义方自负——设施只要求链能从检测世代走到 target_generation）。
pub trait MigrationStep: Send + Sync {
    fn from(&self) -> u32;
    fn to(&self) -> u32;

    /// 读旧世代内容（逐行 JSON，设施已剥除不可解析的未密封坏行——铁律④
    /// 迁移器不猜，坏行容忍见 [`read_jsonl_tolerant`]）→ 编码新世代全部行。
    /// 不落盘。
    fn encode(&self, src: Vec<Value>) -> Result<Vec<Value>, String>;

    /// 发布前校验（对**盘上临时名文件**严格逐行回读后的内容——校验对象是
    /// 落盘产物，不是内存副本）。缺省：非空（可解析性由设施的严格读取
    /// 保证——坏行在严格读取即失败）。
    fn validate(&self, rows: &[Value]) -> Result<(), String> {
        if rows.is_empty() {
            Err("发布内容为空".to_string())
        } else {
            Ok(())
        }
    }
}

/// 单步执行报告。`published` 在 read_only 模式下是**计划目标路径**（未发布
/// ——`MigrationOutcome.read_only = true` 明示）。
#[derive(Debug, Clone)]
pub struct StepReport {
    pub from: u32,
    pub to: u32,
    pub published: PathBuf,
}

/// 链式迁移结果。
#[derive(Debug, Clone)]
pub struct MigrationOutcome {
    pub from_generation: u32,
    pub to_generation: u32,
    pub steps: Vec<StepReport>,
    /// 末次发布的目标路径（零执行 / read_only 时 None）。
    pub published_path: Option<PathBuf>,
    /// true = 只做了探测与计划报告，零写盘（读打开不发布后继）。
    pub read_only: bool,
}

#[derive(Debug)]
pub enum GenError {
    /// 目录里有超过 target 的世代（未来版本拒绝——降级运行的旧代码被明确
    /// 拒绝，不猜不截断）。
    FutureGeneration {
        found: u32,
        target: u32,
    },
    /// 链断：没有 `from == g` 的迁移单元。
    MissingStep {
        from: u32,
    },
    /// encode 产出 0 行（几乎必然是编码 bug——诚实失败优于静默发布空文件）。
    EmptyTarget {
        from: u32,
    },
    /// 盘上回读校验不过 = 不发布（临时名清扫）。
    ValidateFailed {
        from: u32,
        message: String,
    },
    /// encode 自报失败。
    EncodeFailed {
        from: u32,
        message: String,
    },
    Io(io::Error),
}

impl std::fmt::Display for GenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GenError::FutureGeneration { found, target } => {
                write!(
                    f,
                    "发现未来世代 v{found}（当前支持到 v{target}）——拒绝猜测性降级"
                )
            }
            GenError::MissingStep { from } => {
                write!(f, "迁移链缺 v{from} → v{} 单元", from + 1)
            }
            GenError::EmptyTarget { from } => {
                write!(f, "v{from} 编码产出 0 行——拒绝发布空文件")
            }
            GenError::ValidateFailed { from, message } => {
                write!(f, "v{} 发布前校验失败：{message}", from + 1)
            }
            GenError::EncodeFailed { from, message } => {
                write!(f, "v{from} 编码失败：{message}")
            }
            GenError::Io(e) => write!(f, "世代迁移 IO 失败：{e}"),
        }
    }
}

impl std::error::Error for GenError {}

/// 逐行读取 jsonl，**不可解析行跳过**（容忍消费方未密封的尾部坏行——铁律④
/// 迁移器不猜不修复，encode 输入只含可解析行）。用于读**源世代**文件。
fn read_jsonl_tolerant(path: &Path) -> io::Result<Vec<Value>> {
    let content = std::fs::read_to_string(path)?;
    Ok(content
        .lines()
        .filter_map(|line| {
            let t = line.trim();
            if t.is_empty() {
                return None;
            }
            serde_json::from_str(t).ok()
        })
        .collect())
}

/// 逐行严格读取（一行不可解析即 Err）。用于**盘上临时名**发布前回读校验
/// ——刚写出的文件若有坏行就是写盘损坏，必须拦在校验层。
fn read_jsonl_strict(path: &Path) -> Result<Vec<Value>, String> {
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        rows.push(serde_json::from_str(line).map_err(|e| format!("第 {} 行解析失败：{e}", i + 1))?);
    }
    Ok(rows)
}

fn write_jsonl(path: &Path, rows: &[Value]) -> io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    for r in rows {
        writeln!(f, "{r}")?;
    }
    f.flush()?;
    Ok(())
}

/// 清扫该 stem 的发布临时名残留（`{stem}.*.migrating`——encode 后 rename
/// 前进程终止的产物）。残留内容永不采信（可能不完整），删了重走是唯一
/// 语义。韧性删除（Windows AV/索引器瞬时句柄是常态）。
fn cleanup_migrating_residue(dir: &Path, stem: &str) -> io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let prefix = format!("{stem}.");
    for ent in entries {
        let path = ent?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with(&prefix) && name.ends_with(".migrating") {
            let _ = crate::fsutil::remove_file_retry(&path);
        }
    }
    Ok(())
}

/// 链式迁移主入口（铁律②③④的执行体）。
///
/// `target_generation` = 接线方存储的当前格式版本（设施不内置常量——PoC
/// 基线 v1 时链无处施展；接线方如 chat_log 自带版本常量传入）。
///
/// `read_only = true`：只探测 + 报告计划步（steps 的 published 是计划目标
/// 路径），**零写盘**——连 `.migrating` 残留清扫都不做（读打开不发布后继）。
pub fn migrate_to_latest(
    dir: impl AsRef<Path>,
    stem: &str,
    chain: &[Box<dyn MigrationStep>],
    target_generation: u32,
    read_only: bool,
) -> Result<MigrationOutcome, GenError> {
    let dir = dir.as_ref();
    if !read_only {
        cleanup_migrating_residue(dir, stem).map_err(GenError::Io)?;
    }

    let detection = detect_generations(dir, stem).map_err(GenError::Io)?;
    let Some((start_gen, _start_path)) = detection.latest() else {
        // 空目录：无物可迁移（新会话语义，调用方自决）。
        return Ok(MigrationOutcome {
            from_generation: 0,
            to_generation: 0,
            steps: Vec::new(),
            published_path: None,
            read_only,
        });
    };
    if start_gen > target_generation {
        return Err(GenError::FutureGeneration {
            found: start_gen,
            target: target_generation,
        });
    }

    // 已是最新：零执行。
    if start_gen == target_generation {
        return Ok(MigrationOutcome {
            from_generation: start_gen,
            to_generation: start_gen,
            steps: Vec::new(),
            published_path: None,
            read_only,
        });
    }

    // read_only：只报告计划（链缺步照常诚实报告），零执行。
    if read_only {
        let mut plan = Vec::new();
        let mut g = start_gen;
        while g < target_generation {
            let step = chain
                .iter()
                .find(|s| s.from() == g)
                .ok_or(GenError::MissingStep { from: g })?;
            plan.push(StepReport {
                from: g,
                to: step.to(),
                published: dir.join(generation_file_name(stem, step.to())),
            });
            g = step.to();
            if g > target_generation {
                break;
            }
        }
        return Ok(MigrationOutcome {
            from_generation: start_gen,
            to_generation: g,
            steps: plan,
            published_path: None,
            read_only: true,
        });
    }

    // 执行链：每步 encode → 临时名落盘 → 盘上回读校验 → rename 发布 →
    // 源保持原样（铁律③④）。
    let mut g = start_gen;
    let mut reports: Vec<StepReport> = Vec::new();
    let mut last_published: Option<PathBuf> = None;
    while g < target_generation {
        let step = chain
            .iter()
            .find(|s| s.from() == g)
            .ok_or(GenError::MissingStep { from: g })?;
        let target_gen = step.to();

        // 读源世代（容忍坏行）→ encode。
        let src_rows = read_jsonl_tolerant(
            detection
                .generations
                .iter()
                .find(|(g0, _)| *g0 == g)
                .map(|(_, p)| p.as_path())
                .unwrap_or(&dir.join(generation_file_name(stem, g))),
        )
        .map_err(GenError::Io)?;
        let new_rows = step.encode(src_rows).map_err(|m| GenError::EncodeFailed {
            from: g,
            message: m,
        })?;
        if new_rows.is_empty() {
            return Err(GenError::EmptyTarget { from: g });
        }

        // 临时名落盘 → 盘上回读严格校验。
        let target_name = generation_file_name(stem, target_gen);
        let target_path = dir.join(&target_name);
        let tmp_path = dir.join(format!("{target_name}.migrating"));
        write_jsonl(&tmp_path, &new_rows).map_err(GenError::Io)?;
        let on_disk = read_jsonl_strict(&tmp_path)
            .map_err(|m| GenError::ValidateFailed {
                from: g,
                message: m,
            })
            .and_then(|rows| {
                step.validate(&rows)
                    .map(|_| rows)
                    .map_err(|m| GenError::ValidateFailed {
                        from: g,
                        message: m,
                    })
            });
        if let Err(e) = on_disk {
            let _ = crate::fsutil::remove_file_retry(&tmp_path);
            return Err(e);
        }

        // 发布：先查目标（重入/并发已发布 → 幂等臂认输——铁律④已提交
        // 世代永不替换）。诚实边界：检查与 rename 之间仍有 nanos 级竞态
        // 窗口，且 std::fs::rename 两平台都是「存在即原子替换」语义
        //（Windows MOVEFILE_REPLACE_EXISTING / Unix rename(2)）——绝对
        // no-replace 需平台 API（renameat2 RENAME_NOREPLACE / MoveFileEx
        // 去掉 REPLACE 标志），模板用 std 能力，接线方如需严格闭合窗口
        // 在此处换平台调用（设计文档 §4 记录在案）。
        if target_path.exists() {
            let _ = crate::fsutil::remove_file_retry(&tmp_path);
        } else {
            std::fs::rename(&tmp_path, &target_path).map_err(|e| {
                let _ = crate::fsutil::remove_file_retry(&tmp_path);
                GenError::Io(e)
            })?;
        }

        reports.push(StepReport {
            from: g,
            to: target_gen,
            published: target_path.clone(),
        });
        last_published = Some(target_path);
        g = target_gen;
        if g > target_generation {
            break;
        }
    }

    Ok(MigrationOutcome {
        from_generation: start_gen,
        to_generation: g,
        steps: reports,
        published_path: last_published,
        read_only: false,
    })
}

#[cfg(test)]
mod tests;
