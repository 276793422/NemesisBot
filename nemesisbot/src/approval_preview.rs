//! L07（2026-11-01）：审批卡 diff 预览生成——挂起点（拦截处）把「将要发生
//! 的变化」算成 unified diff / 命令现场，随审批卡下发。设计文档：
//! `docs/PLAN/2026-11-01_L07审批卡diff预览与孤儿卡清理_设计文档.md`。
//!
//! 分派按 **args 字段形态**（content / old_text+new_text / edits[] /
//! command）而非工具名——挂起点收到的 operation 是 op_type 字符串，
//! write_file/edit_file/append_file/multiedit 四工具全部折叠为 `file_write`
//! （`tool_to_operation`），工具名在那里不可得；args 形态是工具 schema 的
//! 稳定面。
//!
//! preview 是**快照非授权面**：批准的仍是操作本身；预览必须忠实于将执行
//! 的内容，不做净化（净化的预览会造成批准内容 ≠ 执行内容的假象）。

/// preview 文本字节上限（超出截断 + 尾注记；宁可截不可卡 UI）。
pub const MAX_PREVIEW_BYTES: usize = 64 * 1024;
/// 现盘文件参与 diff 的大小上限（超过则旧侧不读入，注记形态降级）。
const MAX_DIFF_SOURCE_BYTES: u64 = 1024 * 1024;
/// 二进制探测窗口（前 8KB 含 NUL = 二进制，Unix 惯例）。
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// 生成审批卡预览。返回 `(文本, 格式)`；`None` = 不产预览（args 缺失 /
/// 非白名单形态）——卡片无预览区，保持现状。
///
/// `workspace`：工具层 `resolve_workspace_path` 的 join 语义真相源——args
/// 的 path 是**相对 workspace** 解析的（工具执行面），预览侧读现盘必须同
/// 坐标系，否则相对路径形态的预览按进程 cwd 读（cwd ≠ workspace 时读错
/// 文件 / 误报「全量新增」，违反预览忠实原则——BUG-20261106-033）。绝对
/// 路径不受影响；None（未配置 workspace 的环境）保持旧行为。
///
/// `format`：`"diff"` = unified diff（前端按行首字符染色）；`"text"` =
/// 等宽纯文本。
pub fn build_preview(
    workspace: Option<&std::path::Path>,
    _operation: &str,
    _target: &str,
    args: &serde_json::Value,
) -> Option<(String, &'static str)> {
    if !args.is_object() {
        return None;
    }
    // 分派表按序首中（P5：一次只产一种 preview，不拼接）。
    if let Some(edits) = args.get("edits").and_then(|v| v.as_array()) {
        return build_multiedit_preview(workspace, edits);
    }
    if let (Some(path), Some(new_text)) = (
        args.get("path").and_then(|v| v.as_str()),
        args.get("content").and_then(|v| v.as_str()),
    ) {
        return build_write_preview(workspace, path, new_text);
    }
    if let (Some(path), Some(old_text), Some(new_text)) = (
        args.get("path").and_then(|v| v.as_str()),
        args.get("old_text").and_then(|v| v.as_str()),
        args.get("new_text").and_then(|v| v.as_str()),
    ) {
        return build_edit_preview(workspace, path, old_text, new_text);
    }
    if let Some(command) = args.get("command").and_then(|v| v.as_str()) {
        return Some((build_command_preview(command, args), "text"));
    }
    None
}

/// args path → 预览读侧的实际路径：绝对原样；相对按 workspace join（与
/// 工具层 resolve_workspace_path 的 `workspace.join(target)` 同语义——
/// BUG-20261106-033：此前 raw path 直读 = 相对形态按进程 cwd 解析，cwd
/// ≠ workspace 时预览读的是另一个文件或误报不存在）。无 workspace 时
/// 保持 raw（旧行为，测试环境）。
fn resolve_preview_path(workspace: Option<&std::path::Path>, path: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        match workspace {
            Some(ws) => ws.join(p),
            None => p.to_path_buf(),
        }
    }
}

