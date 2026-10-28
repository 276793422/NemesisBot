//! VM 环境自举——QEMU 发行获取（第 10 层安装面，对齐 sandbox install 体验）。
//!
//! 承诺与 [`crate::extract`](sandbox install) 同构：用户零人工拿到核心组件。
//! **平台矩阵化（2026-10-27 配置化改造）**：下载源/包名/二进制名单全部来自
//! [`super::qemu_config`] 矩阵（workspace `config/config.vm_qemu.json` 优先，
//! 缺失回内嵌模板，首装释放），代码零硬编码 URL。统一入口
//! [`install_qemu_for_host_workspace`] 按宿主键取条目后按 `kind` 分派：
//! - `archive`：ranged 并行下载 → SHA-512 双锚校验（钉值 + 上游 sidecar）→
//!   7z 免安装直解（未运行安装器，P5 实证直跑）→ 最小文件集定位 →
//!   落位 `<ws>/tools/vm/runtime/qemu/` → manifest 钉基线；
//! - `packages`：包管理器链按宿主分臂——Linux apt/dnf（免密 sudo 探测 +
//!   非交互安装）、Windows MSYS2 pacman（CLANGARM64，ARM64 宿主原生包源）、
//!   Android Termux（prefix 内 apt-get，无 sudo）。
//!
//! 校验面（诚实声明）：钉值是矩阵条目的显式信任锚（换源 = 改配置文件的
//! url+sha512）；条目 sha512 置空则回落 URL 同目录 sidecar 查表（sidecar
//! 不可达 = 诚实拒绝，不裸跑无校验下载）。
//!
//! 下载纪律：weilnetz 单连接 ~20KB/s（P5 实测），Accept-Ranges 探测通过 =
//! N 路并行分块（`.partN` 分片，完整分片重试时跳过 = 断点续传）；
//! 不支持 = 诚实单线回落。总长对账 Content-Length，全部完成后 SHA-512。
//!
//! 关闭态铁律不变：本模块所有函数只在用户显式触发安装时被调用
//! （CLI `vm install` / WSAPI `vm.install_qemu`，B3 接线）；VM 总开关
//! `executor.vm.enabled=false` 时命令族诚实拒绝。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::qemu_config::{QemuInstallKind, QemuPlatformEntry};

/// 默认并行分块数（weilnetz 单连接 ~20KB/s，12 路 ~5min/197MB，P5 实测口径）。
pub const DEFAULT_PARALLEL: usize = 12;

/// 单块读超时（连接假死检测——每读一段 120s 无进展即失败重试整块）。
const CHUNK_READ_TIMEOUT_SECS: u64 = 120;

/// 安装目录集合（caller 从 workspace 解析路径；URL/包名/二进制名单已矩阵化，
/// 不再是安装输入）。
#[derive(Debug, Clone)]
pub struct QemuInstallDirs {
    /// QEMU 落位目录（`<ws>/tools/vm/runtime/qemu/`——[`super::super`]
    /// 既有 `qemu_install_dir` 消费形态，resolve 链零改动）。
    pub install_dir: PathBuf,
    /// 下载/解包暂存目录（`<ws>/tools/vm/runtime/install-work/`；与
    /// install_dir 同卷 → rename 直移零拷贝）。
    pub work_dir: PathBuf,
    /// 7z 查找/缓存目录（复用 sandbox 的 `<ws>/tools/sandboxie/runtime/`——
    /// 同一份 7z 缓存服务两条安装链，零重复下载）。
    pub seven_zip_dir: PathBuf,
    /// 并行分块数（1 = 单线）。
    pub parallel: usize,
}

impl Default for QemuInstallDirs {
    fn default() -> Self {
        Self {
            install_dir: PathBuf::from("qemu"),
            work_dir: PathBuf::from("install-work"),
            seven_zip_dir: PathBuf::from("runtime"),
            parallel: DEFAULT_PARALLEL,
        }
    }
}

/// 安装结果（CLI/WSAPI 呈现 + manifest 同源数据）。
#[derive(Debug, Clone)]
pub struct QemuInstallReport {
    /// 落位目录。
    pub install_dir: PathBuf,
    /// qemu-system 二进制绝对路径。
    pub qemu_system: PathBuf,
    /// qemu-img 绝对路径。
    pub qemu_img: PathBuf,
    /// `--version` 首行实测（如 `QEMU emulator version 11.1.0 ...`）。
    pub version: String,
    /// 安装包 SHA-256（hex64，实测钉基线——manifest 与本值同源）。
    pub installer_sha256: String,
    /// true = 目录已就绪（幂等早退，未重装）。
    pub already_installed: bool,
}

/// 本次安装消费的配置事实（CLI/WSAPI 呈现 + 日志结构化面——矩阵化后
/// caller 需要知道命中了哪个平台条目、来源是 workspace 还是内嵌模板）。
#[derive(Debug, Clone)]
pub struct QemuResolvedPlatform {
    /// true = 本次触发了模板释放（首装）；false = workspace 已有配置文件。
    pub released: bool,
    /// 实际消费的矩阵来源。
    pub source: super::qemu_config::MatrixSource,
    /// 命中的平台键（`{os}-{arch}`）。
    pub platform_key: String,
    /// 安装形态。
    pub kind: QemuInstallKind,
    /// 条目人读注记。
    pub note: String,
}

