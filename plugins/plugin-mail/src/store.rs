//! MailStore——maildir 按地址分库 + 地址注册表（单一真相源在注册表文件）。
//!
//! 布局（结构性隔离锚点，root = `<home>/mail/`，宿主 init 注入 home 根）：
//!
//! ```text
//! <root>/addresses.json            地址注册表（默认主库 = 首个注册地址）
//! <root>/index.jsonl               追加式去重索引（message_id → 库/文件）
//! <root>/<地址>/{new,cur,tmp}/     每地址一个 maildir
//! ```
//!
//! maildir 变体注记（诚实边界）：标准 maildir 的已读旗标 = 文件名 `:2,S`
//! 后缀——**Windows 文件名禁 `:`**，本实现用目录归属承载旗标（`new/` =
//! 未读，`cur/` = 已读，不带后缀）。个别严格 maildir 读取器把 `cur/` 文件
//! 默认视为已读——语义兼容；`.eml` 自描述性不受影响（POP3 RETR / 任何
//! 邮件客户端直开）。
//!
//! 投递 = tmp/ 写入 + 原子 rename 进 new/（崩溃安全：rename 同卷原子）。
//! 文件名 = `{millis:015}.{seq:04}.{nonce}.eml`（时间戳 + 原子序号 + 随机
//! nonce——UIDL 唯一性三保险，时钟回拨/同毫秒并发都不撞）。
//!
//! 地址注销只停收不删库：注册表摘除条目，库目录与历史信保留（重新注册
//! 自动归位）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 地址合法性（路径段安全 + RFC 形态粗检）：ASCII 字母数字 + `@._+-`，
/// 拒 `..`、路径分隔符、`:`（Windows 文件名禁）、空白与控制字符；
/// 形态须 `local@domain` 两段非空；总长 ≤254（RFC 5321 limit）。
pub fn validate_address(a: &str) -> Result<(), String> {
    if a.is_empty() {
        return Err("地址为空".to_string());
    }
    if a.len() > 254 {
        return Err("地址超长（>254）".to_string());
    }
    if a.contains("..") || a.contains('/') || a.contains('\\') || a.contains(':') {
        return Err(format!("地址含非法字符（路径分隔符/../:）：{a}"));
    }
    if !a
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '-' | '_' | '+'))
    {
        return Err(format!("地址含非法字符（限 ASCII 字母数字 @._+-）：{a}"));
    }
    let Some((local, domain)) = a.split_once('@') else {
        return Err(format!("地址缺少 @ 分段：{a}"));
    };
    if local.is_empty() || domain.is_empty() {
        return Err(format!("地址 local/domain 段为空：{a}"));
    }
    Ok(())
}

/// 注册表条目。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AddrEntry {
    pub address: String,
    /// 注册时间（ms epoch）。
    pub registered_at_ms: u64,
}

/// 列信查询参数（WSAPI/契约 `mail_list` query JSON 的 serde 形态）。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct MailQuery {
    /// 目标库地址；缺省 = 默认主库（首个注册地址）。
    #[serde(default)]
    pub mailbox: Option<String>,
    #[serde(default)]
    pub unread_only: bool,
    /// 缺省 50。
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
}

/// 列表项（列表面轻量元数据）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct MailItem {
    pub id: String,
    pub mailbox: String,
    pub seen: bool,
    pub from: String,
    pub subject: String,
    pub date: String,
    pub message_id: String,
    pub size: u64,
}

/// 文件名序号（进程内原子递增，UIDL 唯一性第二保险）。
static SEQ: AtomicU64 = AtomicU64::new(0);

pub struct MailStore {
    root: PathBuf,
}

impl MailStore {
    /// root = `<home>/mail/`（宿主注入 home 根后由调用方拼好）。
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// mail 根目录（POP3 快照扫描等内部消费）。
    pub fn root(&self) -> &Path {
        &self.root
    }

    // ---- 注册表 ----