/// write_file / append_file（args 形态 `path` + `content`）：读现盘 vs 新
/// 内容的 unified diff；文件不存在 = 全量新增；二进制/大文件 = 注记形态。
fn build_write_preview(
    workspace: Option<&std::path::Path>,
    path: &str,
    new_content: &str,
) -> Option<(String, &'static str)> {
    let p = resolve_preview_path(workspace, path);
    match std::fs::metadata(&p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // 新文件 = 全量新增 diff。
            let body = unified_diff("", new_content, path);
            Some((truncate_preview(&body), "diff"))
        }
        Err(e) => Some((
            truncate_preview(&format!(
                "读取当前文件失败：{e}\n将写入的新内容如下：\n{}",
                fence(new_content)
            )),
            "text",
        )),
        Ok(meta) => {
            if meta.len() > MAX_DIFF_SOURCE_BYTES {
                return Some((
                    format!(
                        "目标文件较大（{}），diff 预览略——请谨慎确认目标路径。",
                        human_size(meta.len())
                    ),
                    "text",
                ));
            }
            match std::fs::read(p) {
                Err(e) => Some((
                    truncate_preview(&format!(
                        "读取当前文件失败：{e}\n将写入的新内容如下：\n{}",
                        fence(new_content)
                    )),
                    "text",
                )),
                Ok(bytes) => {
                    if is_binary(&bytes) {
                        return Some((
                            format!(
                                "二进制文件（{}），不产 diff——请确认目标路径。",
                                human_size(meta.len())
                            ),
                            "text",
                        ));
                    }
                    let old = String::from_utf8_lossy(&bytes);
                    let body = unified_diff(&old, new_content, path);
                    Some((truncate_preview(&body), "diff"))
                }
            }
        }
    }
}

/// edit_file（`path` + `old_text` + `new_text`）：现盘做首次替换模拟 vs
/// 原文的 unified diff。与工具实现同语义（old_text 必须命中）。
fn build_edit_preview(
    workspace: Option<&std::path::Path>,
    path: &str,
    old_text: &str,
    new_text: &str,
) -> Option<(String, &'static str)> {
    let p = resolve_preview_path(workspace, path);
    match std::fs::read(&p) {
        Err(e) => Some((
            format!(
                "读取当前文件失败：{e}\n将把：\n{}\n替换为：\n{}",
                fence(old_text),
                fence(new_text)
            ),
            "text",
        )),
        Ok(bytes) => {
            if is_binary(&bytes) {
                return Some((
                    format!(
                        "二进制文件（{}），不产 diff——请确认目标路径。",
                        human_size(bytes.len() as u64)
                    ),
                    "text",
                ));
            }
            let old = String::from_utf8_lossy(&bytes);
            if !old.contains(old_text) {
                // old_text 不命中：替换模拟失败——工具执行时也会失败，预览
                // 如实说明（预览忠实原则）。
                return Some((
                    format!(
                        "old_text 未在当前文件中命中（执行时将报错）。拟替换：\n{}\n为：\n{}",
                        fence(old_text),
                        fence(new_text)
                    ),
                    "text",
                ));
            }
            let after = old.replacen(old_text, new_text, 1);
            let body = unified_diff(&old, &after, path);
            Some((truncate_preview(&body), "diff"))
        }
    }
}

/// multiedit（`edits: [{path, old_text, new_text}, ...]`）：逐文件模拟替换
/// （同文件按序累积——与工具原子语义一致），每文件一段 diff。
fn build_multiedit_preview(
    workspace: Option<&std::path::Path>,
    edits: &[serde_json::Value],
) -> Option<(String, &'static str)> {
    if edits.is_empty() {
        return None;
    }
    // path → (原文字节, 当前内容)；同文件按序累积（后者看到前者结果）。
    let mut files: std::collections::BTreeMap<String, (Vec<u8>, Option<String>)> =
        std::collections::BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();
    for (i, e) in edits.iter().enumerate() {
        let (Some(path), Some(old_text), Some(new_text)) = (
            e.get("path").and_then(|v| v.as_str()),
            e.get("old_text").and_then(|v| v.as_str()),
            e.get("new_text").and_then(|v| v.as_str()),
        ) else {
            return None; // 条目形态不齐 = 非本工具的 args，诚实不产。
        };
        let entry = files.entry(path.to_string()).or_insert_with(|| {
            let bytes = std::fs::read(resolve_preview_path(workspace, path)).unwrap_or_default();
            let text = if is_binary(&bytes) {
                None
            } else {
                Some(String::from_utf8_lossy(&bytes).into_owned())
            };
            (bytes, text)
        });
        let Some(current) = entry.1.as_mut() else {
            notes.push(format!(
                "{}：二进制文件，第 {} 条编辑不产 diff",
                path,
                i + 1
            ));
            continue;
        };
        if !current.contains(old_text) {
            notes.push(format!(
                "{}：第 {} 条 old_text 未命中（执行时将报错，整批不落盘）",
                path,
                i + 1
            ));
            continue;
        }
        *current = current.replacen(old_text, new_text, 1);
    }
    if files.is_empty() && notes.is_empty() {
        return None;
    }
    let mut out = String::new();
    for (path, (bytes, current)) in &files {
        let Some(current) = current else { continue };
        let original = String::from_utf8_lossy(bytes);
        out.push_str(&unified_diff(&original, current, path));
    }
    for n in &notes {
        out.push_str(n);
        out.push('\n');
    }
    if out.is_empty() {
        return None;
    }
    Some((truncate_preview(&out), "diff"))
}

