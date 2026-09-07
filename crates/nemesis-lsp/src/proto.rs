//! LSP wire protocol: Content-Length framing, JSON-RPC envelopes, and
//! response parsing for the four read-only operations (L1 / U19).
//!
//! Everything here is pure — no process, no async — so the framing and
//! parsing rules are unit-testable in isolation from any language server.

use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

/// Encode one message with LSP base-frame headers
/// (`Content-Length: <n>\r\n\r\n<body>`). Body length is the UTF-8 byte
/// length, per the LSP base protocol.
pub fn encode(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(msg).unwrap_or_default();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

/// Incremental frame decoder: push received bytes, pop decoded messages.
/// Handles partial headers, partial bodies, and multiple messages per push.
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Pop the next complete message, if one has fully arrived.
    /// `Err` only for malformed framing (no Content-Length / bad JSON body).
    pub fn next_message(&mut self) -> Result<Option<Value>, String> {
        // Header block terminates at the first empty line (\r\n\r\n in the
        // stream; searching for the first bare \r\n line works line-by-line).
        let mut pos = 0usize;
        let body_start = loop {
            let Some(nl) = find_subsequence(&self.buf[pos..], b"\r\n") else {
                return Ok(None); // incomplete header block
            };
            let line_start = pos;
            let line_end = pos + nl;
            if line_start == line_end {
                // empty line = end of headers; body starts after this \r\n
                break line_end + 2;
            }
            pos = line_end + 2;
        };

        // Parse Content-Length (case-insensitive name per the spec's ABNF
        // field-name rules; servers in the wild send exact case but be lenient).
        let mut content_length: Option<usize> = None;
        let headers = std::str::from_utf8(&self.buf[..body_start - 2])
            .map_err(|e| format!("non-UTF-8 headers: {e}"))?;
        for line in headers.split("\r\n") {
            if let Some((name, value)) = line.split_once(':')
                && name.trim().eq_ignore_ascii_case("content-length")
            {
                content_length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|e| format!("bad Content-Length {value:?}: {e}"))?,
                );
            }
        }
        let Some(len) = content_length else {
            return Err("message without Content-Length header".to_string());
        };
        if self.buf.len() < body_start + len {
            return Ok(None); // body still arriving
        }
        let body = self.buf[body_start..body_start + len].to_vec();
        self.buf.drain(..body_start + len);
        let msg: Value =
            serde_json::from_slice(&body).map_err(|e| format!("bad JSON body: {e}"))?;
        Ok(Some(msg))
    }
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------------
// JSON-RPC envelopes
// ---------------------------------------------------------------------------

/// A client→server request.
pub fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// A client→server notification (no id, no response expected).
pub fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

/// A client→server response answering a server→client request.
pub fn response_ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Classify an incoming (server→client) message relative to the request we
/// are waiting on.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// A response to our pending request (`pending_id` match). Carries
    /// `result` on success or `error` on JSON-RPC failure — inspect both.
    Response(Value),
    /// A server→client REQUEST expecting a response from us.
    ServerRequest { id: Value, method: String },
    /// A server→client notification (no response expected).
    Notification { method: String },
}

pub fn classify(msg: &Value, pending_id: i64) -> Incoming {
    if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
        if let Some(id) = msg.get("id") {
            Incoming::ServerRequest {
                id: id.clone(),
                method: method.to_string(),
            }
        } else {
            Incoming::Notification {
                method: method.to_string(),
            }
        }
    } else if msg.get("id").and_then(|i| i.as_i64()) == Some(pending_id) {
        Incoming::Response(msg.clone())
    } else {
        // A response to a request we no longer care about (e.g. after a
        // timeout). Treat as a skippable notification.
        Incoming::Notification {
            method: String::new(),
        }
    }
}

/// Sensible default response for a server→client request we don't really
/// implement. Servers block on some of these (rust-analyzer asks
/// `workspace/configuration` during startup), so never-ignore is the safe
/// policy: `workspace/configuration` → `[]` (no client overrides, server
/// defaults apply); everything else → `null`.
pub fn default_server_response(method: &str) -> Value {
    match method {
        "workspace/configuration" => json!([]),
        _ => Value::Null,
    }
}