    fn registry_path(&self) -> PathBuf {
        self.root.join("addresses.json")
    }

    /// 读注册表（文件缺席 = 空注册表；坏 JSON = 诚实 Err 不静默清空）。
    pub fn load_registry(&self) -> Result<Vec<AddrEntry>, String> {
        let path = self.registry_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(&path)
            .map_err(|e| format!("注册表读取失败（{}）：{e}", path.display()))?;
        serde_json::from_str::<Vec<AddrEntry>>(&raw)
            .map_err(|e| format!("注册表解析失败（{}）：{e}", path.display()))
    }

    /// 写注册表（原子：`.tmp` + rename 同卷）。
    fn save_registry(&self, addrs: &[AddrEntry]) -> Result<(), String> {
        fs::create_dir_all(&self.root).map_err(|e| format!("mail 根目录创建失败：{e}"))?;
        let path = self.registry_path();
        let tmp = self.root.join("addresses.json.tmp");
        let raw =
            serde_json::to_string_pretty(addrs).map_err(|e| format!("注册表序列化失败：{e}"))?;
        fs::write(&tmp, raw).map_err(|e| format!("注册表临时文件写入失败：{e}"))?;
        fs::rename(&tmp, &path).map_err(|e| format!("注册表原子落盘失败：{e}"))
    }

    /// 注册地址（幂等；首个注册 = 默认主库；域须 ∈ local_domains）。
    pub fn addr_add(&self, address: &str, local_domains: &[String]) -> Result<(), String> {
        validate_address(address)?;
        let domain = address.split('@').nth(1).unwrap_or("").to_lowercase();
        if !local_domains.iter().any(|d| d.to_lowercase() == domain) {
            return Err(format!(
                "域 {domain} 不在 local_domains（{:?}）——注册被拒",
                local_domains
            ));
        }
        let mut addrs = self.load_registry()?;
        if addrs.iter().any(|a| a.address == address) {
            return Ok(()); // 幂等
        }
        addrs.push(AddrEntry {
            address: address.to_string(),
            registered_at_ms: now_ms(),
        });
        self.save_registry(&addrs)
    }

    /// 注销地址（只停收不删库：摘注册表条目，库目录/历史信保留）。
    pub fn addr_remove(&self, address: &str) -> Result<(), String> {
        validate_address(address)?;
        let mut addrs = self.load_registry()?;
        let before = addrs.len();
        addrs.retain(|a| a.address != address);
        if addrs.len() == before {
            return Err(format!("地址未注册：{address}"));
        }
        self.save_registry(&addrs)
    }

    /// 默认主库 = 首个注册地址（注册表为空 = 诚实 Err）。
    pub fn default_mailbox(&self) -> Result<String, String> {
        self.load_registry()?
            .first()
            .map(|a| a.address.clone())
            .ok_or_else(|| "无注册地址（先 mail_addr_add 注册）".to_string())
    }

    /// 全部注册地址。
    pub fn list_addresses(&self) -> Result<Vec<AddrEntry>, String> {
        self.load_registry()
    }

    // ---- 库目录 ----

    fn mailbox_dir(&self, mailbox: &str) -> PathBuf {
        self.root.join(mailbox)
    }

    /// 建 `new/cur/tmp` 三目录（幂等）。地址合法性在注册/收信判定已把关，
    /// 这里防御性再验一次（路径段拼接点是纵深防御位）。
    fn ensure_mailbox(&self, mailbox: &str) -> Result<PathBuf, String> {
        validate_address(mailbox)?;
        let dir = self.mailbox_dir(mailbox);
        for sub in ["new", "cur", "tmp"] {
            fs::create_dir_all(dir.join(sub))
                .map_err(|e| format!("库目录创建失败（{mailbox}/{sub}）：{e}"))?;
        }
        Ok(dir)
    }

    // ---- 去重索引 ----

    fn index_path(&self) -> PathBuf {
        self.root.join("index.jsonl")
    }