/// 宿主统一安装入口（CLI `vm install` / WSAPI `vm.install_qemu` / examples
/// 单一消费面）：释放模板 → 解析矩阵（workspace 优先，缺失回内嵌）→ 取宿主
/// 条目 → 按 `kind` 分派 archive / packages 链。
pub async fn install_qemu_for_host_workspace(
    ws_config_dir: &Path,
    dirs: &QemuInstallDirs,
) -> Result<(QemuInstallReport, QemuResolvedPlatform)> {
    let released = super::qemu_config::ensure_released(ws_config_dir).map_err(|e| {
        anyhow::anyhow!(
            "释放 VM QEMU 配置模板失败（{}）：{e}",
            ws_config_dir.display()
        )
    })?;
    let ws_cfg = ws_config_dir.join(super::qemu_config::CONFIG_FILE_NAME);
    let (resolved, source) = super::qemu_config::resolve_matrix(Some(&ws_cfg));
    let matrix = resolved
        .map_err(|e| anyhow::anyhow!("VM QEMU 安装矩阵解析失败（{}）：{e}", ws_cfg.display()))?;
    let key = super::qemu_config::host_key();
    let entry = super::qemu_config::host_entry(&matrix)
        .ok_or_else(|| super::qemu_config::unavailable_error(&matrix, &key, Some(&ws_cfg)))?;
    if !entry.produced {
        bail!(
            "平台 {key} 已标记 produced=false（安装面未成立）：{}",
            entry.note
        );
    }
    let meta = QemuResolvedPlatform {
        released,
        source,
        platform_key: key.clone(),
        kind: entry.kind,
        note: entry.note.clone(),
    };
    tracing::info!(
        "[vm-install] 平台矩阵命中 {key} kind={} source={:?} released={released}",
        meta.kind.as_str(),
        source
    );
    let report = match entry.kind {
        QemuInstallKind::Archive => install_qemu_archive(entry, dirs).await?,
        QemuInstallKind::Packages => install_qemu_packages(entry).await?,
        QemuInstallKind::None => {
            bail!("平台 {key} 登记为不支持（kind=none）：{}", entry.note)
        }
    };
    Ok((report, meta))
}

/// packages 形态按宿主 OS 分臂（运行时判别 + 每臂 cfg 门双形态——错误平台
/// 上臂函数诚实 bail 而非编译期消失，分派器全平台可编译）。
async fn install_qemu_packages(entry: &QemuPlatformEntry) -> Result<QemuInstallReport> {
    match std::env::consts::OS {
        "linux" => install_qemu_packages_linux(entry).await,
        "android" => install_qemu_packages_termux(entry).await,
        "windows" => install_qemu_packages_pacman(entry).await,
        other => bail!("平台 {other} 无 packages 安装臂（矩阵 kind=packages 但宿主无对应安装链）"),
    }
}

// ---------------------------------------------------------------------------
// 纯函数（单测钉死）
// ---------------------------------------------------------------------------

// （QEMU 关键二进制名对已矩阵化——`super::qemu_config::required_binaries_for`
// 按条目 binaries 取，安装与探针同源消费。）

/// 分块调度：`total_len` 均分为 `parallel` 块（余数并入末块），返回
/// `(start, len)` 序列。`parallel<=1` 或 `total_len==0` = 单块全覆盖。
pub fn plan_chunks(total_len: u64, parallel: usize) -> Vec<(u64, u64)> {
    if total_len == 0 {
        return vec![(0, 0)];
    }
    let n = parallel.max(1).min(total_len as usize);
    let base = total_len / n as u64;
    let mut chunks = Vec::with_capacity(n);
    let mut start = 0u64;
    for i in 0..n {
        // 余数摊进前 r 块（每块 +1），保证覆盖恰为 total_len。
        let extra = if (i as u64) < total_len % n as u64 {
            1
        } else {
            0
        };
        let len = base + extra;
        chunks.push((start, len));
        start += len;
    }
    chunks
}

/// 上游 sidecar 内容 → hex128（取首个空白分隔 token；合法性 = 128 位 hex）。
pub fn parse_sha512_sidecar(body: &str) -> Option<String> {
    let token = body.split_whitespace().next()?;
    if token.len() != 128 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(token.to_ascii_lowercase())
}

/// 在 `root` 下定位含 `filename` 的目录（广度优先，深度 ≤4，跳过
/// `$PLUGINSDIR`/`Uninstall` 等安装器噪音；命中含该文件的**最浅**目录——
/// NSIS 布局漂移（根/`qemu/` 子目录）鲁棒）。未命中 = Err。
pub fn locate_dir_containing(root: &Path, filename: &str) -> Result<PathBuf> {
    let skip = |name: &str| name.starts_with("$PLUGINSDIR") || name.starts_with("Uninstall");
    let mut queue = std::collections::VecDeque::from([(root.to_path_buf(), 0u8)]);
    while let Some((dir, depth)) = queue.pop_front() {
        if depth > 4 {
            continue;
        }
        let entries =
            std::fs::read_dir(&dir).with_context(|| format!("read_dir {}", dir.display()))?;
        let mut found_file = false;
        for entry in entries {
            let entry = entry.with_context(|| format!("read_dir entry in {}", dir.display()))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if skip(&name) {
                continue;
            }
            let p = entry.path();
            if p.is_dir() {
                queue.push_back((p, depth + 1));
            } else if name == filename {
                found_file = true;
            }
        }
        if found_file {
            return Ok(dir);
        }
    }
    bail!("在 {} 下（深度≤4）未找到 {filename}", root.display());
}

/// 文件 SHA-512 → hex128（流式，不整载）。
pub fn sha512_hex_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha512};
    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut h = Sha512::new();
    std::io::copy(&mut file, &mut h).with_context(|| format!("hash {}", path.display()))?;
    Ok(format!("{:x}", h.finalize()))
}

/// 文件 SHA-256 → hex64（manifest 钉基线用）。
pub fn sha256_hex_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut h = Sha256::new();
    std::io::copy(&mut file, &mut h).with_context(|| format!("hash {}", path.display()))?;
    Ok(format!("{:x}", h.finalize()))
}

// ---------------------------------------------------------------------------
// ranged 下载器
// ---------------------------------------------------------------------------

struct RangeProbe {
    content_length: u64,
    ranges_ok: bool,
}

