//! recover —— 失效找回的逐锚重扫引擎（T5.3）。
//!
//! 输入 = 旧 profile 锚集（中性 [`AnchorLite`] 形态，跨 crate 不耦合
//! ziwei-profile）+ 新目标 exe 文件字节；输出 = 每锚命中判定 + 整体
//! [`RecoverVerdict`]。
//!
//! v1 重扫是**原始字节级**（不依赖反汇编/函数恢复）：
//! - byte_pattern：可执行节通配扫描（`??` 逐字节掩码）；
//! - imm_const：可执行节 imm32 LE 原始字节扫描（诚实注记：非反汇编级，
//!   数据区同值不判——只扫 EXEC 节）；
//! - str_ref：可读数据节文本扫描（UTF-8 字节序列）；
//! - import_call_seq：导入表存在性判定（v1 不校验调用序列顺序，诚实注记）；
//! - bb_hash：v1 显式不支持——从投票中剔除并注记（不冒充 miss）。
//!
//! 判定（[`RecoverVerdict::verdict`]）只由**受支持的锚**投票：全命中 =
//! recoverable（可进用户确认）；有 miss = partially_recoverable；全 miss =
//! unrecoverable。最终 active/stale 状态机在门面（ziwei::verify）。

use serde::Serialize;

use crate::pe::{self, ImageMap, PeInfo};

/// 逐锚重扫结果。
#[derive(Debug, Clone, Serialize)]
pub struct AnchorRescan {
    /// 锚序号（输入锚集下标）。
    pub index: usize,
    /// 关联函数语义名。
    pub function: String,
    /// 锚种类（byte_pattern/imm_const/str_ref/import_call_seq/bb_hash）。
    pub kind: String,
    /// 命中（新镜像中找到锚载荷）。
    pub hit: bool,
    /// 命中位置 RVA（首个；miss 或多命中时也填 first/None）。
    pub found_rva: Option<u32>,
    /// 命中位置数（>1 = 锚不够独特，报告提示但不否决）。
    pub hit_count: usize,
    /// 语义/方法注记（确定性证据 + v1 边界诚实声明）。
    pub note: String,
}

/// 找回判定汇总。
#[derive(Debug, Clone, Serialize)]
pub struct RecoverVerdict {
    pub anchors_total: usize,
    /// 受支持且参与投票的锚数（bb_hash 剔除后）。
    pub anchors_supported: usize,
    pub anchors_hit: usize,
    /// recoverable / partially_recoverable / unrecoverable。
    pub verdict: String,
    pub per_anchor: Vec<AnchorRescan>,
}

/// 中性锚形态（门面从 profile::Anchor 映射；rev 不依赖 ziwei-profile）。
#[derive(Debug, Clone)]
pub struct AnchorLite {
    pub function: String,
    pub kind: String,
    pub pattern: Option<String>,
    pub text: Option<String>,
    pub value: Option<u64>,
    pub imports: Option<String>,
}

/// 从 exe 文件字节构建 ImageMap（头解析 + 按节表 raw_ptr/raw_size 提取节）。
pub fn load_image(file: &[u8]) -> Result<ImageMap, String> {
    let pe_info = PeInfo::parse(file).map_err(|e| format!("PE 头解析失败：{e}"))?;
    let mut blobs: Vec<(String, Vec<u8>)> = Vec::new();
    for sec in &pe_info.sections {
        if sec.raw_size == 0 {
            continue;
        }
        let start = sec.raw_ptr as usize;
        let end = start.saturating_add(sec.raw_size as usize);
        if end > file.len() {
            return Err(format!(
                "节 {} 越界（raw_ptr=0x{:x} raw_size=0x{:x} > 文件 {}B）",
                sec.name,
                sec.raw_ptr,
                sec.raw_size,
                file.len()
            ));
        }
        blobs.push((sec.name.clone(), file[start..end].to_vec()));
    }
    ImageMap::new(pe_info, blobs).map_err(|e| format!("镜像组装失败：{e}"))
}

