//! dispatch —— 协议解析 + 结构解码（T1.4 read_mem 协议面；call_fn/hook_capture
//! 随 M4 接入）。
//!
//! 职责切分（平台无关面）：帧 JSON 解析（[`json_get_str`]/[`json_get_u64`]）、
//! read_mem 应答解析（[`parse_read_mem_resp`]）、内存镜像 → profile 数据结构
//! 解码（[`decode_struct`]）。`read_mem` 发帧本体在 `pipe`（windows-only，
//! 依赖 PipeLink）；本模块保持平台中立——Linux nightly 全量测试可跑解码单测。
//!
//! 原语路由表（总控 §5）：
//! - `read_mem`：读内存 + 按 profile 数据结构解码（本模块 + pipe）；
//! - `call_fn`：M4（payload 侧 CreateRemoteThread 于目标内调函数 + 看门狗）；
//! - `hook_capture`：M4（trampoline 单点捕获）。

use nemesis_ziwei_profile::{DataStructure, FieldBase, FieldDecode, FieldType};

/// 管道/协议错误（**平台中立**——定义在 dispatch 供 Linux 测试引用；
/// `pipe` 模块 re-export 同名类型）。
#[derive(Debug, thiserror::Error)]
pub enum PipeError {
    #[error("管道连接超时（{0:?}）：payload 未在窗口内连回")]
    ConnectTimeout(std::time::Duration),
    #[error("握手超时")]
    HandshakeTimeout,
    #[error("握手令牌不匹配（疑似外来进程抢连，已断开）")]
    TokenMismatch,
    #[error("握手 PID 不匹配：连入={got} 期望={expect}")]
    PidMismatch { got: u32, expect: u32 },
    #[error("握手协议版本不匹配：got={got} expect={expect}")]
    ProtoMismatch { got: u32, expect: u32 },
    #[error("握手架构不匹配：got={got} expect={expect}")]
    ArchMismatch { got: String, expect: String },
    #[error("管道 IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("帧越界（len={0}）")]
    FrameTooBig(u32),
    #[error("对端已断开")]
    Closed,
    /// R5（2026-10-24 第四次外评）：上一交换被外部超时取消——迟到应答仍滞
    /// 留管道缓冲，与新请求的应答无法区分（帧协议无请求 ID）。继续使用 =
    /// 张冠李戴。诚实语义：链路报废，结果未知，重建会话（勿自动重发——
    /// read/call_fn 副作用不可幂等假设）。
    #[error(
        "链路应答归属不可靠（上一请求被超时打断，迟到应答无法与新请求区分）——结果未知，请重建会话，勿自动重发"
    )]
    LinkUnreliable,
    #[error("payload 报错: {0}")]
    Payload(String),
}

// ---------------------------------------------------------------------------
// 手工 JSON 取值（与 payload 同约定：值域不含转义字符；帧协议面共用）
// ---------------------------------------------------------------------------
pub fn json_get_str<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":\"");
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

pub fn json_get_u64(s: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\":");
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    let end = rest
        .find(|c: char| {
            !(c.is_ascii_digit()
                || c == 'x'
                || ('a'..='f').contains(&c)
                || ('A'..='F').contains(&c))
        })
        .unwrap_or(rest.len());
    let raw = &rest[..end];
    if let Some(hex) = raw.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else {
        raw.parse::<u64>().ok()
    }
}

/// read_mem 应答解析（供测试复用）。
pub fn parse_read_mem_resp(resp: &str) -> Result<Vec<u8>, PipeError> {
    if resp.contains("\"ok\":false") {
        let msg = json_get_str(resp, "error").unwrap_or("unknown");
        return Err(PipeError::Payload(msg.to_string()));
    }
    let data = json_get_str(resp, "data").unwrap_or("");
    base64_decode(data).map_err(|e| PipeError::Payload(format!("base64 解码失败: {e}")))
}

