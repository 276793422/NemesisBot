//! B9-1 内部方法：bulk 分块传输 + hash_tree 全树指纹。
//!
//! **这些不是 agent 工具**：gateway 同步引擎（`nemesisbot/src/vm_sync.rs`）
//! 经 `VmSupervisor::internal_call` 专用通道调用——宿主侧 `VM_GUEST_TOOLS`
//! 子集闸不认识它们，agent 工具面永不暴露（vm_sync 的地基，见 B9 实施计划
//! §4.2：bulk_write 收分块 tar 流整树落 /workspace、bulk_read 按清单打包
//! 回传、hash_tree 返回全树/子树 {相对路径→sha256}）。
//!
//! 载体纪律：协议是「一行 JSON 请求 / 一行 JSON 响应」，二进制 tar 以
//! base64 分块坐进 JSON 行（写方向 host→guest 逐块 append 到临时文件，
//! 末块整包解出；读方向 guest 先 `bulk_read` 打包报块数，宿主再逐块
//! `bulk_read_chunk` 拉取，末块即删临时文件）。传输状态挂
//! [`GuestAgent::transfers`]（Arc 共享，连接间可达）。
//!
//! 安全边界（与工具面同水位）：条目名拒绝对路径组件（`..`/绝对路径/
//! Windows 前缀），v1 不落 symlink/硬链接条目（计数上报，宿主侧诚实注记）；
//! transfer id 白名单字符集（防临时文件路径注入）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

/// 写方向单块原始字节上限（宿主按 4MiB 切；guest 收 8MiB 容差闸）。
pub const BULK_CHUNK_MAX_BYTES: usize = 8 * 1024 * 1024;
/// 读方向回传块大小（guest 侧切分常数；宿主按响应里的块数拉取）。
pub const BULK_READ_CHUNK_BYTES: u64 = 4 * 1024 * 1024;
/// 传输条目数上限（hash_tree 截断闸；宿主见 truncated=true 必须诚实报错，
/// 不许拿半棵树当基线）。
pub const MAX_HASH_ENTRIES: usize = 400_000;
/// 传输状态滞留 TTL（毫秒）：超时半截传输在下次任何 bulk 操作开头被 GC。
const TRANSFER_TTL_MS: u64 = 60 * 60 * 1000;
/// 单传输块数上限（4MiB × 200k = 800GB，纯防御性上限）。
const MAX_TOTAL_CHUNKS: u64 = 200_000;

/// 传输方向。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// host→guest：guest 侧 append tar 块，末块解包。
    Write,
    /// guest→host：guest 侧打包，逐块回传。
    Read,
}

pub(crate) struct BulkTransfer {
    direction: Direction,
    total_chunks: u64,
    /// 写方向 = 下一个期望 seq；读方向 = 下一个待取 seq。
    next_seq: u64,
    bytes_so_far: u64,
    file: PathBuf,
    created_ms: u64,
    /// 写方向专用：首块声明的解包前清场标志（末块消费——中间块不携带）。
    clean: bool,
}

/// 传输状态表（GuestAgent 字段；Arc 共享——accept 循环每连接克隆 agent 后
/// 仍指向同一张表，分块序列跨连接可达）。
pub(crate) type TransferMap = Arc<Mutex<HashMap<String, BulkTransfer>>>;

pub(crate) fn new_transfer_map() -> TransferMap {
    Arc::new(Mutex::new(HashMap::new()))
}

static CHUNK_COUNTER: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// transfer id 白名单（防临时文件名路径注入）：alnum / `-` / `_`，≤64 字符。
fn valid_transfer_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// tar 条目名安全闸：拒绝绝对路径 / `..` 组件 / Windows 前缀与根组件。
fn safe_entry_rel(name: &Path) -> Result<String, String> {
    if name.is_absolute() {
        return Err(format!("tar 条目为绝对路径，拒绝: {}", name.display()));
    }
    for c in name.components() {
        match c {
            Component::Normal(_) => {}
            other => {
                return Err(format!(
                    "tar 条目含非法路径组件 ({other:?})，拒绝: {}",
                    name.display()
                ));
            }
        }
    }
    let s = name.to_string_lossy();
    if s.is_empty() {
        return Err("tar 条目名为空，拒绝".to_string());
    }
    Ok(s.replace('\\', "/"))
}