/// 逐锚重扫全流程（T5.3 主入口）。
pub fn rescan(file: &[u8], anchors: &[AnchorLite]) -> Result<RecoverVerdict, String> {
    let img = load_image(file)?;
    let imports = pe::parse_imports_file(file, &img.pe).unwrap_or_default();
    let per_anchor: Vec<AnchorRescan> = anchors
        .iter()
        .enumerate()
        .map(|(i, a)| scan_anchor(&img, &imports, i, a))
        .collect();
    let supported: Vec<&AnchorRescan> = per_anchor.iter().filter(|r| !unsupported(r)).collect();
    let hits = supported.iter().filter(|r| r.hit).count();
    let verdict = if supported.is_empty() {
        "unrecoverable".to_string()
    } else if hits == supported.len() {
        "recoverable".to_string()
    } else if hits > 0 {
        "partially_recoverable".to_string()
    } else {
        "unrecoverable".to_string()
    };
    Ok(RecoverVerdict {
        anchors_total: anchors.len(),
        anchors_supported: supported.len(),
        anchors_hit: hits,
        verdict,
        per_anchor,
    })
}

/// bb_hash（v1 不支持）：不进投票，注记诚实声明。
fn unsupported(r: &AnchorRescan) -> bool {
    r.note.starts_with("bb_hash")
}

/// 单锚扫描分派。
fn scan_anchor(
    img: &ImageMap,
    imports: &[pe::ImportDll],
    index: usize,
    a: &AnchorLite,
) -> AnchorRescan {
    let mut r = AnchorRescan {
        index,
        function: a.function.clone(),
        kind: a.kind.clone(),
        hit: false,
        found_rva: None,
        hit_count: 0,
        note: String::new(),
    };
    match a.kind.as_str() {
        "byte_pattern" => {
            let Some(pat) = a.pattern.as_deref() else {
                r.note = "byte_pattern 锚缺 pattern".into();
                return r;
            };
            // 解析交给 profile 层同源函数（经门面映射时已带 mask）——
            // 这里本地再解析一次（hex + ??），避免 rev 依赖 ziwei-profile。
            match parse_pattern(pat) {
                Ok((bytes, mask)) => {
                    let hits = scan_exec(img, &bytes, &mask);
                    r.hit_count = hits.len();
                    r.found_rva = hits.first().copied();
                    r.hit = !hits.is_empty();
                    r.note = format!(
                        "通配模式扫描（{} 字节），命中 {} 处{}",
                        bytes.len(),
                        hits.len(),
                        if hits.len() > 1 {
                            "（多命中：锚独特性下降，人工复核）"
                        } else {
                            ""
                        }
                    );
                }
                Err(e) => r.note = format!("pattern 解析失败：{e}"),
            }
        }
        "imm_const" => {
            let Some(v) = a.value else {
                r.note = "imm_const 锚缺 value".into();
                return r;
            };
            if v > u32::MAX as u64 {
                r.note = format!("imm_const 0x{v:X} 超 imm32 域（v1 只扫 32 位立即数）");
                return r;
            }
            let bytes = (v as u32).to_le_bytes();
            let hits = scan_exec(img, &bytes, &[0xFF; 4]);
            r.hit_count = hits.len();
            r.found_rva = hits.first().copied();
            r.hit = !hits.is_empty();
            r.note = format!(
                "imm32 LE 原始字节扫描（非反汇编级，只扫 EXEC 节），0x{v:X} 命中 {} 处",
                hits.len()
            );
        }
        "str_ref" => {
            let Some(text) = a.text.as_deref() else {
                r.note = "str_ref 锚缺 text".into();
                return r;
            };
            // 空 text 与缺 text 同类（BUG-20261029-156）：save 期校验只保护
            // 本方写入，load 侧不校验——手改/旧版 profile 空串会一路到这里，
            // windows(0) 直接 panic。与缺 pattern/value/imports 注记分支对称。
            if text.is_empty() {
                r.note = "str_ref 锚 text 为空".into();
                return r;
            }
            let needle = text.as_bytes();
            let mut hits = Vec::new();
            for sec in &img.pe.sections {
                if sec.is_executable() || sec.raw_size == 0 {
                    continue;
                }
                let Some(blob) = img.read_rva(sec.rva, sec.raw_size as usize) else {
                    continue;
                };
                for (off, w) in blob.windows(needle.len()).enumerate() {
                    if w == needle {
                        // 命中 RVA 走 saturating（BUG-20261029-154 同族：损坏
                        // PE 节基址近顶 + 节内偏移可溢出）
                        hits.push(sec.rva.saturating_add(off as u32));
                    }
                }
            }
            hits.sort_unstable();
            hits.dedup();
            r.hit_count = hits.len();
            r.found_rva = hits.first().copied();
            r.hit = !hits.is_empty();
            r.note = format!("可读数据节文本扫描，{text:?} 命中 {} 处", hits.len());
        }
        "import_call_seq" => {
            let Some(seq) = a.imports.as_deref() else {
                r.note = "import_call_seq 锚缺 imports".into();
                return r;
            };
            // v1：序列中每个 dll!func 都在导入表 = 命中（不校验调用顺序，
            // 诚实注记）。寻找锚函数：序列成员调用点的共同函数不可得（无
            // 反汇编）——found_rva 置 None，只给布尔。
            let want: Vec<&str> = seq
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            let mut all_present = true;
            let mut missing: Vec<String> = Vec::new();
            for w in &want {
                let Some((dll, func)) = w.split_once('!') else {
                    all_present = false;
                    missing.push((*w).to_string());
                    continue;
                };
                let found = imports.iter().any(|d| {
                    d.dll.eq_ignore_ascii_case(dll) && d.funcs.iter().any(|(f, _)| f == func)
                });
                if !found {
                    all_present = false;
                    missing.push((*w).to_string());
                }
            }
            r.hit = all_present;
            r.hit_count = if all_present { 1 } else { 0 };
            r.note = if all_present {
                format!("导入表存在性判定（v1 不校验调用顺序）：{seq} 全部在表")
            } else {
                format!("导入表存在性判定：缺失 {missing:?}")
            };
        }
        "bb_hash" => {
            r.note =
                "bb_hash 重扫 v1 未实现（归一化基本块哈希依赖反汇编级重建）——本锚从投票剔除".into();
        }
        other => {
            r.note = format!("未知锚种类 {other:?}（从投票剔除）");
        }
    }
    r
}