/// 按 profile 数据结构解码内存镜像 → JSON 对象。
///
/// 解码语义（FieldDecode）：
/// - Utf8 / Utf16：定长字符域（遇 NUL 截断）；
/// - UnixMs / UnixSec：整数 → RFC3339 字符串（附加 `_<name>_raw` 原值）；
/// - Hex：整数 → "0x..." 字符串；
/// - 数组（array_len>1）：值数组（标量）或拼合字符串（char/wchar）。
pub fn decode_struct(data: &[u8], ds: &DataStructure) -> Result<serde_json::Value, String> {
    let mut out = serde_json::Map::new();
    for f in &ds.fields {
        let val = decode_field(data, f.offset, &f.type_, f.decode)?;
        out.insert(f.name.clone(), val);
    }
    Ok(serde_json::Value::Object(out))
}

fn decode_field(
    data: &[u8],
    offset: u32,
    ft: &FieldType,
    decode: Option<FieldDecode>,
) -> Result<serde_json::Value, String> {
    let elem = ft.size_of() / ft.array_len.unwrap_or(1).max(1);
    let is_text = matches!(ft.base, FieldBase::Char | FieldBase::WChar);
    if is_text {
        // 文本域：整体一块解码（NUL 截断）
        let total = ft.size_of() as usize;
        let start = offset as usize;
        if start + total > data.len() {
            return Err(format!(
                "字段越界：offset={offset} size={total} data={}",
                data.len()
            ));
        }
        let raw = &data[start..start + total];
        let s = match ft.base {
            FieldBase::Char => {
                let end = raw.iter().position(|&b| b == 0).unwrap_or(total);
                String::from_utf8_lossy(&raw[..end]).to_string()
            }
            FieldBase::WChar => {
                let units: Vec<u16> = raw
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
                String::from_utf16_lossy(&units[..end])
            }
            _ => unreachable!(),
        };
        return Ok(serde_json::Value::String(s));
    }

    // 标量/标量数组
    let n = ft.array_len.unwrap_or(1);
    let mut vals = Vec::with_capacity(n as usize);
    for i in 0..n {
        let at = offset + i * elem;
        vals.push(decode_scalar(data, at, ft.base, decode)?);
    }
    if ft.array_len.is_some() {
        Ok(serde_json::Value::Array(vals))
    } else {
        Ok(vals.into_iter().next().unwrap_or(serde_json::Value::Null))
    }
}

fn decode_scalar(
    data: &[u8],
    offset: u32,
    base: FieldBase,
    decode: Option<FieldDecode>,
) -> Result<serde_json::Value, String> {
    let (start, size) = (offset as usize, elem_size(base) as usize);
    if start + size > data.len() {
        return Err(format!(
            "字段越界：offset={offset} size={size} data={}",
            data.len()
        ));
    }
    let raw = &data[start..start + size];
    let signed_i64 = |bytes: &[u8]| -> i64 {
        let mut v: u64 = 0;
        for (i, b) in bytes.iter().enumerate() {
            v |= u64::from(*b) << (8 * i);
        }
        let sign_bits = 64 - bytes.len() * 8;
        (v << sign_bits) as i64 >> sign_bits
    };
    let u = |bytes: &[u8]| -> u64 {
        let mut v: u64 = 0;
        for (i, b) in bytes.iter().enumerate() {
            v |= u64::from(*b) << (8 * i);
        }
        v
    };
    Ok(match base {
        FieldBase::U8 | FieldBase::U16 | FieldBase::U32 | FieldBase::U64 => {
            let v = u(raw);
            match decode {
                Some(FieldDecode::Hex) => serde_json::Value::String(format!("{v:#x}")),
                Some(FieldDecode::UnixMs) => unix_to_json(v, true),
                Some(FieldDecode::UnixSec) => unix_to_json(v, false),
                _ => serde_json::Value::from(v),
            }
        }
        FieldBase::I8 | FieldBase::I16 | FieldBase::I32 | FieldBase::I64 => {
            serde_json::Value::from(signed_i64(raw))
        }
        FieldBase::F32 => {
            let v = f32::from_le_bytes(raw.try_into().unwrap());
            serde_json::Value::from(v)
        }
        FieldBase::F64 => {
            let v = f64::from_le_bytes(raw.try_into().unwrap());
            serde_json::Value::from(v)
        }
        FieldBase::Bool => serde_json::Value::from(raw[0] != 0),
        FieldBase::Char | FieldBase::WChar => {
            // 单元素文本（array_len=1 时走这里）
            let s = match base {
                FieldBase::Char => String::from_utf8_lossy(raw).to_string(),
                _ => {
                    let units: Vec<u16> = raw
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    String::from_utf16_lossy(&units)
                }
            };
            serde_json::Value::String(s.trim_end_matches('\0').to_string())
        }
    })
}

