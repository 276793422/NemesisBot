//! 编译期价目表内嵌（2026-09-07）：`$OUT_DIR/model_prices_embedded.json`
//! 由本脚本保证**永远存在**，`pricing.rs` 用 `include_str!(OUT_DIR)` 吃进
//! 二进制当内置兜底层。
//!
//! 三条路径（2026-10-22 起）：
//! - **默认（`NEMESIS_PRICES_REFRESH` 未设）**：纯离线——仓库快照
//!   `assets/model_prices_litellm.json` 字节拷贝到 OUT_DIR，零网络（本地
//!   /离线/受限网络构建永不因下载卡住或失败）。
//! - **`NEMESIS_PRICES_FILE=<路径>`**（优先级最高）：构建脚本预下载的本地
//!   文件——同一构建会话内的多次 cargo 调用（build-windows.bat 主构建 +
//!   vm-display pass）只在脚本里下载一次，本脚本读文件走与网络路径同一套
//!   过滤/合并/校验，失败 → `cargo:warning` + 快照兜底。读取/校验失败等同
//!   下载失败，绝不内嵌坏表。
//! - **`NEMESIS_PRICES_REFRESH=<任意非空值>`**（FILE 未设时的旧行为）：走
//!   镜像链下载 LiteLLM 最新表 → 过滤 → 合并精选补充 → 校验（≥100 条）→
//!   写 OUT_DIR。任一步失败 → `cargo:warning` + 快照兜底（fail-open：构建
//!   永不因下载失败而失败）。FILE 模式下 REFRESH 退化为纯「值变化 → 重跑」
//!   信号（脚本放内容指纹，表更新才重跑）。
//!
//! 产物格式 = LiteLLM 原始形状（与运行时下载层同构），解析只有
//! `parse_litellm_json` 一条路径。来源通过
//! `NEMESIS_PRICES_EMBED_SOURCE` 注入二进制（`embedded_source()`，CLI /
//! API 诚实显示）。
//!
//! 共享逻辑（过滤/合并/URL 表）在 `src/pricing_filter.rs`，`#[path]`
//! include 进本脚本——与 lib 单一真相源。

#[path = "src/pricing_filter.rs"]
mod pricing_filter;

use pricing_filter::{
    PRICE_MIRROR_URLS, filter_litellm_table, merge_extras, validate_filtered_table,
};

const SNAPSHOT_PATH: &str = "assets/model_prices_litellm.json";
const SNAPSHOT_META_PATH: &str = "assets/model_prices_litellm.meta.txt";
const EXTRAS_PATH: &str = "assets/model_prices_extras.json";
const OUT_FILE: &str = "model_prices_embedded.json";
/// 单镜像下载超时。镜像链最多 3 条，全坏的最坏停顿 ~90s——只在
/// REFRESH 路径（官方构建）发生，本地默认离线路径零网络。
const DOWNLOAD_TIMEOUT_SECS: u64 = 30;