/// HEAD 探测：Content-Length + Accept-Ranges（12 路并行的前提）。
async fn probe_range(client: &reqwest::Client, url: &str) -> Result<RangeProbe> {
    let resp = client
        .head(url)
        .send()
        .await
        .with_context(|| format!("HEAD {url}"))?;
    if !resp.status().is_success() {
        bail!("HEAD {url} → {}", resp.status());
    }
    let content_length = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .context("HEAD 响应缺 Content-Length（无法对账总长）")?;
    let ranges_ok = resp
        .headers()
        .get(reqwest::header::ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("bytes"));
    Ok(RangeProbe {
        content_length,
        ranges_ok,
    })
}

/// 下载单个分块到 `part_path`（流式写盘；写入字节数 != len = 块失败）。
/// part 文件已存在且尺寸吻合 = 跳过（断点续传语义）。
async fn download_chunk(
    client: &reqwest::Client,
    url: &str,
    start: u64,
    len: u64,
    part_path: &Path,
) -> Result<()> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;

    // 已完成分片 = 直接复用（重试/续传路径）。
    if let Ok(meta) = tokio::fs::metadata(part_path).await
        && meta.len() == len
    {
        return Ok(());
    }

    let end = start + len.saturating_sub(1);
    let resp = client
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
        .send()
        .await
        .with_context(|| format!("GET range {start}-{end}"))?;
    if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        bail!(
            "range 请求未回 206（{}，Accept-Ranges 假阳性？）",
            resp.status()
        );
    }

    let mut file = tokio::fs::File::create(part_path)
        .await
        .with_context(|| format!("create {}", part_path.display()))?;
    let mut stream = resp.bytes_stream();
    let mut written: u64 = 0;
    loop {
        let item = tokio::time::timeout(
            std::time::Duration::from_secs(CHUNK_READ_TIMEOUT_SECS),
            stream.next(),
        )
        .await
        .with_context(|| format!("分块 {start} 读超时（{CHUNK_READ_TIMEOUT_SECS}s 无进展）"))?;
        match item {
            Some(Ok(bytes)) => {
                file.write_all(&bytes)
                    .await
                    .with_context(|| format!("write {}", part_path.display()))?;
                written += bytes.len() as u64;
            }
            Some(Err(e)) => return Err(e).context(format!("分块 {start} 流读失败")),
            None => break,
        }
    }
    file.flush().await.context("flush part")?;
    drop(file);
    if written != len {
        bail!("分块 {start} 字节数不符：写 {written} / 期望 {len}");
    }
    Ok(())
}

/// ranged 并行下载：Accept-Ranges 通过 = N 路分块（`.partN` 分片，重试跳过
/// 完整分片）；否则单线流式（诚实回落）。完成后顺序拼接 + 总长对账。
/// 返回落盘字节数。`dest` 已存在时覆盖。
pub async fn ranged_download(url: &str, dest: &Path, parallel: usize) -> Result<u64> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build http client")?;

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }

    let probe = probe_range(&client, url).await?;

    if !probe.ranges_ok || parallel <= 1 {
        // 单线诚实回落（weilnetz 慢——但能力缺失时不假装并行）。
        tracing::info!(
            "[vm-install] {url} 不支持 range（或 parallel=1），单线下载 {} 字节",
            probe.content_length
        );
        let part = dest.with_extension("part0");
        download_single_line(&client, url, &part).await?;
        verify_len(&part, probe.content_length).await?;
        tokio::fs::rename(&part, dest)
            .await
            .with_context(|| format!("rename {} -> {}", part.display(), dest.display()))?;
        return Ok(probe.content_length);
    }

    let chunks = plan_chunks(probe.content_length, parallel);
    tracing::info!(
        "[vm-install] {url}: {} 字节 → {} 路并行分块",
        probe.content_length,
        chunks.len()
    );

    let mut handles = Vec::with_capacity(chunks.len());
    for (i, (start, len)) in chunks.iter().copied().enumerate() {
        let client = client.clone();
        let url = url.to_string();
        let part = dest.with_extension(format!("part{i}"));
        handles.push(tokio::spawn(async move {
            download_chunk(&client, &url, start, len, &part)
                .await
                .map_err(|e| (i, e))
        }));
    }
    let results = futures::future::join_all(handles).await;
    let mut failures: Vec<(usize, anyhow::Error)> = Vec::new();
    for r in results {
        match r {
            Ok(Ok(())) => {}
            Ok(Err((i, e))) => failures.push((i, e)),
            Err(e) => failures.push((usize::MAX, anyhow::anyhow!("chunk task join: {e}"))),
        }
    }
    if !failures.is_empty() {
        // 分片留盘（完整分片下次跳过 = 断点续传）。
        let detail = failures
            .iter()
            .map(|(i, e)| format!("chunk#{i}: {e}"))
            .collect::<Vec<_>>()
            .join("; ");
        bail!(
            "并行下载 {} 块失败 {}/{}：{detail}（分片已留盘，重试跳过完整分片）",
            parallel,
            failures.len(),
            chunks.len()
        );
    }

    // 拼接（顺序 = 分块序；io::copy 流式，不整载）。
    use tokio::io::AsyncWriteExt;
    let concat_dest = dest.with_extension("part.concat");
    {
        let mut out = tokio::fs::File::create(&concat_dest)
            .await
            .with_context(|| format!("create {}", concat_dest.display()))?;
        for i in 0..chunks.len() {
            let part = dest.with_extension(format!("part{i}"));
            let mut f = tokio::fs::File::open(&part)
                .await
                .with_context(|| format!("open {}", part.display()))?;
            tokio::io::copy(&mut f, &mut out)
                .await
                .with_context(|| format!("concat part{i}"))?;
        }
        out.flush().await.context("flush concat")?;
    }
    verify_len(&concat_dest, probe.content_length).await?;
    tokio::fs::rename(&concat_dest, dest)
        .await
        .with_context(|| format!("rename concat -> {}", dest.display()))?;
    // 分片已并入，清盘（尽力而为）。
    for i in 0..chunks.len() {
        let _ = tokio::fs::remove_file(dest.with_extension(format!("part{i}"))).await;
    }
    Ok(probe.content_length)
}