/// 可执行节通配扫描（bytes/mask 等长；mask 0x00=通配 0xFF=精确）。
fn scan_exec(img: &ImageMap, bytes: &[u8], mask: &[u8]) -> Vec<u32> {
    let mut hits = Vec::new();
    for sec in &img.pe.sections {
        if !sec.is_executable() || sec.raw_size == 0 {
            continue;
        }
        let Some(blob) = img.read_rva(sec.rva, sec.raw_size as usize) else {
            continue;
        };
        if blob.len() < bytes.len() {
            continue;
        }
        for (off, w) in blob.windows(bytes.len()).enumerate() {
            if w.iter()
                .zip(bytes)
                .zip(mask)
                .all(|((&b, &p), &m)| b & m == p & m)
            {
                // saturating 防 u32 加法溢出（BUG-20261029-154 同族）
                hits.push(sec.rva.saturating_add(off as u32));
            }
        }
    }
    hits.sort_unstable();
    hits
}

/// hex + `??` 模式解析（带空格 / 连续 hex 双形态；与 profile 层 parse_byte_pattern 同源语义）。
fn parse_pattern(pat: &str) -> Result<(Vec<u8>, Vec<u8>), String> {
    let compact: String = pat.split_whitespace().collect();
    if compact.is_empty() || !compact.len().is_multiple_of(2) {
        return Err(format!("pattern 形态非法（空或奇数长）：{pat:?}"));
    }
    let bytes = compact.as_bytes();
    let mut out = Vec::with_capacity(compact.len() / 2);
    let mut mask = Vec::with_capacity(compact.len() / 2);
    for pair in bytes.chunks(2) {
        let (hi, lo) = (pair[0], pair[1]);
        let parse_nib = |c: u8| -> Result<Option<u8>, String> {
            match c {
                b'0'..=b'9' => Ok(Some(c - b'0')),
                b'a'..=b'f' => Ok(Some(c - b'a' + 10)),
                b'A'..=b'F' => Ok(Some(c - b'A' + 10)),
                b'?' => Ok(None),
                other => Err(format!(
                    "pattern 含非法字符 {:?}（只认 hex 与 ?）",
                    other as char
                )),
            }
        };
        match (parse_nib(hi), parse_nib(lo)) {
            (Ok(Some(h)), Ok(Some(l))) => {
                out.push(h << 4 | l);
                mask.push(0xFF);
            }
            (Ok(None), Ok(None)) => {
                out.push(0x00);
                mask.push(0x00);
            }
            (Ok(_), Ok(_)) => {
                return Err(format!(
                    "pattern 半通配 nibble 不支持（只认 ?? 整字节）：{pat:?}"
                ));
            }
            (Err(e), _) | (_, Err(e)) => return Err(e),
        }
    }
    Ok((out, mask))
}

#[cfg(test)]
mod tests;