fn unix_to_json(v: u64, is_ms: bool) -> serde_json::Value {
    use chrono::TimeZone;
    let secs = if is_ms { v / 1000 } else { v };
    let nsecs = if is_ms {
        ((v % 1000) * 1_000_000) as u32
    } else {
        0
    };
    match chrono::Utc.timestamp_opt(secs as i64, nsecs) {
        chrono::LocalResult::Single(dt) => serde_json::Value::String(dt.to_rfc3339()),
        _ => serde_json::Value::Null,
    }
}

fn elem_size(base: FieldBase) -> u32 {
    match base {
        FieldBase::U8 | FieldBase::I8 | FieldBase::Char | FieldBase::Bool => 1,
        FieldBase::U16 | FieldBase::I16 | FieldBase::WChar => 2,
        FieldBase::U32 | FieldBase::I32 | FieldBase::F32 => 4,
        FieldBase::U64 | FieldBase::I64 | FieldBase::F64 => 8,
    }
}

/// base64 解码（标准字母表；无外部依赖）。
pub fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Result<u32, String> {
        match c {
            b'A'..=b'Z' => Ok(u32::from(c - b'A')),
            b'a'..=b'z' => Ok(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Ok(u32::from(c - b'0') + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(format!("非法 base64 字符 {:#x}", c)),
        }
    }
    let bytes: Vec<u8> = s.bytes().filter(|&b| b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n: u32 = 0;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        match chunk.len() {
            4 => out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]),
            3 => out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8]),
            2 => out.push((n >> 16) as u8),
            _ => return Err("base64 长度非 4 的倍数".into()),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// M4：call_fn 应答三态 + hook 事件（拉取制）
// ---------------------------------------------------------------------------

/// call_fn 调用约定码（payload CONV_* 同源；0=msx64 1=stdcall 2=cdecl）。
/// 定义在平台中立面——conv_to_code 与 Linux 测试可直接引用。
pub const CONV_MSX64: u32 = 0;
pub const CONV_STDCALL: u32 = 1;
pub const CONV_CDECL: u32 = 2;

/// call_fn 应答（payload 三态：正常返回 / 硬件异常 VEH 救援 / 看门狗超时）。
#[derive(Debug, Clone, PartialEq)]
pub enum CallFnOutcome {
    Ok { ret: u64, elapsed_ms: u64 },
    Seh { code: u32, elapsed_ms: u64 },
    Timeout { waited_ms: u64 },
}

impl CallFnOutcome {
    /// JSON 呈现（工具/WSAPI 输出面）。
    pub fn to_json(&self, addr: u64) -> serde_json::Value {
        match self {
            CallFnOutcome::Ok { ret, elapsed_ms } => serde_json::json!({
                "ok": true, "addr": format!("{addr:#x}"), "ret": ret, "elapsed_ms": elapsed_ms,
            }),
            CallFnOutcome::Seh { code, elapsed_ms } => serde_json::json!({
                "ok": false, "addr": format!("{addr:#x}"), "seh": format!("0x{code:08X}"),
                "elapsed_ms": elapsed_ms,
                "note": "目标函数硬件异常；payload VEH 已救援，宿主未受影响",
            }),
            CallFnOutcome::Timeout { waited_ms } => serde_json::json!({
                "ok": false, "addr": format!("{addr:#x}"), "timeout": true, "waited_ms": waited_ms,
                "note": "调用超时；payload 线程未强杀（Win32 无安全手段），完成前新调用被单飞限制拒绝",
            }),
        }
    }
}