/// 临时传输目录（懒建）。
fn transfer_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("nemesis-guest-bulk");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建传输临时目录失败: {e}"))?;
    Ok(dir)
}

/// GC 半截滞留传输（超 TTL 即删状态 + 临时文件；每次 bulk 操作开头跑）。
async fn gc_stale(transfers: &TransferMap) {
    let mut map = transfers.lock().await;
    let cutoff = now_ms().saturating_sub(TRANSFER_TTL_MS);
    let stale: Vec<(String, PathBuf)> = map
        .iter()
        .filter(|(_, t)| t.created_ms < cutoff)
        .map(|(k, t)| (k.clone(), t.file.clone()))
        .collect();
    for (id, file) in stale {
        map.remove(&id);
        let _ = std::fs::remove_file(&file);
    }
}

// ---------------------------------------------------------------------------
// bulk_write：host→guest 分块 tar 整树落地
// ---------------------------------------------------------------------------

/// `{"transfer":id,"seq":N,"total":N,"data":b64,"clean":bool?}`。
/// 末块解包（clean=true 仅首块携带时先清空工作区），返回解包摘要。
pub async fn bulk_write(
    val: &serde_json::Value,
    workspace: &Path,
    transfers: &TransferMap,
) -> Result<String, String> {
    let id = val
        .get("transfer")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'transfer' argument".to_string())?;
    if !valid_transfer_id(id) {
        return Err(format!("invalid transfer id: {id:?}"));
    }
    let seq = val
        .get("seq")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "Missing 'seq' argument".to_string())?;
    let total = val
        .get("total")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "Missing 'total' argument".to_string())?;
    if total == 0 || total > MAX_TOTAL_CHUNKS {
        return Err(format!(
            "total out of range (1..={MAX_TOTAL_CHUNKS}): {total}"
        ));
    }
    if seq >= total {
        return Err(format!("seq {seq} out of range (total={total})"));
    }
    let data = val
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'data' argument".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("data base64 decode failed: {e}"))?;
    if bytes.len() > BULK_CHUNK_MAX_BYTES {
        return Err(format!(
            "chunk too large: {} bytes (max {BULK_CHUNK_MAX_BYTES})",
            bytes.len()
        ));
    }
    let clean = val.get("clean").and_then(|v| v.as_bool()).unwrap_or(false);

    gc_stale(transfers).await;
    let mut map = transfers.lock().await;
    let dir = transfer_dir()?;
    let file = dir.join(format!("{id}.tar"));

    match map.get_mut(id) {
        None => {
            if seq != 0 {
                return Err(format!(
                    "unknown transfer {id:?} with seq={seq}（首块必须 seq=0）"
                ));
            }
            // 首块：重开临时文件（同 id 重传 = 截断重来）。
            let f =
                std::fs::File::create(&file).map_err(|e| format!("创建传输临时文件失败: {e}"))?;
            drop(f);
            map.insert(
                id.to_string(),
                BulkTransfer {
                    direction: Direction::Write,
                    total_chunks: total,
                    next_seq: 1,
                    bytes_so_far: bytes.len() as u64,
                    file: file.clone(),
                    created_ms: now_ms(),
                    clean,
                },
            );
        }
        Some(t) => {
            if t.direction != Direction::Write {
                return Err(format!("transfer {id:?} 是读方向传输，不能 bulk_write"));
            }
            if t.total_chunks != total {
                return Err(format!(
                    "total mismatch for transfer {id:?}: got {total}, expected {}",
                    t.total_chunks
                ));
            }
            if t.next_seq != seq {
                return Err(format!(
                    "out-of-order chunk for transfer {id:?}: expected seq {}, got {seq}",
                    t.next_seq
                ));
            }
            t.next_seq += 1;
            t.bytes_so_far += bytes.len() as u64;
        }
    }

    // append 本块（持锁执行——同传输的块序以锁为序，乱序到达被上面的
    // next_seq 闸拒绝；4MiB 本地盘 append 毫秒级，串行无害）。
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .map_err(|e| format!("打开传输临时文件失败: {e}"))?;
        f.write_all(&bytes)
            .map_err(|e| format!("写入传输临时文件失败: {e}"))?;
    }

    let is_final = seq + 1 == total;
    if !is_final {
        return Ok(serde_json::json!({ "received": seq + 1, "total": total }).to_string());
    }

    // 末块：整包解出到工作区（解出前持锁摘状态——解包失败时该传输作废，
    // 宿主重传 = 全新 transfer id；clean 消费首块声明的标志）。
    let state = map.remove(id).expect("just checked");
    drop(map);
    if state.clean {
        clean_workspace(workspace)?;
    }
    let (written, skipped_symlinks) = extract_tar_safe(&state.file, workspace)?;
    // 解包完成后临时 tar 才可删（先删后解 = 必然失败的顺序）。
    let _ = std::fs::remove_file(&state.file);
    Ok(serde_json::json!({
        "written": written,
        "skipped_symlinks": skipped_symlinks,
        "bytes": state.bytes_so_far,
    })
    .to_string())
}