/// 单线流式下载（无 range 能力时的诚实回落）。
async fn download_single_line(client: &reqwest::Client, url: &str, dest: &Path) -> Result<()> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;

    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        bail!("GET {url} → {}", resp.status());
    }
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("create {}", dest.display()))?;
    let mut stream = resp.bytes_stream();
    loop {
        let item = tokio::time::timeout(
            std::time::Duration::from_secs(CHUNK_READ_TIMEOUT_SECS),
            stream.next(),
        )
        .await
        .context("单线下载读超时（120s 无进展）")?;
        match item {
            Some(Ok(bytes)) => file.write_all(&bytes).await.context("write")?,
            Some(Err(e)) => return Err(e).context("流读失败"),
            None => break,
        }
    }
    file.flush().await.context("flush")?;
    Ok(())
}

async fn verify_len(path: &Path, expected: u64) -> Result<()> {
    let actual = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("stat {}", path.display()))?
        .len();
    if actual != expected {
        bail!(
            "Content-Length 对账失败：{} 实际 {actual} / 期望 {expected}",
            path.display()
        );
    }
    Ok(())
}

/// 取上游 sidecar SHA-512（`<url>.sha512`）。不可达 = Err（caller 决定
/// 诚实拒绝或以显式 expected 继续）。
pub async fn fetch_sha512_sidecar(url: &str) -> Result<String> {
    let sidecar_url = format!("{url}.sha512");
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .context("build http client")?;
    let resp = client
        .get(&sidecar_url)
        .send()
        .await
        .with_context(|| format!("GET {sidecar_url}"))?;
    if !resp.status().is_success() {
        bail!("sidecar {sidecar_url} → {}", resp.status());
    }
    let body = resp.text().await.context("read sidecar body")?;
    parse_sha512_sidecar(&body).context("sidecar 内容不含合法 hex128")
}

// ---------------------------------------------------------------------------
// archive 安装链（下载 + 校验 + 免安装直解；平台无关）
// ---------------------------------------------------------------------------

/// 安装包临时命名（work_dir 内暂存，装完清盘）。
const INSTALLER_NAME: &str = "qemu-installer.bin";
const UNPACK_DIR_NAME: &str = "unpack";

/// 安装包在侧校验（钉值/sidecar 双锚），不一致 = Err（错误信息带两侧值）。
async fn verify_installer(path: &Path, expected: &Option<String>, url: &str) -> Result<()> {
    let actual = tokio::task::spawn_blocking({
        let path = path.to_path_buf();
        move || sha512_hex_file(&path)
    })
    .await
    .context("hash task join")??;

    match expected {
        Some(want) => {
            let want_lc = want.to_ascii_lowercase();
            if actual == want_lc {
                return Ok(());
            }
            // 钉值不符：顺带取 sidecar 丰富诊断（上游已更新？传输损坏？）。
            let sidecar = fetch_sha512_sidecar(url).await.ok();
            match sidecar {
                Some(sc) if sc == actual => bail!(
                    "下载文件 SHA-512 与本仓钉值不符，但与上游 sidecar 一致——\
                     上游发行已更新（钉 {} / 实测 {actual}）。请确认新版本后更新钉值，\
                     或显式换源并给 expected_sha512",
                    &want_lc[..16]
                ),
                _ => bail!(
                    "下载文件 SHA-512 校验失败：钉 {} / 实测 {actual}（sidecar 不可达或同样不符）",
                    &want_lc[..16]
                ),
            }
        }
        None => {
            let sidecar = fetch_sha512_sidecar(url).await?;
            if sidecar != actual {
                bail!(
                    "下载文件 SHA-512 与上游 sidecar 不符：sidecar {} / 实测 {actual}",
                    &sidecar[..16]
                );
            }
            Ok(())
        }
    }
}