/// 解析 call_fn 应答（三态分派；`ok:false` 无 timeout/seh = payload 报错）。
pub fn parse_call_fn_resp(resp: &str) -> Result<CallFnOutcome, PipeError> {
    if resp.contains("\"ok\":true") {
        return Ok(CallFnOutcome::Ok {
            ret: json_get_u64(resp, "ret").unwrap_or(0),
            elapsed_ms: json_get_u64(resp, "elapsed_ms").unwrap_or(0),
        });
    }
    if resp.contains("\"timeout\":true") {
        return Ok(CallFnOutcome::Timeout {
            waited_ms: json_get_u64(resp, "waited_ms").unwrap_or(0),
        });
    }
    if let Some(seh) = json_get_str(resp, "seh") {
        return Ok(CallFnOutcome::Seh {
            code: u32::from_str_radix(seh.trim_start_matches("0x"), 16).unwrap_or(0),
            elapsed_ms: json_get_u64(resp, "elapsed_ms").unwrap_or(0),
        });
    }
    Err(PipeError::Payload(
        json_get_str(resp, "error")
            .unwrap_or("call_fn 应答不可解析")
            .to_string(),
    ))
}

/// 单条 hook 捕获事件（payload 环形缓冲，拉取制）。
#[derive(Debug, Clone, PartialEq)]
pub struct HookEvent {
    /// capture id（hook_set 分配）。
    pub id: u32,
    /// 写入序（payload 侧单调计数低 32 位）。
    pub seq: u32,
    /// 目标进程内触发线程 id。
    pub tid: u32,
    /// 前 4 个整型参数（x64 寄存器实参 / x86 栈实参）。
    pub regs: [u64; 4],
}

/// 解析 poll_events 应答 →（事件批，累计丢弃数）。
pub fn parse_poll_events_resp(resp: &str) -> Result<(Vec<HookEvent>, u64), PipeError> {
    if resp.contains("\"ok\":false") {
        return Err(PipeError::Payload(
            json_get_str(resp, "error")
                .unwrap_or("poll_events 失败")
                .to_string(),
        ));
    }
    // 事件对象值域无嵌套大括号（{id,seq,tid,regs:[..]}）——按 "},{" 切分；
    // regs 数组的 ']' 先于事件数组闭合，不能用首个 ']' 找事件区末尾——
    // 改以「id 键消失」判定事件区结束（截断/尾部字段自然 break）。
    let mut events = Vec::new();
    if let Some(pos) = resp.find("\"events\":[") {
        let rest = &resp[pos + "\"events\":[".len()..];
        for obj in rest.split("},{") {
            let obj = obj.trim_start_matches('{').trim_end_matches('}');
            let Some(id) = json_get_u64(obj, "id") else {
                break;
            };
            events.push(HookEvent {
                id: id as u32,
                seq: json_get_u64(obj, "seq").unwrap_or(0) as u32,
                tid: json_get_u64(obj, "tid").unwrap_or(0) as u32,
                regs: json_u64_array(obj, "regs").unwrap_or([0; 4]),
            });
        }
    }
    let dropped = json_get_u64(resp, "dropped").unwrap_or(0);
    Ok((events, dropped))
}

/// JSON 数组取前 4 个数值（与 payload json_args 同约定；非整数 → None）。
pub fn json_u64_array(s: &str, key: &str) -> Option<[u64; 4]> {
    let needle = format!("\"{key}\":[");
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    let end = rest.find(']')?;
    let mut out = [0u64; 4];
    for (n, part) in rest[..end].split(',').enumerate() {
        if n >= 4 {
            break;
        }
        let t = part.trim();
        if t.is_empty() {
            break;
        }
        let v = if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            u64::from_str_radix(hex, 16).ok()?
        } else {
            t.parse::<u64>().ok()?
        };
        out[n] = v;
    }
    Some(out)
}

#[cfg(test)]
mod tests;