    fn index_append(&self, message_id: &str, mailbox: &str, file: &str) {
        // 追加失败不阻塞投递（去重索引是尽力面——缺行只影响 record 去重）。
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.index_path())
        {
            use std::io::Write;
            let line = serde_json::json!({
                "message_id": normalize_message_id(message_id),
                "mailbox": mailbox,
                "file": file
            });
            let _ = writeln!(f, "{line}");
        }
    }

    fn index_contains(&self, message_id: &str) -> bool {
        let mid = normalize_message_id(message_id);
        if mid.is_empty() {
            return false;
        }
        let Ok(raw) = fs::read_to_string(self.index_path()) else {
            return false;
        };
        raw.lines().any(|l| {
            serde_json::from_str::<serde_json::Value>(l)
                .ok()
                .and_then(|v| {
                    v.get("message_id")
                        .and_then(|m| m.as_str())
                        .map(|m| m == mid)
                })
                .unwrap_or(false)
        })
    }

    // ---- 投递 ----

    /// 投递一封信进指定库（tmp 写 + 原子 rename 进 new/）。返回文件 id。
    pub fn deliver(&self, mailbox: &str, eml: &str) -> Result<String, String> {
        let dir = self.ensure_mailbox(mailbox)?;
        let millis = now_ms();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let nonce = rand_nonce();
        let name = format!("{millis:015}.{seq:04}.{nonce}.eml");
        let tmp_path = dir.join("tmp").join(&name);
        fs::write(&tmp_path, eml).map_err(|e| format!("投递临时文件写入失败：{e}"))?;
        let final_path = dir.join("new").join(&name);
        fs::rename(&tmp_path, &final_path)
            .map_err(|e| format!("投递原子落盘失败（tmp→new）：{e}"))?;
        let id = name.trim_end_matches(".eml").to_string();
        let message_id = extract_header(eml, "Message-ID").unwrap_or_default();
        self.index_append(&message_id, mailbox, &id);
        Ok(id)
    }

    // ---- 查询/读取 ----

    /// 列信（单库；mailbox 须已注册）。按文件名降序（新信在前）。
    pub fn list(&self, q: &MailQuery) -> Result<Vec<MailItem>, String> {
        let mailbox = match &q.mailbox {
            Some(m) if !m.is_empty() => m.clone(),
            _ => self.default_mailbox()?,
        };
        validate_address(&mailbox)?;
        let registered = self.load_registry()?;
        if !registered.iter().any(|a| a.address == mailbox) {
            return Err(format!("地址未注册（只读已注册库）：{mailbox}"));
        }
        let dir = self.mailbox_dir(&mailbox);
        let mut items: Vec<MailItem> = Vec::new();
        for (sub, seen) in [("new", false), ("cur", true)] {
            let Ok(entries) = fs::read_dir(dir.join(sub)) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("eml") {
                    continue;
                }
                let name = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                let eml = fs::read_to_string(&path).unwrap_or_default();
                items.push(MailItem {
                    id: name,
                    mailbox: mailbox.clone(),
                    seen,
                    from: extract_header(&eml, "From").unwrap_or_default(),
                    subject: extract_header(&eml, "Subject").unwrap_or_default(),
                    date: extract_header(&eml, "Date").unwrap_or_default(),
                    message_id: extract_header(&eml, "Message-ID").unwrap_or_default(),
                    size,
                });
            }
        }
        // 新信在前：文件名前缀 = 时间戳定宽，字典序降序 = 时间降序。
        items.sort_by(|a, b| b.id.cmp(&a.id));
        let offset = q.offset.unwrap_or(0) as usize;
        let limit = q.limit.unwrap_or(50).max(1) as usize;
        let filtered: Vec<MailItem> = items
            .into_iter()
            .filter(|i| !q.unread_only || !i.seen)
            .skip(offset)
            .take(limit)
            .collect();
        Ok(filtered)
    }

    /// 按 id 全库定位（注册地址范围内扫描）→ (库, 路径, 已读旗标)。
    fn find_message(&self, id: &str) -> Result<Option<(String, PathBuf, bool)>, String> {
        validate_path_stem(id)?;
        for addr in self.load_registry()? {
            let dir = self.mailbox_dir(&addr.address);
            for (sub, seen) in [("new", false), ("cur", true)] {
                let path = dir.join(sub).join(format!("{id}.eml"));
                if path.is_file() {
                    return Ok(Some((addr.address.clone(), path, seen)));
                }
            }
        }
        Ok(None)
    }

    /// 读信全文 → (元数据, 正文)。正文 = 头块之后全部（保留原始字节，
    /// POP3 RETR 同源）。
    pub fn read(&self, id: &str) -> Result<(MailItem, String), String> {
        let Some((mailbox, path, seen)) = self.find_message(id)? else {
            return Err(format!("邮件不存在：{id}"));
        };
        let eml = fs::read_to_string(&path).map_err(|e| format!("邮件读取失败（{id}）：{e}"))?;
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let (headers, body) = split_headers_body(&eml);
        let item = MailItem {
            id: id.to_string(),
            mailbox,
            seen,
            from: extract_header(&headers, "From").unwrap_or_default(),
            subject: extract_header(&headers, "Subject").unwrap_or_default(),
            date: extract_header(&headers, "Date").unwrap_or_default(),
            message_id: extract_header(&headers, "Message-ID").unwrap_or_default(),
            size,
        };
        Ok((item, body))
    }

    /// 标记已读（new/ → cur/ 平移；已在 cur/ = 幂等 ok）。
    pub fn mark_read(&self, id: &str) -> Result<(), String> {
        let Some((_mailbox, path, seen)) = self.find_message(id)? else {
            return Err(format!("邮件不存在：{id}"));
        };
        if seen {
            return Ok(());
        }
        let cur = path
            .parent()
            .unwrap_or(Path::new("."))
            .parent()
            .unwrap_or(Path::new("."))
            .join("cur");
        fs::rename(&path, cur.join(path.file_name().ok_or("文件名不可得")?))
            .map_err(|e| format!("已读旗标平移失败（new→cur）：{e}"))
    }

    /// 删信（带小重试——Windows AV/索引器瞬时句柄是常态不是异常）。
    pub fn delete(&self, id: &str) -> Result<(), String> {
        let Some((_mailbox, path, _seen)) = self.find_message(id)? else {
            return Err(format!("邮件不存在：{id}"));
        };
        let mut last = None;
        for _ in 0..3 {
            match fs::remove_file(&path) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
        Err(format!("删除失败（{}）：{:?}", path.display(), last))
    }

    /// 库统计（各注册地址：总数/未读/字节）。
    pub fn stats(&self) -> Result<serde_json::Value, String> {
        let mut per = Vec::new();
        for addr in self.load_registry()? {
            let dir = self.mailbox_dir(&addr.address);
            let mut total = 0u64;
            let mut unread = 0u64;
            let mut bytes = 0u64;
            for (sub, seen) in [("new", false), ("cur", true)] {
                let Ok(entries) = fs::read_dir(dir.join(sub)) else {
                    continue;
                };
                for entry in entries.flatten() {
                    if entry.path().extension().and_then(|e| e.to_str()) != Some("eml") {
                        continue;
                    }
                    total += 1;
                    if !seen {
                        unread += 1;
                    }
                    bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
            per.push(serde_json::json!({
                "mailbox": addr.address,
                "total": total,
                "unread": unread,
                "bytes": bytes,
            }));
        }
        Ok(serde_json::json!({"ok": true, "mailboxes": per}))
    }

    /// 收信记录登记（宿主管外部来源元数据统一纳管；message_id 去重）。
    /// meta 字段：`mailbox`(可选,缺=默认主库) / `from` / `to` / `subject` /
    /// `message_id` / `date`(可选,缺=now) / `body`(可选,缺=空)。
    pub fn record(&self, meta: &serde_json::Value) -> Result<serde_json::Value, String> {
        let message_id = normalize_message_id(
            meta.get("message_id")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        )
        .to_string();
        if !message_id.is_empty() && self.index_contains(&message_id) {
            return Ok(serde_json::json!({"ok": true, "deduped": true}));
        }
        let mailbox = match meta.get("mailbox").and_then(|v| v.as_str()) {
            Some(m) if !m.is_empty() => m.to_string(),
            _ => self.default_mailbox()?,
        };
        let from = meta.get("from").and_then(|v| v.as_str()).unwrap_or("");
        let to = meta.get("to").and_then(|v| v.as_str()).unwrap_or("");
        let subject = meta.get("subject").and_then(|v| v.as_str()).unwrap_or("");
        let date = meta.get("date").and_then(|v| v.as_str()).unwrap_or("");
        let body = meta.get("body").and_then(|v| v.as_str()).unwrap_or("");
        let date_line = if date.is_empty() {
            chrono::Utc::now().to_rfc2822()
        } else {
            date.to_string()
        };
        let mid_line = if message_id.is_empty() {
            format!("<{}.{}@mailbot.local>", now_ms(), rand_nonce())
        } else {
            format!("<{message_id}>")
        };
        let eml = format!(
            "From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\nDate: {date_line}\r\n\
             Message-ID: {mid_line}\r\n\r\n{body}"
        );
        let id = self.deliver(&mailbox, &eml)?;
        Ok(serde_json::json!({"ok": true, "id": id, "mailbox": mailbox, "deduped": false}))
    }
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// message_id 索引键归一（键空间统一裸形态）：trim + 剥一层尖括号。
/// 调用方可能给 RFC 头形态 `<x@y>`（record meta / 头值提取）或裸 id
/// ——写入与查询双侧同源归一，形态分歧永不造成去重失效。
fn normalize_message_id(mid: &str) -> &str {
    let t = mid.trim();
    t.strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(t)
}

/// 路径干理（防注入纵深）：id 只允许文件名字符集（字母数字 + `._-`）。
fn validate_path_stem(id: &str) -> Result<(), String> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        || id.contains("..")
    {
        return Err(format!("非法邮件 id（限字母数字 ._- 且无 ..）：{id}"));
    }
    Ok(())
}

/// 轻量头提取（RFC 5322 头块内首个同名行；大小写不敏感；含折行续行）。
/// 输入可为完整 .eml 或纯头块。
pub fn extract_header(eml: &str, name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    let mut result: Option<String> = None;
    let mut in_headers = true;
    let mut lines = eml.lines().peekable();
    while let Some(line) = lines.next() {
        if in_headers && line.is_empty() {
            in_headers = false;
            continue;
        }
        if !in_headers {
            break;
        }
        if let Some((field, value)) = nemesis_mail_protocol::split_header_field(line)
            && field == lower
        {
            let mut v = value;
            // 折行续行（RFC 5322 WSP 开头行并入前值）。
            while let Some(next) = lines.peek() {
                if next.starts_with(' ') || next.starts_with('\t') {
                    v.push(' ');
                    v.push_str(next.trim());
                    let _ = lines.next();
                } else {
                    break;
                }
            }
            result = Some(v);
            break;
        }
    }
    result
}

/// 头块 / 正文切分（首个空行为界）。
fn split_headers_body(eml: &str) -> (String, String) {
    match eml.find("\r\n\r\n") {
        Some(i) => (eml[..i].to_string(), eml[i + 4..].to_string()),
        None => match eml.find("\n\n") {
            Some(i) => (eml[..i].to_string(), eml[i + 2..].to_string()),
            None => (eml.to_string(), String::new()),
        },
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 随机 nonce（8 hex；getrandom 不引依赖——OS 时间 + 进程计数足够 UIDL
/// 唯一性第三保险，nonce 只兜极端同毫秒同序号场景）。
fn rand_nonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{nanos:08x}")
}