/// archive 安装链（下载 + 校验 + 免安装直解；平台无关——矩阵条目带 url 即走，
/// 当前消费形态 windows-x86_64 weilnetz NSIS；统一入口
/// [`install_qemu_for_host_workspace`]）。
///
/// 步骤：幂等检查 → 7z 解析（复用 sandbox 缓存）→ 下载+校验 → 7z 直解 →
/// 定位 qemu 根目录 → rename 落位 → `--version` 探针 → SHA-256 钉 manifest。
async fn install_qemu_archive(
    entry: &QemuPlatformEntry,
    dirs: &QemuInstallDirs,
) -> Result<QemuInstallReport> {
    let (sys_name, img_name) = super::qemu_config::required_binaries_for(entry)?;
    // 归一为 owned（spawn_blocking 闭包要求 'static；条目引用不出函数体）。
    let qemu_name = sys_name.to_string();
    let img_name = img_name.to_string();
    // sha512 钉值：条目显式给出则用（小写归一）；空 = 回落上游 sidecar 查表
    // （verify_installer 的 None 臂）。
    let expected_sha512 = if entry.sha512.trim().is_empty() {
        None
    } else {
        Some(entry.sha512.to_ascii_lowercase())
    };

    // 幂等：已就绪 = 版本探针过即早退（不重装）。
    let qemu_system = dirs.install_dir.join(&qemu_name);
    if qemu_system.exists() {
        let version = super::probe::qemu_version_ok(&qemu_system)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{} 已存在但不可执行（{e}）；请删除 {} 后重试安装",
                    qemu_system.display(),
                    dirs.install_dir.display()
                )
            })?;
        return Ok(QemuInstallReport {
            install_dir: dirs.install_dir.clone(),
            qemu_system,
            qemu_img: dirs.install_dir.join(img_name),
            version,
            installer_sha256: String::new(),
            already_installed: true,
        });
    }

    // 残 install_dir（有目录无二进制）= 半安装态，清掉再装。
    if dirs.install_dir.exists()
        && !nemesis_utils::fsutil::remove_dir_all_resilient(&dirs.install_dir)
    {
        bail!(
            "清理半安装目录失败：{}（Windows 瞬时句柄占用？）",
            dirs.install_dir.display()
        );
    }

    tokio::fs::create_dir_all(&dirs.work_dir)
        .await
        .with_context(|| format!("create work dir {}", dirs.work_dir.display()))?;

    // 1. 7z（复用 sandbox 缓存：cached → system → 按需下载）。
    let seven_zip = crate::extract::resolve_seven_zip(&dirs.seven_zip_dir).await?;

    // 2. 下载 + 校验。
    let installer = dirs.work_dir.join(INSTALLER_NAME);
    let prior = tokio::fs::metadata(&installer).await.ok().map(|m| m.len());
    if prior.is_none() {
        ranged_download(&entry.url, &installer, dirs.parallel).await?;
    }
    if let Err(e) = verify_installer(&installer, &expected_sha512, &entry.url).await {
        // 坏包不滞留：留在 work_dir 的 installer 会让下次运行走
        // `prior.is_some()` 跳过重下，永远卡在同一坏包现场——删掉再报错，
        // 下轮重新下载。
        let _ = tokio::fs::remove_file(&installer).await;
        return Err(e);
    }

    // 3. 7z 直解（NSIS 免安装直跑——P5 实证；sync IO → spawn_blocking）。
    let unpack = dirs.work_dir.join(UNPACK_DIR_NAME);
    if unpack.exists() && !nemesis_utils::fsutil::remove_dir_all_resilient(&unpack) {
        bail!("清理旧解包目录失败：{}", unpack.display());
    }
    let installer_c = installer.clone();
    let unpack_c = unpack.clone();
    let seven_zip_c = seven_zip.clone();
    tokio::task::spawn_blocking(move || {
        crate::extract::extract(&installer_c, &unpack_c, &seven_zip_c)
    })
    .await
    .context("extract task join")??;

    // 4. 定位 qemu 根（NSIS 布局漂移鲁棒）+ 完整目录落位（保 pc-bios/DLL 邻居）。
    let qemu_root = tokio::task::spawn_blocking({
        let unpack = unpack.clone();
        let qemu_name = qemu_name.clone();
        move || locate_dir_containing(&unpack, &qemu_name)
    })
    .await
    .context("locate task join")??;
    if let Some(parent) = dirs.install_dir.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }
    tokio::fs::rename(&qemu_root, &dirs.install_dir)
        .await
        .with_context(|| {
            format!(
                "rename {} -> {}（跨卷？install_dir 与 work_dir 应同卷）",
                qemu_root.display(),
                dirs.install_dir.display()
            )
        })?;

    // 5. 最小文件集 + 版本探针（二进制在场 ≠ 可用，实跑对齐 diff 纪律）。
    let qemu_system = dirs.install_dir.join(&qemu_name);
    let qemu_img = dirs.install_dir.join(&img_name);
    if !qemu_img.exists() {
        bail!(
            "解包产物缺 {img_name}（{}）——安装包布局异常",
            qemu_img.display()
        );
    }
    let version = super::probe::qemu_version_ok(&qemu_system)
        .await
        .map_err(anyhow::Error::msg)?;

    // 6. SHA-256 钉基线 + manifest（后续重装对账）。
    let installer_sha256 = tokio::task::spawn_blocking({
        let installer = installer.clone();
        move || sha256_hex_file(&installer)
    })
    .await
    .context("sha256 task join")??;

    let manifest = serde_json::json!({
        "component": "qemu",
        "platform": super::qemu_config::host_key(),
        "url": entry.url,
        "sha512": expected_sha512.clone().unwrap_or_default(),
        "layout": entry.layout,
        "sha256_installer": installer_sha256,
        "version": version,
        "installed_at": chrono::Utc::now().to_rfc3339(),
    });
    let manifest_path = dirs.install_dir.join("manifest.json");
    tokio::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)
        .await
        .with_context(|| format!("write {}", manifest_path.display()))?;

    // 7. 暂存清盘（installer/unpack；fsutil 韧性删，尽力而为；空壳目录
    //    一并收走——remove_dir 只删空目录，非空即留不给误删面）。
    let _ = nemesis_utils::fsutil::remove_dir_all_resilient(&unpack);
    let _ = tokio::fs::remove_file(&installer).await;
    let _ = tokio::fs::remove_dir(&dirs.work_dir).await;

    tracing::info!(
        "[vm-install] QEMU 就绪：{}（{version}）",
        dirs.install_dir.display()
    );
    Ok(QemuInstallReport {
        install_dir: dirs.install_dir.clone(),
        qemu_system,
        qemu_img,
        version,
        installer_sha256,
        already_installed: false,
    })
}

// ---------------------------------------------------------------------------
// B2：引导命令构造（纯函数——CLI/WSAPI 引导文案与真实执行同源产出）
// ---------------------------------------------------------------------------

/// WHPX 的 Windows 可选功能名（DISM 单一真相源）。
pub const DISM_WHPX_FEATURE: &str = "HypervisorPlatform";

/// DISM 启用 WHPX 参数表（`/NoRestart`：重启时机由用户决定，不替用户拍板）。
pub fn dism_enable_whpx_args() -> Vec<String> {
    vec![
        "/Online".into(),
        "/NoRestart".into(),
        "/Enable-Feature".into(),
        format!("/FeatureName:{DISM_WHPX_FEATURE}"),
        "/All".into(),
    ]
}

/// DISM 查询 WHPX 功能状态参数表（只读；非提权上下文可能 740 拒绝——
/// 调用方按 None 诚实降级，不假装分得清）。
pub fn dism_query_state_args() -> Vec<String> {
    vec![
        "/Online".into(),
        "/Get-FeatureInfo".into(),
        format!("/FeatureName:{DISM_WHPX_FEATURE}"),
    ]
}

/// DISM `/Get-FeatureInfo` 的功能状态（解析失败 = None，诚实未知）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DismFeatureState {
    /// 功能已启用（含 Disable Pending——禁用挂起仍算在启用态）。
    Enabled,
    /// 启用已挂起（待重启生效）。
    EnablePending,
    /// 功能未启用。
    Disabled,
}