/// 清空工作区内容（首次全量同步的 clean 语义；工作区目录本身保留）。
fn clean_workspace(workspace: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(workspace).map_err(|e| format!("读取工作区失败: {e}"))?;
    for ent in entries.flatten() {
        let p = ent.path();
        let res = if p.is_dir() {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
        if let Err(e) = res {
            // 删除挂起幻影（AV/索引器瞬时句柄）容忍到重试也失败的形态：
            // clean 是尽力而为的清场，个别残留不阻塞解包（解包同名覆盖）。
            if p.is_dir() && !p.exists() {
                continue;
            }
            return Err(format!("清空工作区失败 ({}): {e}", p.display()));
        }
    }
    Ok(())
}

/// 安全解包 tar：条目名过 [`safe_entry_rel`]，Regular 落盘（Unix 恢复权限
/// 位），Directory 建目录，其余形态（symlink/hardlink/fifo 等）跳过计数。
fn extract_tar_safe(tar_path: &Path, workspace: &Path) -> Result<(usize, usize), String> {
    let f = std::fs::File::open(tar_path).map_err(|e| format!("打开 tar 失败: {e}"))?;
    let mut arch = tar::Archive::new(std::io::BufReader::new(f));
    let mut written = 0usize;
    let mut skipped = 0usize;
    let mut entries = arch.entries().map_err(|e| format!("tar 解析失败: {e}"))?;
    for entry in entries.by_ref() {
        let mut entry = entry.map_err(|e| format!("tar 条目读取失败: {e}"))?;
        let rel = safe_entry_rel(&entry.path().map_err(|e| format!("{e}"))?)?;
        let dst = workspace.join(&rel);
        match entry.header().entry_type() {
            tar::EntryType::Directory => {
                std::fs::create_dir_all(&dst).map_err(|e| format!("创建目录 {rel} 失败: {e}"))?;
                written += 1;
            }
            tar::EntryType::Regular => {
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("创建父目录 {rel} 失败: {e}"))?;
                }
                let mut out =
                    std::fs::File::create(&dst).map_err(|e| format!("创建文件 {rel} 失败: {e}"))?;
                std::io::copy(&mut entry, &mut out).map_err(|e| format!("写入 {rel} 失败: {e}"))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = entry.header().mode().unwrap_or(0o644) & 0o7777;
                    let _ = std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(mode));
                }
                written += 1;
            }
            _ => {
                skipped += 1; // symlink/hardlink/fifo：v1 不落，宿主诚实注记
            }
        }
    }
    Ok((written, skipped))
}