/// exec 类（`command` + 可选 `cwd`）：完整命令现场。env 不展开（防凭据经
/// 预览泄漏）。
fn build_command_preview(command: &str, args: &serde_json::Value) -> String {
    let cwd = args
        .get("cwd")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "(默认工作目录)".to_string());
    format!("将执行命令：\n{}\n工作目录：{}", fence(command), cwd)
}

/// 统一包裹：preview 文本统一用三反引号围栏（命令现场/新内容降级形态可读；
/// diff 形态由前端等宽染色渲染，围栏不影响）。
fn fence(text: &str) -> String {
    format!("```\n{}\n```", text)
}

/// 二进制探测：前 8KB 含 NUL 字节。
fn is_binary(bytes: &[u8]) -> bool {
    let window = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    window.contains(&0u8)
}

fn human_size(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// 64KB 截断 + 尾注记（字符边界安全——按字符迭代不劈 UTF-8）。
fn truncate_preview(text: &str) -> String {
    if text.len() <= MAX_PREVIEW_BYTES {
        return text.to_string();
    }
    let mut end = MAX_PREVIEW_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n… [预览已截断：完整内容 {} 字节]",
        &text[..end],
        text.len()
    )
}

/// 行级 unified diff（公共前缀/后缀 + 中段全 `-`/`+`）。非最优 LCS——审批
/// 预览只需合法 diff 形态与可读性，不引第三方 diff crate。
fn unified_diff(old: &str, new: &str, path: &str) -> String {
    let a: Vec<&str> = if old.is_empty() {
        Vec::new()
    } else {
        old.lines().collect()
    };
    let b: Vec<&str> = if new.is_empty() {
        Vec::new()
    } else {
        new.lines().collect()
    };
    // 空文件行数修正：lines() 对空串给 0 行，但「空文件 → 有内容」要显式
    // 表达为全量新增（-0,0 +1,N）。
    let (old_label, new_label) = (format!("a/{path}"), format!("b/{path}"));
    if a.is_empty() && b.is_empty() {
        return String::new();
    }
    // 公共前缀
    let mut pre = 0usize;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    // 公共后缀（不越过前缀）
    let mut suf = 0usize;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf]
    {
        suf += 1;
    }
    let mid_a = &a[pre..a.len() - suf];
    let mid_b = &b[pre..b.len() - suf];
    if mid_a.is_empty() && mid_b.is_empty() {
        return String::new(); // 无变化
    }
    // 上下文行数：变化区前后各 1 行（可用时），计入 hunk 行数——标准
    // unified diff 语义，hunk 头描述含上下文的完整窗口。
    let lead = usize::min(pre, 1);
    let tail = usize::min(suf, 1);
    let mut out = String::new();
    out.push_str(&format!("--- {old_label}\n+++ {new_label}\n"));
    // hunk 头行号：1 起；空侧记 0（全量新增 `-0,0` / 全量删除 `+0,0`）。
    let a_from = if a.is_empty() { 0 } else { pre + 1 - lead };
    let b_from = if b.is_empty() { 0 } else { pre + 1 - lead };
    out.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        a_from,
        lead + mid_a.len() + tail,
        b_from,
        lead + mid_b.len() + tail
    ));
    for l in &a[pre - lead..pre] {
        out.push(' ');
        out.push_str(l);
        out.push('\n');
    }
    for l in mid_a {
        out.push('-');
        out.push_str(l);
        out.push('\n');
    }
    for l in mid_b {
        out.push('+');
        out.push_str(l);
        out.push('\n');
    }
    for l in &b[b.len() - suf..b.len() - suf + tail] {
        out.push(' ');
        out.push_str(l);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests;