/// 解析 DISM 输出里的 `State :` 行（英文字段名；本地化 Windows 解析不到 =
/// None，调用方诚实降级，不瞎猜）。
pub fn parse_dism_feature_state(output: &str) -> Option<DismFeatureState> {
    for line in output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("State :") {
            return match rest.trim() {
                "Enabled" => Some(DismFeatureState::Enabled),
                "Enabled with Disable Pending" | "Disable Pending" => {
                    Some(DismFeatureState::Enabled)
                }
                "Enable Pending" => Some(DismFeatureState::EnablePending),
                "Disabled" => Some(DismFeatureState::Disabled),
                _ => None,
            };
        }
    }
    None
}

/// Linux CPU vendor（`/proc/cpuinfo` 的 `vendor_id` 行；找不到 = None）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuVendor {
    Intel,
    Amd,
}

pub fn parse_cpu_vendor(cpuinfo: &str) -> Option<CpuVendor> {
    for line in cpuinfo.lines() {
        if let Some(rest) = line.strip_prefix("vendor_id") {
            let v = rest
                .trim_start_matches(|c: char| c == ':' || c.is_whitespace())
                .trim();
            return match v {
                "GenuineIntel" => Some(CpuVendor::Intel),
                "AuthenticAMD" => Some(CpuVendor::Amd),
                _ => None,
            };
        }
    }
    None
}

/// CPU vendor → KVM 内核模块名（modprobe 引导用；未知 vendor = None 不瞎猜）。
pub fn kvm_module_for_vendor(vendor: CpuVendor) -> &'static str {
    match vendor {
        CpuVendor::Intel => "kvm_intel",
        CpuVendor::Amd => "kvm_amd",
    }
}

/// Linux 包管理器形态（PATH 探测；未识别 = Unknown 诚实指路，不瞎猜命令）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxPm {
    Apt,
    Dnf,
    Unknown,
}

/// 引导命令串（人读文案 + CLI 呈现；包清单由调用方从矩阵条目按 pm 取——
/// 单一真相源住 config.vm_qemu.json；`sudo` 前缀统一——自动执行分支在
/// 免密 sudo 下同样经 sudo -n 走，语义一致）。
///
/// ⚠ 文案是**终端执行**形态，与程序化链（[`install_qemu_packages_linux`]）
/// 形式有意分化：终端 sudo 可要密码，`env` 中转形式必行；`sudo -n` 无法
/// 交互，程序化链因此不带 DEBIAN_FRONTEND（见彼处注释）。
pub fn pm_install_command(pm: LinuxPm, pkgs: &str) -> Option<String> {
    match pm {
        // DEBIAN_FRONTEND 压掉交互式确认。用 `env` 中转而非 `sudo VAR=val
        // cmd` 前缀：默认 sudoers（env_reset，无 SETENV tag）下 sudo 拒绝
        // 设置不在 env_keep 的变量（前缀形式与密码无关必炸），env 中转
        // 形式终端必行。
        LinuxPm::Apt => Some(format!(
            "sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y {pkgs}"
        )),
        LinuxPm::Dnf => Some(format!("sudo dnf install -y {pkgs}")),
        LinuxPm::Unknown => None,
    }
}

/// KVM 内核模块装载引导命令。
pub fn modprobe_kvm_command(module: &str) -> String {
    format!("sudo modprobe {module}")
}

/// kvm 组成员引导命令（生效需重新登录——文案由呈现层带）。
pub fn usermod_kvm_command(user: &str) -> String {
    format!("sudo usermod -aG kvm {user}")
}

/// Linux 包管理器探测（PATH 上 apt-get / dnf 谁在场；都无 = Unknown）。
#[cfg(target_os = "linux")]
pub fn detect_linux_pm() -> LinuxPm {
    for candidate in ["/usr/bin", "/bin", "/usr/local/bin"] {
        if Path::new(candidate).join("apt-get").exists() {
            return LinuxPm::Apt;
        }
        if Path::new(candidate).join("dnf").exists() {
            return LinuxPm::Dnf;
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            if dir.join("apt-get").exists() {
                return LinuxPm::Apt;
            }
            if dir.join("dnf").exists() {
                return LinuxPm::Dnf;
            }
        }
    }
    LinuxPm::Unknown
}