// ---------------------------------------------------------------------------
// bulk_read / bulk_read_chunk：guest→host 按清单打包回传
// ---------------------------------------------------------------------------

/// `{"paths":[rel,...]}` → 打包 tar 到临时文件，返回
/// `{"transfer":id,"chunks":N,"bytes":B,"missing":[...]}`。
/// 宿主随后逐块 `bulk_read_chunk`；缺清单文件容忍（进 missing 上报，
/// 不算失败——宿主侧清单可能领先/滞后于 VM 盘）。
pub async fn bulk_read(
    val: &serde_json::Value,
    workspace: &Path,
    transfers: &TransferMap,
) -> Result<String, String> {
    let paths: Vec<String> = val
        .get("paths")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Missing 'paths' argument".to_string())?
        .iter()
        .map(|p| {
            p.as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "'paths' 元素必须是字符串".to_string())
        })
        .collect::<Result<_, _>>()?;
    if paths.is_empty() {
        return Err("paths 为空".to_string());
    }
    // 去重保序 + 安全校验。
    let mut seen = std::collections::HashSet::new();
    let mut rels = Vec::new();
    for p in &paths {
        let rel = safe_entry_rel(Path::new(p))?;
        if seen.insert(rel.clone()) {
            rels.push(rel);
        }
    }

    gc_stale(transfers).await;
    let id = format!(
        "out-{}-{}",
        now_ms(),
        CHUNK_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let dir = transfer_dir()?;
    let file = dir.join(format!("{id}.tar"));
    let packed = {
        let workspace = workspace.to_path_buf();
        let file = file.clone();
        let rels = rels.clone();
        tokio::task::spawn_blocking(move || pack_files(&workspace, &rels, &file))
            .await
            .map_err(|e| format!("打包任务失败: {e}"))??
    };

    // 分块账按 tar **实际长度**：头部开销（512B/文件 + 1024B 收尾块）不
    // 在 payload 字节和里——按 packed.bytes 切会在大变更集跨 4MiB 边界时
    // 少给块，宿主拉到截尾 tar 解包失败（H1 回归锁见 bulk/tests.rs）。
    let tar_len = std::fs::metadata(&file)
        .map(|m| m.len())
        .unwrap_or(packed.bytes);
    let chunks = read_chunk_plan(tar_len);
    // 缺席清单文件容忍（不算失败——宿主侧清单可能领先/滞后于 VM 盘）。
    let missing: Vec<String> = rels
        .iter()
        .filter(|r| !packed.present.contains(r))
        .cloned()
        .collect();
    transfers.lock().await.insert(
        id.clone(),
        BulkTransfer {
            direction: Direction::Read,
            total_chunks: chunks,
            next_seq: 0,
            bytes_so_far: tar_len,
            file,
            created_ms: now_ms(),
            clean: false,
        },
    );
    Ok(serde_json::json!({
        "transfer": id,
        "chunks": chunks,
        "bytes": tar_len,
        "missing": missing,
    })
    .to_string())
}

/// 读方向分块计划（纯函数；0 字节也给 1 块——空 tar 仍有收尾块要传，
/// 宿主按块数循环拉取）。块数必须覆盖 tar 全长：`ceil(tar_len / 4MiB)`。
fn read_chunk_plan(tar_len: u64) -> u64 {
    if tar_len == 0 {
        1
    } else {
        tar_len.div_ceil(BULK_READ_CHUNK_BYTES)
    }
}

struct PackResult {
    bytes: u64,
    /// 实际打包进 tar 的相对路径（缺席文件不进包）。
    present: Vec<String>,
}

