//! snapshot —— 配置期内存快照存储（T3.4，纯 fs）。
//!
//! 布局：`<workspace>/ziwei/setup/<profile_id>/snapshots/<unixms>/`
//! ├── meta.json（模块基址/machine/节表/来源 PID/exe 路径）
//! ├── headers.bin（头页，含节表——pe::parse 直接可解）
//! └── <section>.bin（.text/.rdata/.data 逐节）
//!
//! 删除走 nemesis-utils::fsutil 韧性包装（CLAUDE.md 环境速查纪律）；
//! TTL/容量清扫与 CaptureStore 同纪律：read_dir 结果显式排序。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::pe::{PeInfo, SectionInfo};

/// 快照元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    /// 所属（草稿）profile id。
    pub profile_id: String,
    /// RFC3339 时间。
    pub ts_rfc3339: String,
    pub pid: u32,
    /// 主模块基址（进程内 VA）。
    pub module_base: u64,
    pub machine: u16,
    /// 磁盘 exe 路径（指纹素材）。
    pub exe_path: String,
    /// 快照覆盖的节（头页之外）。
    pub sections: Vec<SectionInfo>,
}

/// 清扫报告（诚实计数）。
#[derive(Debug, Default, PartialEq)]
pub struct SweepReport {
    pub expired: usize,
    pub kept: usize,
}

/// 快照存储。
pub struct SnapshotStore {
    root: PathBuf, // <ws>/ziwei/setup/<profile_id>/snapshots
}

impl SnapshotStore {
    pub fn new(workspace: &Path, profile_id: &str) -> SnapshotStore {
        SnapshotStore {
            root: workspace
                .join("ziwei")
                .join("setup")
                .join(profile_id)
                .join("snapshots"),
        }
    }

    fn ts_dir(&self, ts_ms: u64) -> PathBuf {
        self.root.join(ts_ms.to_string())
    }

    /// 落盘一份快照。返回目录。
    /// 原子纪律：全部内容先写进 `.tmp<ts>`，最后才创建 ts 目录并改名——
    /// 失败路径只可能残留 `.tmp*`（sweep 的数字名过滤天然跳过），绝不留
    /// 空 ts 目录被 latest/sweep 误当活快照。
    pub fn save(
        &self,
        meta: &SnapshotMeta,
        headers: &[u8],
        section_blobs: &[(String, Vec<u8>)],
        ts_ms: u64,
    ) -> std::io::Result<PathBuf> {
        let dir = self.ts_dir(ts_ms);
        let tmp = self.root.join(format!(".tmp{ts_ms}"));
        let _ = nemesis_utils::fsutil::remove_dir_all_retry(&tmp); // 残留防御
        std::fs::create_dir_all(&tmp)?;
        let write_all = || -> std::io::Result<()> {
            std::fs::write(tmp.join("meta.json"), serde_json::to_vec(meta)?)?;
            std::fs::write(tmp.join("headers.bin"), headers)?;
            for (name, bytes) in section_blobs {
                // 节名只含 [A-Za-z0-9_$.]（PE 节名常态：.text/.rdata），防御路径注入
                let safe: String = name
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '.'))
                    .collect();
                if safe.is_empty() || safe != *name {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("节名非法：{name}"),
                    ));
                }
                std::fs::write(tmp.join(format!("{safe}.bin")), bytes)?;
            }
            Ok(())
        };
        if let Err(e) = write_all() {
            let _ = nemesis_utils::fsutil::remove_dir_all_retry(&tmp);
            return Err(e);
        }
        std::fs::create_dir_all(&dir)?;
        std::fs::rename(&tmp, dir.join("data"))?;
        Ok(dir)
    }

    /// 最新快照（按目录数字序）→ (meta, headers, sections)。
    pub fn latest(
        &self,
    ) -> std::io::Result<Option<(SnapshotMeta, Vec<u8>, Vec<(String, Vec<u8>)>)>> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(_) => return Ok(None),
        };
        let mut dirs: Vec<(u64, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                name.parse::<u64>().ok().map(|ts| (ts, e.path()))
            })
            .collect();
        dirs.sort(); // read_dir 顺序是平台细节——显式排序
        let latest = match dirs.last() {
            Some((_, p)) => p.join("data"),
            None => return Ok(None),
        };
        let meta: SnapshotMeta = serde_json::from_slice(&std::fs::read(latest.join("meta.json"))?)?;
        let headers = std::fs::read(latest.join("headers.bin"))?;
        let mut sections = Vec::new();
        for si in &meta.sections {
            let p = latest.join(format!("{}.bin", si.name));
            if p.exists() {
                sections.push((si.name.clone(), std::fs::read(p)?));
            }
        }
        Ok(Some((meta, headers, sections)))
    }

    /// TTL + 快照目录总数上限清扫（超出上限删最旧；目录粒度）。
    pub fn sweep(&self, ttl: std::time::Duration, max_kept: usize) -> SweepReport {
        let mut report = SweepReport::default();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(_) => return report,
        };
        let now = std::time::SystemTime::now();
        let mut dirs: Vec<(u64, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                name.parse::<u64>().ok().map(|ts| (ts, e.path()))
            })
            .collect();
        dirs.sort(); // 旧 → 新
        // TTL（目录 mtime——目录名是 unixms 创建序号，不是绝对时间基准）
        let mut removed = 0usize;
        for (_, p) in &dirs {
            let age = std::fs::metadata(p)
                .and_then(|m| m.modified())
                .map(|m| now.duration_since(m).unwrap_or_default())
                .unwrap_or_default();
            if age > ttl && nemesis_utils::fsutil::remove_dir_all_retry(p).is_ok() {
                removed += 1;
            }
        }
        // 容量（TTL 后仍存活的按序保最新 max_kept 份）
        let alive: Vec<(u64, PathBuf)> = dirs.iter().filter(|(_, p)| p.exists()).cloned().collect();
        if alive.len() > max_kept {
            for (_, p) in &alive[..alive.len() - max_kept] {
                if nemesis_utils::fsutil::remove_dir_all_retry(p).is_ok() {
                    removed += 1;
                }
            }
        }
        report.expired = removed;
        report.kept = dirs.iter().filter(|(_, p)| p.exists()).count();
        report
    }
}

/// PeInfo 解析辅助：从快照头页重建（analyze 路径）。
pub fn parse_headers(headers: &[u8]) -> Result<PeInfo, crate::pe::PeError> {
    PeInfo::parse(headers)
}