/// 免密 sudo 探测（`sudo -n true` 退出码 0 = 可全自动安装；非 0 = 半自动，
/// 展示命令让用户自己执行——不假装能绕过 root）。
#[cfg(target_os = "linux")]
pub async fn passwordless_sudo_available() -> bool {
    tokio::process::Command::new("sudo")
        .arg("-n")
        .arg("true")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// B2：平台安装链
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// B2：packages 安装臂（Linux apt/dnf / Android Termux / Windows MSYS2 pacman）
// ---------------------------------------------------------------------------

/// packages 条目按 pm 取包清单（空 = None——矩阵没给该 pm 的清单，诚实指路
/// 补全配置；单一真相源 = config.vm_qemu.json 条目字段）。
#[cfg(target_os = "linux")]
fn packages_for_pm(entry: &QemuPlatformEntry, pm: LinuxPm) -> Option<&str> {
    let raw = match pm {
        LinuxPm::Apt => entry.apt.as_str(),
        LinuxPm::Dnf => entry.dnf.as_str(),
        LinuxPm::Unknown => return None,
    };
    let pkgs = raw.trim();
    if pkgs.is_empty() { None } else { Some(pkgs) }
}

/// Linux QEMU 包安装链（`#[cfg(linux)]`——非 Linux 编译期消失，分派器 stub
/// 兜底）。
///
/// 全自动分支：识别包管理器 + 免密 sudo 可用 → apt/dnf 非交互安装 →
/// PATH 二进制实跑探针。半自动分支：免密 sudo 不可用 → 诚实报错，错误
/// 信息自带精确命令（用户终端执行后探针复检转绿）。
#[cfg(target_os = "linux")]
async fn install_qemu_packages_linux(entry: &QemuPlatformEntry) -> Result<QemuInstallReport> {
    let pm = detect_linux_pm();
    let Some(pkgs) = packages_for_pm(entry, pm) else {
        bail!(
            "矩阵条目未含本机包管理器的包清单（或未识别包管理器）——请在 \
             config.vm_qemu.json 本平台条目补全 apt（Debian/Ubuntu）或 dnf（Fedora）\
             字段后重试（note：{}）",
            entry.note
        );
    };
    let Some(cmd) = pm_install_command(pm, pkgs) else {
        bail!(
            "未识别包管理器（非 apt/dnf 系）——请按发行版惯例安装 {} \
             （官方下载：https://qemu.org/download/），装好后复检",
            entry.binaries.join(" / ")
        );
    };
    if !passwordless_sudo_available().await {
        bail!(
            "免密 sudo 不可用（半自动模式）：请在终端执行 `{cmd}` 后复检——\
             不假装能绕过 root 鉴权"
        );
    }

    // 非交互拆分执行。⚠ 不用 `sudo VAR=val apt-get` env 前缀形式：默认
    // sudoers（env_reset，无 SETENV tag）下 sudo 拒绝设置未入 env_keep 的
    // 变量——NOPASSWD 放行 apt-get 也照样拦（远程回归实证：`sudo: you are
    // not allowed to set the following environment variables:
    // DEBIAN_FRONTEND`）。非交互性由 -y + sudo -n + 无 tty 下 debconf
    // noninteractive fallback 保证（qemu 包集无 debconf 交互）。
    // 包名单从矩阵条目派生（单一真相源，不重复硬编码）。
    let packages: Vec<String> = pkgs.split_whitespace().map(str::to_string).collect();
    let (program, mut args) = match pm {
        LinuxPm::Apt => (
            "sudo",
            vec![
                "-n".to_string(),
                "apt-get".to_string(),
                "install".to_string(),
                "-y".to_string(),
            ],
        ),
        LinuxPm::Dnf => (
            "sudo",
            vec![
                "-n".to_string(),
                "dnf".to_string(),
                "install".to_string(),
                "-y".to_string(),
            ],
        ),
        LinuxPm::Unknown => unreachable!("Unknown 已在上方 bail"),
    };
    args.extend(packages);
    tracing::info!("[vm-install] Linux 包安装开始：{cmd}");
    let out = tokio::process::Command::new(program)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("包管理器执行失败: {e}"))?;
    if !out.status.success() {
        bail!(
            "包安装失败（{}）：{}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    // 装后裁决：PATH 二进制实跑探针（包管理器说装好 ≠ 二进制可用）。
    let (sys_name, img_name) = super::qemu_config::required_binaries_for(entry)?;
    let bin_dir = super::probe::find_binary_dir_on_path(sys_name).ok_or_else(|| {
        anyhow::anyhow!("包已安装但 {sys_name} 不在 PATH 上（PATH 未刷新？重试复检）")
    })?;
    let sys_path = bin_dir.join(sys_name);
    let version = super::probe::qemu_version_ok(&sys_path)
        .await
        .map_err(|e| {
            anyhow::anyhow!("包已安装但 {sys_name} 实跑探针失败：{e}（PATH 未刷新？重试复检）")
        })?;
    Ok(QemuInstallReport {
        qemu_img: bin_dir.join(img_name),
        install_dir: bin_dir,
        qemu_system: sys_path,
        version,
        installer_sha256: String::new(),
        already_installed: false,
    })
}

/// 非 Linux 编译目标：诚实 bail（分派器全平台可编译的 stub 臂）。
#[cfg(not(target_os = "linux"))]
async fn install_qemu_packages_linux(_entry: &QemuPlatformEntry) -> Result<QemuInstallReport> {
    bail!("当前编译目标非 Linux：apt/dnf 安装臂不可用")
}

/// Android（Termux）QEMU 包安装链（`#[cfg(android)]`）。
///
/// Termux prefix 无 sudo（单用户环境），`apt-get install -y` 直跑；
/// `TERMUX_VERSION` 在场 = Termux 环境校验（缺席 = 诚实拒绝——Android target
/// 不必然是 Termux，不瞎猜 prefix）。
#[cfg(target_os = "android")]
async fn install_qemu_packages_termux(entry: &QemuPlatformEntry) -> Result<QemuInstallReport> {
    if std::env::var_os("TERMUX_VERSION").is_none() {
        bail!(
            "TERMUX_VERSION 不在场：Android 臂只支持 Termux 环境（其他发行形态请\
             手动安装 {} 后复检）",
            entry.binaries.join(" / ")
        );
    }
    let pkgs = entry.apt.trim();
    if pkgs.is_empty() {
        bail!(
            "矩阵条目 apt 字段为空：请在 config.vm_qemu.json 本平台条目补全 \
             （Termux 仓库用 apt/pkg 前端；note：{}）",
            entry.note
        );
    }
    // Termux prefix（缺省官方布局，`PREFIX` env 可覆盖）。
    let prefix = std::env::var_os("PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/data/data/com.termux/files/usr"));
    tracing::info!("[vm-install] Termux 包安装开始：apt-get install -y {pkgs}");
    let out = tokio::process::Command::new("apt-get")
        .args(["install", "-y"])
        .args(pkgs.split_whitespace())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("apt-get 执行失败: {e}"))?;
    if !out.status.success() {
        bail!(
            "包安装失败（{}）：{}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    // 装后裁决：prefix/bin 实跑探针。
    let (sys_name, img_name) = super::qemu_config::required_binaries_for(entry)?;
    let bin = prefix.join("bin");
    let sys_path = bin.join(sys_name);
    let version = super::probe::qemu_version_ok(&sys_path)
        .await
        .map_err(|e| anyhow::anyhow!("包已安装但 {} 实跑探针失败：{e}", sys_path.display()))?;
    Ok(QemuInstallReport {
        install_dir: prefix,
        qemu_system: sys_path,
        qemu_img: bin.join(img_name),
        version,
        installer_sha256: String::new(),
        already_installed: false,
    })
}

/// 非 Android 编译目标：诚实 bail（分派器全平台可编译的 stub 臂）。
#[cfg(not(target_os = "android"))]
async fn install_qemu_packages_termux(_entry: &QemuPlatformEntry) -> Result<QemuInstallReport> {
    bail!("当前编译目标非 Android：Termux 安装臂不可用")
}

/// Windows MSYS2 pacman 安装链（`#[cfg(windows)]`——windows-aarch64 走
/// CLANGARM64 原生 ARM64 构建，pacman 负责解 mingw 运行库依赖链）。
///
/// 定位 MSYS2 根（[`locate_msys2_root`]）→ `msys2_shell.cmd -clangarm64
/// -defterm -no-start -c "pacman -Sy --noconfirm <pkgs>"` → 装后探针
/// `<msys2>/clangarm64/bin`。**不自动安装 MSYS2 本体**（缺席 = 诚实指路）。
#[cfg(target_os = "windows")]
async fn install_qemu_packages_pacman(entry: &QemuPlatformEntry) -> Result<QemuInstallReport> {
    let pkgs = entry.pacman.trim();
    if pkgs.is_empty() {
        bail!(
            "矩阵条目 pacman 字段为空：请在 config.vm_qemu.json 本平台条目补全 \
             （note：{}）",
            entry.note
        );
    }
    let msys2 = locate_msys2_root().ok_or_else(|| {
        anyhow::anyhow!(
            "未找到 MSYS2（查过 NEMESISBOT_MSYS2_ROOT / C:\\msys64 / C:\\msys2）。\
             本平台 QEMU 走 MSYS2 CLANGARM64 包（{pkgs}），请安装 MSYS2 \
             （https://www.msys2.org）后重试；本程序不代装 MSYS2 本体"
        )
    })?;
    let shell = msys2.join("msys2_shell.cmd");
    if !shell.exists() {
        bail!(
            "MSYS2 根 {} 缺 msys2_shell.cmd——目录形态异常",
            shell.display()
        );
    }
    let script = format!("pacman -Sy --noconfirm {pkgs}");
    tracing::info!("[vm-install] MSYS2 pacman 安装开始：{script}");
    let out = tokio::process::Command::new("cmd")
        .arg("/C")
        .arg(&shell)
        .args(["-clangarm64", "-defterm", "-no-start", "-c", &script])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("msys2_shell.cmd 执行失败: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // keyring/签名首次初始化报错是 MSYS2 新装常态，给指引不替跑。
        let keyring_hint = if stderr.contains("keyring") || stderr.contains("signature") {
            "；若为 keyring/签名错误：在 MSYS2 shell 执行 `pacman -Sy msys2-keyring archlinux-keyring` 后重试"
        } else {
            ""
        };
        bail!("pacman 安装失败（{}）：{stderr}{keyring_hint}", out.status);
    }
    // 装后裁决：CLANGARM64 bin 实跑探针。
    let (sys_name, img_name) = super::qemu_config::required_binaries_for(entry)?;
    let bin = msys2.join("clangarm64").join("bin");
    let sys_path = bin.join(sys_name);
    let version = super::probe::qemu_version_ok(&sys_path)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "pacman 完成（可能为 no-op）但 {} 实跑探针失败：{e}",
                sys_path.display()
            )
        })?;
    Ok(QemuInstallReport {
        qemu_img: bin.join(img_name),
        install_dir: bin,
        qemu_system: sys_path,
        version,
        installer_sha256: String::new(),
        already_installed: false,
    })
}