/// 按清单打包（排序保证确定性；Unix 保留实际权限位，Windows 缺省 755）。
fn pack_files(workspace: &Path, rels: &[String], out: &Path) -> Result<PackResult, String> {
    let mut sorted: Vec<&String> = rels.iter().collect();
    sorted.sort();
    let f = std::fs::File::create(out).map_err(|e| format!("创建回传 tar 失败: {e}"))?;
    let mut builder = tar::Builder::new(std::io::BufWriter::new(f));
    let mut bytes = 0u64;
    let mut present = Vec::new();
    for rel in sorted {
        let src = workspace.join(rel);
        let meta = match std::fs::metadata(&src) {
            Ok(m) if m.is_file() => m,
            _ => continue, // 缺席/非普通文件 → missing
        };
        let mut header = tar::Header::new_gnu();
        header.set_size(meta.len());
        header.set_mtime(
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            header.set_mode(meta.permissions().mode() & 0o7777);
        }
        #[cfg(not(unix))]
        header.set_mode(0o755);
        header.set_cksum();
        let mut src_f = std::fs::File::open(&src).map_err(|e| format!("打开 {rel} 失败: {e}"))?;
        builder
            .append_data(&mut header, rel.as_str(), &mut src_f)
            .map_err(|e| format!("打包 {rel} 失败: {e}"))?;
        bytes += meta.len();
        present.push(rel.clone());
    }
    builder
        .finish()
        .map_err(|e| format!("收尾 tar 失败: {e}"))?;
    Ok(PackResult { bytes, present })
}

/// `{"transfer":id,"seq":N}` → `{"data":b64,"seq":N}`；末块即删临时文件。
pub async fn bulk_read_chunk(
    val: &serde_json::Value,
    transfers: &TransferMap,
) -> Result<String, String> {
    let id = val
        .get("transfer")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'transfer' argument".to_string())?;
    let seq = val
        .get("seq")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "Missing 'seq' argument".to_string())?;
    let mut map = transfers.lock().await;
    let t = map
        .get_mut(id)
        .ok_or_else(|| format!("unknown transfer {id:?}"))?;
    if t.direction != Direction::Read {
        return Err(format!(
            "transfer {id:?} 是写方向传输，不能 bulk_read_chunk"
        ));
    }
    if t.next_seq != seq {
        return Err(format!(
            "out-of-order chunk for transfer {id:?}: expected seq {}, got {seq}",
            t.next_seq
        ));
    }
    if seq >= t.total_chunks {
        return Err(format!(
            "seq {seq} out of range (chunks={})",
            t.total_chunks
        ));
    }
    let file = t.file.clone();
    t.next_seq += 1;
    let is_final = t.next_seq == t.total_chunks;
    drop(map);

    let data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let mut f = std::fs::File::open(&file).map_err(|e| format!("打开 tar 失败: {e}"))?;
        let start = seq
            .checked_mul(BULK_READ_CHUNK_BYTES)
            .ok_or_else(|| "chunk offset overflow".to_string())?;
        use std::io::{Seek, SeekFrom};
        f.seek(SeekFrom::Start(start))
            .map_err(|e| format!("seek 失败: {e}"))?;
        let mut buf = vec![0u8; BULK_READ_CHUNK_BYTES as usize];
        let mut filled = 0usize;
        while filled < buf.len() {
            let n = f
                .read(&mut buf[filled..])
                .map_err(|e| format!("读 tar 失败: {e}"))?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        buf.truncate(filled);
        Ok(buf)
    })
    .await
    .map_err(|e| format!("读块任务失败: {e}"))??;

    if is_final {
        let mut map = transfers.lock().await;
        if let Some(t) = map.remove(id) {
            let _ = std::fs::remove_file(&t.file);
        }
    }
    Ok(serde_json::json!({
        "data": base64::engine::general_purpose::STANDARD.encode(&data),
        "seq": seq,
    })
    .to_string())
}

// ---------------------------------------------------------------------------
// hash_tree：全树/子树指纹（开工基线 + commit 增量扫描的读取面）
// ---------------------------------------------------------------------------

