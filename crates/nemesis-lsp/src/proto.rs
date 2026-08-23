//! LSP wire protocol: Content-Length framing, JSON-RPC envelopes, and
//! response parsing for the four read-only operations (L1 / U19).
//!
//! Everything here is pure — no process, no async — so the framing and
//! parsing rules are unit-testable in isolation from any language server.

use serde_json::{json, Value};

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
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("content-length") {
                    content_length = Some(
                        value
                            .trim()
                            .parse::<usize>()
                            .map_err(|e| format!("bad Content-Length {value:?}: {e}"))?,
                    );
                }
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
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
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
    matches!(err.get("code").and_then(|c| c.as_i64()), Some(-32800) | Some(-32801))
}

// ---------------------------------------------------------------------------
// URI helpers (LSP uses file:// URIs; we work in paths)
// ---------------------------------------------------------------------------

/// Path → `file:///` URI with RFC 3986 percent-encoding. Non-ASCII (e.g.
/// Chinese in paths) and reserved bytes are encoded as UTF-8 percent
/// escapes; `/` separators stay literal.
pub fn path_to_uri(p: &std::path::Path) -> String {
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

/// file:// URI → path string (percent-decoded). Tolerant of unencoded
/// characters: bytes that are not `%XX` escapes pass through as-is, so URIs
/// that servers echo back in raw form still decode.
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
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
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

#[cfg(test)]
mod tests;