/// Whether a JSON-RPC error object is a transient server-side
/// invalidation the client may safely retry: -32800 RequestCancelled /
/// -32801 ContentModified (spec-defined codes servers use when a request
/// is invalidated by document/VFS changes rather than rejected).
pub fn is_transient_error(err: &Value) -> bool {
    matches!(
        err.get("code").and_then(|c| c.as_i64()),
        Some(-32800) | Some(-32801)
    )
}

// ---------------------------------------------------------------------------
// URI helpers (LSP uses file:// URIs; we work in paths)
// ---------------------------------------------------------------------------

/// Path → `file://` URI，url crate（WHATWG）形态。2026-09-05 根修背景：
/// 真实语言服务器（rust-analyzer 等，url-crate 系）会把收到的 URI 按
/// WHATWG 规范化——Windows 盘符小写（`C:` → `c:`）——推送诊断时回显的
/// 是**规范化后**的形态，与任何字面精确匹配的记录/查询永远 miss（实机
/// 闭环验证暴露；fake 服务器逐字回显掩盖了它）。
///
/// 注意：发射端**不保证**与服务器回显字面一致——本仓 toolchain 的
/// rust-url `from_file_path` 产出大写盘符，而新版服务器回显小写。闭环
/// 正确性由 [`uri_key`] 在记录/查询双端归一兜住（见其 doc）。选 url
/// crate 做发射是为了 percent-encoding 形态与主流服务器同源（手写编码
/// 器对 sub-delim 的编码集更激进，异形路径下会放大形态差）。非绝对路径
/// （from_file_path 失败）退回手写编码器（RFC 3986 percent-encoding，
/// 非 ASCII 与保留字节编码为 UTF-8 转义）。
pub fn path_to_uri(p: &std::path::Path) -> String {
    if let Ok(u) = url::Url::from_file_path(p) {
        return String::from(u);
    }
    let s = p.to_string_lossy().replace('\\', "/");
    // "file://" + authority("") + path-with-leading-slash: POSIX roots
    // already carry the slash, Windows drive paths need one added.
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.as_bytes() {
        match b {
            // ':' stays literal — legal pchar in RFC 3986 paths, and
            // Windows drive colons ("file:///C:/…") are conventionally
            // unencoded (VS Code / rust-analyzer both emit them raw).
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// URI → **规范化键**。服务器推送的诊断按此键记录、本地查询按此键查找
/// ——字符串形态不同但语义相同的 URI 落到同一个键上。
///
/// 归一范围（对齐真实服务器的实际行为，2026-09-05 实机闭环验证）：
/// **Windows 盘符小写**（WHATWG file-URL 规则，rust-analyzer 的 url
/// crate 把 `C:` 规范成 `c:` 回显）——纯字符串级实现，不依赖宿主平台
/// 路径语义，Windows/POSIX 行为一致。先过 `Url::parse` + 重序列化
/// （吃掉协议级畸形；rust-url 不做 unreserved 百分号解码，故不承诺编
/// 码归一——实测 rust-analyzer/gopls 回显的编码与本端发射一致）。整体
/// 解析失败原样返回，绝不 panic。
pub fn uri_key(uri: &str) -> String {
    let s = url::Url::parse(uri)
        .map(String::from)
        .unwrap_or_else(|_| uri.to_string());
    // file:///X:… → file:///x:…（X 为单个 ASCII 字母；WHATWG 盘符规则）。
    let b = s.as_bytes();
    if s.starts_with("file:///") && b.len() >= 10 && b[8].is_ascii_alphabetic() && b[9] == b':' {
        let mut out = String::with_capacity(s.len());
        out.push_str(&s[..8]);
        out.push((b[8].to_ascii_lowercase()) as char);
        out.push_str(&s[9..]);
        return out;
    }
    s
}

/// file::// URI → path string (percent-decoded). Tolerant of unencoded
/// characters: bytes that are not `%XX` escapes pass through as-is, so URIs
/// that servers echo back in raw form still decode. （不走 url crate：本端
/// 解码显示不是 2026-09-05 的 miss 根因，且此实现对未编码原始形态更宽容、
/// 保留 `/` 分隔符的显示一致性——有既有测试钉住。）
pub fn uri_to_path(uri: &str) -> String {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    // Percent-decode first (a drive colon may arrive as %3A), reading
    // b[i+1]/b[i+2] with i+2 < len.
    let b = rest.as_bytes();
    let hex = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    let decoded = String::from_utf8_lossy(&out).to_string();
    // file:///C:/x → "/C:/x" — the leading '/' before a Windows drive
    // letter is an artifact of the URI form, drop it. POSIX paths keep it.
    let db = decoded.as_bytes();
    if db.len() >= 3 && db[0] == b'/' && db[1].is_ascii_alphabetic() && db[2] == b':' {
        decoded[1..].to_string()
    } else {
        decoded
    }
}

// ---------------------------------------------------------------------------
// Response parsing (the four read-only ops)
// ---------------------------------------------------------------------------

/// A resolved location (path + 0-based LSP position).
#[derive(Debug, Clone, PartialEq)]
pub struct Loc {
    pub path: String,
    pub line: u32,
    pub character: u32,
}

/// Parse a definition/references/implementation result. The spec allows
/// `Location | Location[] | LocationLink[] | null`, and some servers wrap
/// single objects in arrays inconsistently — handle all shapes defensively.
pub fn parse_locations(result: &Value) -> Vec<Loc> {
    let one = |v: &Value| -> Option<Loc> {
        // LocationLink uses targetUri/targetRange; Location uses uri/range.
        let uri = v
            .get("targetUri")
            .or_else(|| v.get("uri"))
            .and_then(|u| u.as_str())?;
        let range = v.get("targetRange").or_else(|| v.get("range"))?;
        let start = range.get("start")?;
        let line = start.get("line").and_then(|l| l.as_u64())? as u32;
        let character = start.get("character").and_then(|c| c.as_u64())? as u32;
        Some(Loc {
            path: uri_to_path(uri),
            line,
            character,
        })
    };
    match result {
        Value::Null => vec![],
        Value::Array(items) => items.iter().filter_map(one).collect(),
        obj => one(obj).into_iter().collect(),
    }
}

/// Parse a hover result to plain text. Shapes: `null`,
/// `{contents: MarkupContent|MarkedString|MarkedString[]}`. MarkedString is
/// `string | {language, value}`; MarkupContent is `{kind, value}`.
pub fn parse_hover(result: &Value) -> String {
    let Some(contents) = result.get("contents") else {
        return String::new();
    };
    let one = |v: &Value| -> String {
        match v {
            Value::String(s) => s.clone(),
            obj => {
                // {language, value} (MarkedString object) or {kind, value}
                // (MarkupContent). The fenced form signals code to readers.
                if let (Some(_lang), Some(value)) = (
                    obj.get("language").and_then(|l| l.as_str()),
                    obj.get("value").and_then(|s| s.as_str()),
                ) {
                    format!("```{}\n{}\n```", _lang, value)
                } else if let Some(value) = obj.get("value").and_then(|s| s.as_str()) {
                    value.to_string()
                } else {
                    String::new()
                }
            }
        }
    };
    match contents {
        Value::Array(items) => items
            .iter()
            .map(one)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        v => one(v),
    }
}

// ---------------------------------------------------------------------------
// WorkspaceEdit / CodeAction 解析 + TextEdit 应用（C7 写型 LSP）
// ---------------------------------------------------------------------------

/// One LSP `TextEdit`: replace `range_start..range_end` (0-based, UTF-16
/// columns) with `new_text`. An insertion has start == end; a deletion has
/// an empty `new_text`.
#[derive(Debug, Clone, PartialEq)]
pub struct TextEdit {
    pub range_start: (u32, u32),
    pub range_end: (u32, u32),
    pub new_text: String,
}

/// All edits a rename applies to ONE file (uri already decoded to a path).
#[derive(Debug, Clone, PartialEq)]
pub struct FileEdits {
    pub path: String,
    pub edits: Vec<TextEdit>,
}

/// Parse a `WorkspaceEdit` result (textDocument/rename, codeAction.edit).
/// Handles both shapes servers emit: `{"changes": {uri: [TextEdit]}}` and
/// `{"documentChanges": [{textDocument: {uri}, edits: [...]}]}` (spec
/// 3.16 prefers the latter; rust-analyzer/gopls use both in the wild).
/// Tolerant: `null` → empty; malformed entries skipped rather than fatal.
pub fn parse_workspace_edit(result: &Value) -> Vec<FileEdits> {
    if result.is_null() {
        return vec![];
    }
    let mut out: Vec<FileEdits> = Vec::new();
    let mut push_edits = |path: String, items: &Value| {
        let Some(arr) = items.as_array() else {
            return;
        };
        let edits: Vec<TextEdit> = arr
            .iter()
            .filter_map(|e| {
                let range = e.get("range")?;
                let new_text = e.get("newText")?.as_str()?.to_string();
                let pos = |p: Option<&Value>| -> Option<(u32, u32)> {
                    let p = p?;
                    Some((
                        p.get("line")?.as_u64()? as u32,
                        p.get("character")?.as_u64()? as u32,
                    ))
                };
                Some(TextEdit {
                    range_start: pos(range.get("start"))?,
                    range_end: pos(range.get("end"))?,
                    new_text,
                })
            })
            .collect();
        if !edits.is_empty() {
            out.push(FileEdits { path, edits });
        }
    };
    if let Some(changes) = result.get("changes").and_then(|c| c.as_object()) {
        for (uri, items) in changes {
            push_edits(uri_to_path(uri), items);
        }
    }
    if let Some(docs) = result.get("documentChanges").and_then(|c| c.as_array()) {
        for d in docs {
            let Some(uri) = d
                .get("textDocument")
                .and_then(|t| t.get("uri"))
                .and_then(|u| u.as_str())
            else {
                continue;
            };
            push_edits(uri_to_path(uri), d.get("edits").unwrap_or(&Value::Null));
        }
    }
    out
}

/// One listed code action, reduced to what the model can act on.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeActionInfo {
    pub title: String,
    /// LSP CodeActionKind, e.g. `quickfix` / `refactor`. Missing = server
    /// didn't classify it.
    pub kind: Option<String>,
    /// Whether the action carries a WorkspaceEdit (vs only a `command`).
    pub has_edit: bool,
    pub is_preferred: bool,
}

/// Parse a `textDocument/codeAction` result: `CodeAction[] | null`. Legacy
/// `Command[]` responses (no title-bearing objects with kind/edit) degrade
/// to entries with just the command title. Malformed entries are skipped.
pub fn parse_code_actions(result: &Value) -> Vec<CodeActionInfo> {
    let Some(arr) = result.as_array() else {
        return vec![];
    };
    arr.iter()
        .filter_map(|a| {
            let title = a.get("title")?.as_str()?.to_string();
            Some(CodeActionInfo {
                kind: a.get("kind").and_then(|k| k.as_str()).map(String::from),
                has_edit: a.get("edit").map(|e| !e.is_null()).unwrap_or(false),
                is_preferred: a
                    .get("isPreferred")
                    .and_then(|p| p.as_bool())
                    .unwrap_or(false),
                title,
            })
        })
        .collect()
}

/// LSP position → UTF-8 byte offset in `content`.
///
/// LSP positions are (line, character) with character counted in **UTF-16
/// code units** — a Rust `str` is UTF-8, so the column must be converted
/// through the line's chars: BMP chars (incl. CJK) are 1 unit, astral
/// chars (emoji etc.) are 2. Errors are honest: line out of range, or
/// character beyond the line's UTF-16 length both return `Err` (callers
/// must not silently clamp — a wrong offset corrupts the edit).
pub fn position_to_byte(content: &str, line: u32, character: u32) -> Result<usize, String> {
    // Fast-forward to the start byte of the target line.
    let mut byte = 0usize;
    let mut seen = 0u32;
    while seen < line {
        match content[byte..].find('\n') {
            Some(nl) => {
                byte += nl + 1;
                seen += 1;
            }
            None => return Err(format!("line {line} beyond document ({seen} lines)")),
        }
    }
    // Walk the target line counting UTF-16 units.
    let mut units: u32 = 0;
    for c in content[byte..].chars() {
        if c == '\n' {
            break;
        }
        if units >= character {
            return Ok(byte);
        }
        units += c.len_utf16() as u32;
        if units > character {
            return Err(format!(
                "character {character} splits a surrogate pair on line {line}"
            ));
        }
        byte += c.len_utf8();
    }
    if units == character {
        // End-of-line / end-of-file insertion point (byte is right before
        // the line's '\n' or at EOF) — legal.
        Ok(byte)
    } else {
        Err(format!(
            "character {character} beyond line {line} length ({units} UTF-16 units)"
        ))
    }
}

/// Apply LSP `TextEdit`s to file content. Edits are sorted by position and
/// applied back-to-front so earlier byte offsets stay valid; overlapping
/// ranges and out-of-bounds positions are honest errors (a rename that
/// would produce corrupted text must fail loudly, not write garbage).
pub fn apply_text_edits(content: &str, edits: &[TextEdit]) -> Result<String, String> {
    if edits.is_empty() {
        return Ok(content.to_string());
    }
    let mut ordered: Vec<&TextEdit> = edits.iter().collect();
    ordered.sort_by(|a, b| {
        (a.range_start.0, a.range_start.1).cmp(&(b.range_start.0, b.range_start.1))
    });
    // Overlap check on adjacent pairs (after sorting, any overlap is between
    // neighbours; nested ranges from broken servers must not silently merge).
    for w in ordered.windows(2) {
        let (a, b) = (w[0], w[1]);
        let a_end_after_b_start =
            (a.range_end.0, a.range_end.1) > (b.range_start.0, b.range_start.1);
        if a_end_after_b_start {
            return Err(format!(
                "overlapping edits: [{:?}-{:?}] and [{:?}-{:?}]",
                a.range_start, a.range_end, b.range_start, b.range_end
            ));
        }
    }
    // Pre-resolve byte offsets BEFORE mutating (all offsets refer to the
    // original content).
    let mut resolved: Vec<(usize, usize, &str)> = Vec::with_capacity(ordered.len());
    for e in &ordered {
        let start = position_to_byte(content, e.range_start.0, e.range_start.1)?;
        let end = position_to_byte(content, e.range_end.0, e.range_end.1)?;
        if end < start {
            return Err(format!(
                "edit range end before start: [{:?}-{:?}]",
                e.range_start, e.range_end
            ));
        }
        resolved.push((start, end, e.new_text.as_str()));
    }
    // Apply back-to-front.
    let mut out = content.to_string();
    for (start, end, text) in resolved.into_iter().rev() {
        out.replace_range(start..end, text);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Diagnostics (C2：textDocument/publishDiagnostics 消费)
// ---------------------------------------------------------------------------

/// One published diagnostic, reduced to the fields the model needs.
/// Positions are 0-based LSP `(line, character)` pairs.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub range_start: (u32, u32),
    pub range_end: (u32, u32),
    /// 1=Error 2=Warning 3=Information 4=Hint（规范缺省 = 1）。
    pub severity: u8,
    pub source: Option<String>,
    pub message: String,
}

impl Diagnostic {
    /// C7：还原成 LSP Diagnostic JSON——`textDocument/codeAction` 的
    /// `context.diagnostics` 参数要求把当前诊断原样回传（服务器按它决定
    /// 给哪些 quickfix）。
    pub fn to_json(&self) -> Value {
        json!({
            "range": {
                "start": {"line": self.range_start.0, "character": self.range_start.1},
                "end": {"line": self.range_end.0, "character": self.range_end.1},
            },
            "severity": self.severity,
            "source": self.source,
            "message": self.message,
        })
    }
}

/// Parse `textDocument/publishDiagnostics` params into `(uri, diagnostics)`.
/// Tolerant: `None` when `uri`/`diagnostics` are missing or misshapen;
/// individual malformed entries (no range / no message) are skipped rather
/// than fatal. The optional spec `version` field is ignored — every push
/// carries the uri's full list, so the latest push is always current.
pub fn parse_publish_diagnostics(params: &Value) -> Option<(String, Vec<Diagnostic>)> {
    let uri = params.get("uri")?.as_str()?.to_string();
    let items = params.get("diagnostics")?.as_array()?;
    let pos = |p: Option<&Value>| -> Option<(u32, u32)> {
        let p = p?;
        Some((
            p.get("line")?.as_u64()? as u32,
            p.get("character")?.as_u64()? as u32,
        ))
    };
    let diags = items
        .iter()
        .filter_map(|d| {
            let range = d.get("range")?;
            let message = d.get("message")?.as_str()?.to_string();
            // 1..=4 之外（缺省/越界）回落规范默认 1（Error）。
            let severity = match d.get("severity").and_then(|s| s.as_u64()) {
                Some(s @ 1..=4) => s as u8,
                _ => 1,
            };
            Some(Diagnostic {
                range_start: pos(range.get("start"))?,
                range_end: pos(range.get("end"))?,
                severity,
                source: d.get("source").and_then(|s| s.as_str()).map(String::from),
                message,
            })
        })
        .collect();
    Some((uri, diags))
}

#[cfg(test)]
mod tests;