/// `{"mode":"dirs"}` → 全部目录 `[{path, mtime_ms}]`（目录 mtime 是 B9-2
/// commit 闸粗筛的信号——Linux 在条目增删改名时更新父目录 mtime）。
/// `{"mode":"files","subtree"?:"rel","exclude_prefixes"?:[..],"paths"?:[..]}`
/// → 子树内全部普通文件 `[{path, sha256, size}]`；`paths` 给出时为**定点
/// 指纹**——只查这些精确相对路径（存在才返回，缺失静默不报——宿主以
/// 缺席分类「VM 侧已删」），O(变更量) 的开工闸嫌疑文件三方对比面。
/// 目录树超过 [`MAX_HASH_ENTRIES`] 时 `"truncated": true`（宿主必须诚实
/// 报错）。
pub async fn hash_tree(val: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let mode = val
        .get("mode")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "Missing 'mode' argument (dirs|files)".to_string())?;
    let subtree = match val.get("subtree").and_then(|v| v.as_str()) {
        Some("") | None => String::new(),
        Some(s) => safe_entry_rel(Path::new(s))?,
    };
    let prefixes: Vec<String> = val
        .get("exclude_prefixes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|p| {
                    let s = p.as_str().ok_or("'exclude_prefixes' 元素必须是字符串")?;
                    let norm = safe_entry_rel(Path::new(s))?;
                    Ok(format!("{}/", norm.trim_end_matches('/')))
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .transpose()?
        .unwrap_or_default();
    let paths: Option<Vec<String>> = val
        .get("paths")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|p| {
                    let s = p.as_str().ok_or("'paths' 元素必须是字符串")?;
                    safe_entry_rel(Path::new(s))
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .transpose()?;
    let root = if subtree.is_empty() {
        workspace.to_path_buf()
    } else {
        workspace.join(&subtree)
    };
    if !root.is_dir() {
        return Err(format!("subtree 不存在或不是目录: {subtree:?}"));
    }

    let workspace = workspace.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<String, String> {
        // 定点指纹臂：paths 给出 = 不走树，逐条 stat+hash（O(变更量)）。
        if mode != "dirs"
            && let Some(paths) = paths
        {
            if paths.len() > MAX_HASH_ENTRIES {
                return Ok(serde_json::json!({
                    "mode": "files", "truncated": true, "files": []
                })
                .to_string());
            }
            let mut files: Vec<(String, String, u64)> = Vec::new();
            for rel in &paths {
                let p = workspace.join(rel);
                let Ok(meta) = std::fs::metadata(&p) else {
                    continue; // 缺失 = 静默不报（宿主以缺席分类 VM 已删）
                };
                if !meta.is_file() {
                    continue;
                }
                files.push((rel.clone(), hash_file(&p)?, meta.len()));
            }
            files.sort();
            return Ok(serde_json::json!({
                "mode": "files",
                "truncated": false,
                "files": files
                    .into_iter()
                    .map(|(path, sha256, size)| {
                        serde_json::json!({"path": path, "sha256": sha256, "size": size})
                    })
                    .collect::<Vec<_>>(),
            })
            .to_string());
        }
        let mut dirs: Vec<(String, u64)> = Vec::new();
        let mut files: Vec<(String, String, u64)> = Vec::new();
        let mut truncated = false;
        walk_hash(
            &workspace,
            &root,
            &prefixes,
            mode == "dirs",
            &mut dirs,
            &mut files,
            &mut truncated,
        )?;
        dirs.sort();
        files.sort();
        Ok(if mode == "dirs" {
            serde_json::json!({
                "mode": "dirs",
                "truncated": truncated,
                "dirs": dirs
                    .into_iter()
                    .map(|(path, mtime_ms)| serde_json::json!({"path": path, "mtime_ms": mtime_ms}))
                    .collect::<Vec<_>>(),
            })
        } else {
            serde_json::json!({
                "mode": "files",
                "truncated": truncated,
                "files": files
                    .into_iter()
                    .map(|(path, sha256, size)| {
                        serde_json::json!({"path": path, "sha256": sha256, "size": size})
                    })
                    .collect::<Vec<_>>(),
            })
        }
        .to_string())
    })
    .await
    .map_err(|e| format!("hash_tree 任务失败: {e}"))?
}