fn main() {
    println!("cargo:rerun-if-env-changed=NEMESIS_PRICES_REFRESH");
    println!("cargo:rerun-if-env-changed=NEMESIS_PRICES_FILE");
    println!("cargo:rerun-if-changed={SNAPSHOT_PATH}");
    println!("cargo:rerun-if-changed={SNAPSHOT_META_PATH}");
    println!("cargo:rerun-if-changed={EXTRAS_PATH}");
    println!("cargo:rerun-if-changed=src/pricing_filter.rs");

    // pricing-embed 关：跳过快照拷贝/下载/注入（pricing.rs 的 include_str!
    // 与 env! 同被 cfg 掉，OUT_DIR 产物不需要存在）。
    if std::env::var_os("CARGO_FEATURE_PRICING_EMBED").is_none() {
        return;
    }

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let dst = std::path::Path::new(&out_dir).join(OUT_FILE);
    let snapshot =
        std::fs::read_to_string(SNAPSHOT_PATH).expect("bundled price snapshot must exist");
    let extras = std::fs::read_to_string(EXTRAS_PATH).expect("bundled price extras must exist");

    let refresh = std::env::var("NEMESIS_PRICES_REFRESH")
        .ok()
        .filter(|v| !v.trim().is_empty());

    let (body, source) = if let Some(path) = std::env::var("NEMESIS_PRICES_FILE")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        // 构建脚本预下载（会话内唯一网络点）：读本地文件，与网络路径同一套
        // 过滤/合并/校验；失败同样快照兜底（fail-open 语义不变）。
        match std::fs::read_to_string(&path)
            .map_err(|e| format!("read {path}: {e}"))
            .and_then(|raw| refine_raw(raw, &extras))
        {
            Ok((table, count)) => {
                let date = today();
                (
                    table,
                    format!("pre-downloaded {path} ({count} entries, build {date})"),
                )
            }
            Err(err) => {
                println!(
                    "cargo:warning=[pricing-embed] 预下载价目表不可用（{}），使用仓库快照兜底",
                    err
                );
                (snapshot, bundled_source())
            }
        }
    } else {
        match refresh {
            None => (snapshot, bundled_source()),
            Some(_) => match fetch_filter_merge(&extras) {
                Ok((table, url, count)) => {
                    let date = today();
                    (
                        table,
                        format!("downloaded {url} ({count} entries, build {date})"),
                    )
                }
                Err(err) => {
                    println!(
                        "cargo:warning=[pricing-embed] 价目表下载失败（{}），使用仓库快照兜底",
                        err
                    );
                    (snapshot, bundled_source())
                }
            },
        }
    };

    std::fs::write(&dst, &body).unwrap_or_else(|e| panic!("write {}: {e}", dst.display()));
    println!("cargo:rustc-env=NEMESIS_PRICES_EMBED_SOURCE={source}");
}

/// 快照兜底的来源标记（构建日志 / `prices list` 可追溯）。
fn bundled_source() -> String {
    match std::fs::read_to_string(SNAPSHOT_META_PATH) {
        Ok(meta) => format!("bundled snapshot ({})", meta.trim()),
        Err(_) => "bundled snapshot".to_string(),
    }
}

/// 镜像链逐条尝试：下载 → 过滤 → 合并补充 → 校验。全部失败 → Err
/// （各镜像错误拼接）。解析/校验失败等同该镜像失败，换下一条。
fn fetch_filter_merge(extras: &str) -> Result<(String, String, usize), String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("NemesisBot-build/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let mut errors = Vec::new();
    for url in PRICE_MIRROR_URLS {
        match try_one(&client, url, extras) {
            Ok((table, count)) => return Ok((table, (*url).to_string(), count)),
            Err(e) => errors.push(format!("{url}: {e}")),
        }
    }
    Err(errors.join("; "))
}

fn try_one(
    client: &reqwest::blocking::Client,
    url: &str,
    extras: &str,
) -> Result<(String, usize), String> {
    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("download: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let raw_bytes = resp.bytes().map_err(|e| format!("read body: {e}"))?;
    // 不走 resp.text()（按响应头 charset 解码，被干扰的响应头会让完好的
    // 字节也解码失败——真机实证）；LiteLLM 表是 UTF-8 JSON，直取字节。
    let raw = String::from_utf8_lossy(&raw_bytes);
    refine_raw(raw.into_owned(), extras)
}

/// 过滤 → 合并精选补充 → 校验（预下载文件路径与网络路径共用的唯一后处理）。
fn refine_raw(raw: String, extras: &str) -> Result<(String, usize), String> {
    let filtered = filter_litellm_table(&raw)?;
    let merged = merge_extras(&filtered, extras)?;
    let count = validate_filtered_table(&merged)?;
    Ok((merged, count))
}

/// UTC 日期（来源标记用；本地时区在构建机上无意义）。
fn today() -> String {
    // 简易 days-since-epoch → YYYY-MM-DD（build-deps 不引 chrono，保持
    // build 依赖面最小）。
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    civil_from_days(days)
}

/// Howard Hinnant 的 days→civil 算法（公有领域）。
fn civil_from_days(z: i64) -> String {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