/// 非 Windows 编译目标：诚实 bail（分派器全平台可编译的 stub 臂）。
#[cfg(not(target_os = "windows"))]
async fn install_qemu_packages_pacman(_entry: &QemuPlatformEntry) -> Result<QemuInstallReport> {
    bail!("当前编译目标非 Windows：MSYS2 pacman 安装臂不可用")
}

/// MSYS2 根定位（env `NEMESISBOT_MSYS2_ROOT` > `C:\msys64` > `C:\msys2`；
/// 判定 = `msys2_shell.cmd` 在场——目录在 ≠ 装了）。公开给探针同源消费。
#[cfg(target_os = "windows")]
pub fn locate_msys2_root() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(v) = std::env::var_os("NEMESISBOT_MSYS2_ROOT") {
        roots.push(PathBuf::from(v));
    }
    roots.push(PathBuf::from("C:\\msys64"));
    roots.push(PathBuf::from("C:\\msys2"));
    roots
        .into_iter()
        .find(|r| r.join("msys2_shell.cmd").exists())
}

/// Windows WHPX 启用链（`#[cfg(windows)]`）：DISM runas 提权 fire-and-forget
/// （复用 sandbox elevation 范式——gateway/CLI 进程自身不提权，UAC 弹框由
/// 用户确认）。完成态由探针复检裁决（`/NoRestart`：重启由用户决定）。
///
/// 返回引导说明（人读文案）；真实生效判定恒走 `probe::probe_whpx` 复检。
#[cfg(target_os = "windows")]
pub fn enable_accel_windows() -> Result<String> {
    if super::probe::whpx_dll_present() {
        return Ok("WHPX 已就绪（无需启用）".to_string());
    }
    // DISM 路径（System32 惯例位；WINDIR 缺席按 C:\Windows 兜底）。
    let windir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    let dism = windir.join("System32").join("Dism.exe");
    if !dism.exists() {
        bail!(
            "Dism.exe 不在场（{}）——请在管理员终端手动执行 DISM 启用",
            dism.display()
        );
    }
    let args = dism_enable_whpx_args();
    crate::elevation::relaunch_elevated(&dism, &args)?;
    Ok(
        "已发起 UAC 提权请求（请在弹框确认）：DISM 启用 HypervisorPlatform（/NoRestart）。\
         完成后如探针仍不绿：功能挂起重启则需重启 Windows；重启后仍缺席请进 BIOS \
         开启虚拟化（Intel VT-x / AMD-V）。"
            .to_string(),
    )
}

#[cfg(test)]
mod tests;