/// `{"paths":[...]}` → 定点删除（B9-2 开工闸宿主删除传播）。
/// `{"deleted": n, "missing": m}`——缺失按 missing 计（幂等：重复删除
/// 不算错）；其他 IO 错误诚实上抛。路径过 [`safe_entry_rel`] 闸，目录
/// 不删（工作树是纯文件对账面；整目录删除走 clean 全量重同步）。
pub async fn bulk_delete(val: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let mut paths: Vec<String> = val
        .get("paths")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Missing 'paths' argument".to_string())?
        .iter()
        .map(|p| {
            let s = p.as_str().ok_or("'paths' 元素必须是字符串")?;
            safe_entry_rel(Path::new(s))
        })
        .collect::<Result<Vec<_>, String>>()?;
    paths.sort();
    paths.dedup();
    let workspace = workspace.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<String, String> {
        let mut deleted = 0u64;
        let mut missing = 0u64;
        for rel in &paths {
            let p = workspace.join(rel);
            match std::fs::remove_file(&p) {
                Ok(()) => deleted += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing += 1,
                Err(e) => return Err(format!("删除 {rel} 失败: {e}")),
            }
        }
        Ok(serde_json::json!({"deleted": deleted, "missing": missing}).to_string())
    })
    .await
    .map_err(|e| format!("bulk_delete 任务失败: {e}"))?
}

/// 递归走树（读目录排序保确定性；symlink 一律不跟不落——VM 树是纯文件
/// 工作树；命中排除前缀的目录整枝剪掉）。
#[allow(clippy::too_many_arguments)]
fn walk_hash(
    workspace: &Path,
    dir: &Path,
    prefixes: &[String],
    collect_dirs: bool,
    dirs: &mut Vec<(String, u64)>,
    files: &mut Vec<(String, String, u64)>,
    truncated: &mut bool,
) -> Result<(), String> {
    if dirs.len() + files.len() > MAX_HASH_ENTRIES {
        *truncated = true;
        return Ok(()); // 截断闸：停止深入，宿主见 truncated 诚实报错
    }
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)
        .map_err(|e| format!("读取目录 {} 失败: {e}", dir.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("枚举 {} 失败: {e}", dir.display()))?;
    entries.sort_by_key(|e| e.file_name());
    for ent in entries {
        let path = ent.path();
        // symlink 不跟（file_type 不解引用）。
        let ft = ent
            .file_type()
            .map_err(|e| format!("stat {} 失败: {e}", path.display()))?;
        let rel = path
            .strip_prefix(workspace)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let excluded = prefixes
            .iter()
            .any(|p| rel == p.trim_end_matches('/') || rel.starts_with(p.as_str()));
        if excluded {
            continue;
        }
        if ft.is_dir() {
            let mtime_ms = dir_mtime_ms(&path);
            dirs.push((rel.clone(), mtime_ms));
            walk_hash(
                workspace,
                &path,
                prefixes,
                collect_dirs,
                dirs,
                files,
                truncated,
            )?;
            if dirs.len() + files.len() > MAX_HASH_ENTRIES {
                *truncated = true;
                return Ok(());
            }
        } else if ft.is_file() && !collect_dirs {
            let hash = hash_file(&path)?;
            let size = path
                .metadata()
                .map(|m| m.len())
                .map_err(|e| format!("stat {} 失败: {e}", path.display()))?;
            files.push((rel, hash, size));
        }
    }
    Ok(())
}

fn dir_mtime_ms(path: &Path) -> u64 {
    path.metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 流式 sha256（256KiB 缓冲；hex 小写）。
fn hash_file(path: &Path) -> Result<String, String> {
    let mut f =
        std::fs::File::open(path).map_err(|e| format!("打开 {} 失败: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("读 {} 失败: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests;
