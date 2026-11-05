// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// Grep tool — regex search across workspace files. Returns file:line: match.
pub struct GrepTool {
    workspace: String,
}

impl GrepTool {
    pub fn new(workspace: String) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn description(&self) -> String {
        "Search file contents with a regex pattern across the workspace. Returns matching \
         lines as file:line: content. Use to find code, symbols, or text. Faster and more \
         structured than exec grep."
            .to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Regex pattern to search for"},
                "path": {"type": "string", "description": "Directory or single file to search (default: workspace root)"},
                "glob": {"type": "string", "description": "File-name filter, e.g. *.rs (simple suffix match)"},
                "max_results": {"type": "integer", "description": "Max matches (default 50)"}
            },
            "required": ["pattern"]
        })
    }
    async fn execute(&self, args: &str, _ctx: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON: {}", e))?;
        let pattern = v["pattern"].as_str().ok_or("missing 'pattern'")?;
        let re = regex::Regex::new(pattern).map_err(|e| format!("invalid regex: {}", e))?;
        // FT：相对 path 锚定工作区根——此前只有缺省时回退
        // workspace，显式相对子目录按进程 cwd 解析（exe 直启形态下搜错树）。
        let root = match v["path"].as_str() {
            Some(s) if !s.is_empty() => {
                let p = Path::new(s);
                if p.is_absolute() {
                    s.to_string()
                } else {
                    Path::new(&self.workspace)
                        .join(p)
                        .to_string_lossy()
                        .to_string()
                }
            }
            _ => self.workspace.clone(),
        };
        let glob = v["glob"].as_str();
        let max = v["max_results"].as_u64().unwrap_or(50) as usize;
        let root_path = Path::new(&root);
        // （产品验证实验发现）：path=单文件此前落 grep_recursive 的
        // read_dir → Err 静默吞 → 假「No matches」且 ok=true，模型反复换
        // pattern 重试烧轮次（USAGCSV 轮实证：Cargo.toml 连查 5 次）。文件
        // 路径升为一等公民：直接扫该文件；glob 不匹配 / 超限 / 非 UTF-8
        // 一律诚实注明，绝不裸「No matches」掩盖「根本没搜」。
        if root_path.is_file() {
            if let Some(g) = glob {
                let suffix = g.trim_start_matches('*');
                let matched = root_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|f| f.ends_with(suffix))
                    .unwrap_or(false);
                if !matched {
                    return Ok(format!("Skipped {}: does not match glob '{}'.", root, g));
                }
            }
            if let Ok(meta) = std::fs::metadata(root_path)
                && meta.len() > MAX_GREP_SINGLE_FILE_BYTES
            {
                return Ok(format!(
                    "Skipped {}: {} bytes exceeds the single-file scan limit ({} bytes); \
                     use read_file with offsets instead.",
                    root,
                    meta.len(),
                    MAX_GREP_SINGLE_FILE_BYTES
                ));
            }
            let mut out: Vec<String> = Vec::new();
            if grep_file(root_path, &re, max, &mut out) {
                if out.is_empty() {
                    Ok(format!("No matches for pattern /{}/ in {}", pattern, root))
                } else {
                    Ok(format!(
                        "Found {} match(es):\n{}",
                        out.len(),
                        out.join("\n")
                    ))
                }
            } else {
                Ok(format!(
                    "Skipped {}: not valid UTF-8 (binary file?) — not scanned.",
                    root
                ))
            }
        } else if root_path.is_dir() {
            let mut out: Vec<String> = Vec::new();
            grep_recursive(root_path, &re, glob, max, &mut out);
            if out.is_empty() {
                Ok(format!("No matches for pattern /{}/", pattern))
            } else {
                Ok(format!(
                    "Found {} match(es):\n{}",
                    out.len(),
                    out.join("\n")
                ))
            }
        } else {
            Err(format!("path '{}' does not exist", root))
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// git 单次执行的归一输出（普通 piped / 受限文件重定向两形态同构，
/// [`GitTool:run_git`] 产物、三态判定与 commit 专用路径共用）。
struct GitOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

/// Git tool — read queries (status/diff/log/show/branch) plus the daily-safe
/// write actions (add/commit/branch_create/checkout/restore/stash) in the
/// workspace.: the write surface is enum-whitelisted — push,
/// reset --hard, clean, rebase … are intentionally NOT reachable here (the
/// model must use exec for those, which goes through the 8-layer security
/// pipeline and approval). Write actions take dedicated params (paths/message/
/// name/ref), not the free-form `args` string, so flags can't be smuggled in.
pub struct GitTool {
    workspace: String,
}

impl GitTool {
    pub fn new(workspace: String) -> Self {
        Self { workspace }
    }

    /// git 子进程执行的单一入口。 黑窗根修后：普通模式 =
    /// `output()`（隐式 piped）+ CREATE_NO_WINDOW 压黑窗（git.exe 是
    /// console 程序，release gateway 无 console 下默认 flags 会闪窗）；
    /// workspace-dacl 受限模式 = 工作区 `.exec_out/` 文件重定向 +
    /// `status()` + 读回（std 匿名管道创建在受限令牌下必死
    /// ——ShellOutputCapture 注释）+ console flags 单一真相源（新分配在
    /// 受限树下必死，附着成功=继承/未附着=DETACHED）。
    /// GitTool 在 MOVE_TOOLS 里（exec_worker 内运行），三处旧 `.output()`
    /// 调用在受限形态下全部 os error 5——本 helper 是它们的共同根修。
    fn run_git(&self, args: &[&str]) -> Result<GitOutput, String> {
        let mut c = std::process::Command::new("git");
        c.current_dir(&self.workspace).args(args);
        // 环境凭据剥离：git 子进程链会经钩子执行
        // 任意仓库脚本（模型可控内容），钩子进程不得看到继承的凭据变量。
        let stripped = nemesis_utils::env_sanitize::sanitize_command(&mut c);
        if stripped > 0 {
            debug!("[Tools] env sanitized: {stripped} vars stripped");
        }
        #[cfg(target_os = "windows")]
        {
            if let Some(f) = console_creation_flags() {
                use std::os::windows::process::CommandExt;
                c.creation_flags(f);
            }
        }
        let mut capture = ShellOutputCapture::current()
            .map_err(|e| format!("failed to run git: output capture setup failed: {e}"))?;
        if let Some(cap) = capture.as_mut() {
            cap.apply_to_std(&mut c);
        }
        match capture {
            Some(cap) => {
                let status = c.status().map_err(|e| format!("failed to run git: {e}"))?;
                let (stdout, stderr) = cap.read_back();
                Ok(GitOutput {
                    success: status.success(),
                    stdout,
                    stderr,
                })
            }
            None => {
                let out = c.output().map_err(|e| format!("failed to run git: {e}"))?;
                Ok(GitOutput {
                    success: out.status.success(),
                    stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                })
            }
        }
    }

    /// Run git with `args` in the workspace and format the output using the
    /// three-state convention shared by all read actions:
    /// fail with empty stdout → Err(stderr) / empty output → no-changes note /
    /// otherwise → stdout. (write actions reuse the same three states.)
    fn run_three_state(&self, args: &[&str], action: &str) -> Result<String, String> {
        let out = self.run_git(args)?;
        if !out.success && out.stdout.trim().is_empty() {
            Err(format!("git {} failed: {}", action, out.stderr.trim()))
        } else if out.stdout.trim().is_empty() {
            Ok(format!("(no changes / empty)\n{}", out.stderr.trim()))
        } else {
            Ok(out.stdout)
        }
    }

    /// Write actions take dedicated keys, not the free-form `args` string
    /// (anti flag-smuggling). `args` on a write action is an honest error
    /// that tells the model the right shape (same philosophy as edit hints).
    fn reject_freeform_args(
        &self,
        v: &serde_json::Value,
        action: &str,
        takes_args: bool,
    ) -> Result<(), String> {
        if !takes_args && !v["args"].as_str().unwrap_or("").trim().is_empty() {
            return Err(format!(
                "action '{}' takes dedicated params, not free-form 'args' (see the tool schema)",
                action
            ));
        }
        Ok(())
    }

    fn required_str<'a>(
        &self,
        v: &'a serde_json::Value,
        key: &str,
        action: &str,
    ) -> Result<&'a str, String> {
        v[key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("action '{}' requires a non-empty '{}' param", action, key))
    }
}

#[async_trait]
impl Tool for GitTool {
    fn description(&self) -> String {
        "Run git commands in the workspace. Read: status, diff, log, show, branch. Safe writes \
         (D1): add, commit, branch_create, checkout, restore, stash (list/push/pop). Dangerous \
         git ops (push, reset --hard, clean, rebase, ...) are intentionally NOT exposed — use \
         the exec tool for those (it goes through the security pipeline and approval)."
            .to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["status", "diff", "log", "show", "branch", "add", "commit", "branch_create", "checkout", "restore", "stash"], "description": "Git subcommand"},
                "args": {"type": "string", "description": "Extra args for read actions (e.g. a file path for diff/show, or '-10' for log count); for 'stash' it selects the op: list (default), push, or pop"},
                "paths": {"type": "string", "description": "Whitespace-separated paths for add/restore (e.g. 'src/main.rs' or '-A' to stage everything)"},
                "message": {"type": "string", "description": "Commit message for 'commit' (required); optional message for 'stash' push"},
                "name": {"type": "string", "description": "Branch name for 'branch_create'"},
                "ref": {"type": "string", "description": "Branch or commit to switch to for 'checkout'"}
            },
            "required": ["action"]
        })
    }
    async fn execute(&self, args: &str, _ctx: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON: {}", e))?;
        let action = v["action"].as_str().ok_or("missing 'action'")?;
        let extra = v["args"].as_str().unwrap_or("");

        // --- Read actions: free-form extra args, as before (unchanged) ---
        let base: Vec<&str> = match action {
            "status" => vec!["status", "--short", "--branch"],
            "diff" => vec!["diff"],
            "log" => vec!["log", "--oneline", "-20"],
            "show" => vec!["show"],
            "branch" => vec!["branch", "-vv"],
            // --- Write actions: dedicated params, enum-whitelisted ---
            "add" => {
                self.reject_freeform_args(&v, action, false)?;
                let paths = self.required_str(&v, "paths", action)?;
                let mut base = vec!["add"];
                base.extend(paths.split_whitespace());
                return self.run_three_state(&base, action);
            }
            "commit" => {
                self.reject_freeform_args(&v, action, false)?;
                let message = self.required_str(&v, "message", action)?;
                // Not run_three_state: `git commit` prints its failure text
                // ("nothing to commit", hook rejections) on *stdout* with exit
                // code 1, which the shared three-state would surface as Ok —
                // a failed commit must be an honest Err. Success text goes to
                // stdout as usual.
                let out = self.run_git(&["commit", "-m", message])?;
                let stdout = out.stdout;
                let stderr = out.stderr;
                if !out.success {
                    let detail = if stderr.trim().is_empty() {
                        stdout.trim()
                    } else {
                        stderr.trim()
                    };
                    return Err(format!("git commit failed: {}", detail));
                }
                // Best-effort: show the new commit (hash + subject) so the
                // model immediately sees the result of the write.
                let mut result = stdout;
                if let Ok(o) = self.run_git(&["log", "--oneline", "-1"]) {
                    let line = o.stdout;
                    if !line.trim().is_empty() {
                        result.push_str(&format!("\nLast commit: {}", line.trim()));
                    }
                }
                return Ok(result);
            }
            "branch_create" => {
                self.reject_freeform_args(&v, action, false)?;
                let name = self.required_str(&v, "name", action)?;
                return self.run_three_state(&["branch", name], action);
            }
            "checkout" => {
                self.reject_freeform_args(&v, action, false)?;
                let target = self.required_str(&v, "ref", action)?;
                return self.run_three_state(&["checkout", target], action);
            }
            "restore" => {
                self.reject_freeform_args(&v, action, false)?;
                let paths = self.required_str(&v, "paths", action)?;
                let mut base = vec!["restore"];
                base.extend(paths.split_whitespace());
                return self.run_three_state(&base, action);
            }
            "stash" => {
                // Op whitelist: only list/push/pop (empty = list). Drop/clear/
                // apply and any flags are rejected — stash stays recoverable.
                let op = extra.trim();
                let base: Vec<&str> = match op {
                    "" | "list" => vec!["stash", "list"],
                    "push" => {
                        let mut base = vec!["stash", "push"];
                        if let Some(m) = v["message"].as_str().filter(|m| !m.trim().is_empty()) {
                            base.push("-m");
                            base.push(m.trim());
                        }
                        base
                    }
                    "pop" => vec!["stash", "pop"],
                    other => {
                        return Err(format!(
                            "unsupported stash op '{}' (allowed: list, push, pop)",
                            other
                        ));
                    }
                };
                return self.run_three_state(&base, action);
            }
            other => {
                return Err(format!(
                    "unknown git action '{}' (write actions: add/commit/branch_create/checkout/restore/stash; \
                     push/reset --hard and other dangerous ops are not exposed — use exec, \
                     which goes through the security pipeline)",
                    other
                ));
            }
        };
        let mut cmd_args = base;
        if !extra.is_empty() {
            // （审计修复）：读操作此前接受 free-form 选项——
            // `git diff --output=<path>` 可写任意文件、`--ext-diff` 可让 git
            // 执行外部 diff 程序，读原语实为写/执行原语。封死规则：`--` 长
            // 选项与短选项一律拒绝；唯一例外是 schema 文档化的 `-<N>` 纯数字
            // 形态（log 条数）。读操作传文件路径无需选项。
            for tok in extra.split_whitespace() {
                if let Some(rest) = tok.strip_prefix('-')
                    && (tok.starts_with("--")
                        || rest.is_empty()
                        || !rest.bytes().all(|b| b.is_ascii_digit()))
                {
                    return Err(format!(
                        "git read action '{}' does not accept option '{}': read actions take \
                         only plain file paths (--output/--ext-diff style options would turn \
                         this read primitive into a write/exec primitive); '-<N>' (log count) \
                         is the only allowed form",
                        action, tok
                    ));
                }
                cmd_args.push(tok);
            }
        }
        self.run_three_state(&cmd_args, action)
    }
}

/// Recursively search `dir` for regex matches, collecting file:line: content lines.
/// Skips hidden/build/dep dirs and large or non-UTF-8 files.
pub(super) fn grep_recursive(
    dir: &Path,
    re: &regex::Regex,
    glob: Option<&str>,
    max: usize,
    out: &mut Vec<String>,
) {
    if out.len() >= max {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut kids: Vec<_> = entries.flatten().collect();
    // 确定性走查序：`max` 截断让返回的命中子集依赖遍历序，
    // 而 read_dir 顺序是平台实现细节（NTFS 字典序 vs ext4 哈希序）——每层
    // 按文件名排序，同一目录树跨平台命中同一子集（同 history_search 的
    // limit 截断先例）。
    kids.sort_by_key(|e| e.file_name());
    for entry in kids {
        if out.len() >= max {
            return;
        }
        let path = entry.path();
        // 目录符号链接跳过。file_type() 不穿越链接——
        // 旧代码 path.is_dir() 跟随链接，链接环下同一文件以「loop\loop\...」
        // 假路径重复命中几十层（直到 OS 路径长度上限才收敛）。
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) && path.is_dir() {
            continue;
        }
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.')
                || matches!(name, "target" | "node_modules" | "dist" | "build" | ".git")
            {
                continue;
            }
            grep_recursive(&path, re, glob, max, out);
        } else if path.is_file() {
            if let Some(g) = glob
                && let Some(fname) = path.file_name().and_then(|n| n.to_str())
            {
                let suffix = g.trim_start_matches('*');
                if !fname.ends_with(suffix) {
                    continue;
                }
            }
            if let Ok(meta) = std::fs::metadata(&path)
                && meta.len() > 1_000_000
            {
                continue;
            }
            grep_file(&path, re, max, out);
        }
    }
}

/// 单文件逐行扫描（glob 过滤与 1MB 走查帽归调用方——目录走查里它们是噪声
/// 过滤，显式单文件路径是明确意图，门槛不同：见 `MAX_GREP_SINGLE_FILE_BYTES`）。
/// 返回是否真正扫描了内容（非 UTF-8 → false；调用方据此诚实注记，不以
/// 「No matches」掩盖「根本没搜」—— 假阴性缺陷的根修拆分）。
fn grep_file(path: &Path, re: &regex::Regex, max: usize, out: &mut Vec<String>) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    for (i, line) in content.lines().enumerate() {
        if out.len() >= max {
            return true;
        }
        if re.is_match(line) {
            let display = line.trim();
            if display.len() > 300 {
                // Truncate at the nearest char boundary ≤ 300 bytes.
                // Slicing at a fixed byte index can land inside a
                // multibyte UTF-8 char (e.g. Chinese) and panic.
                let mut end = 300;
                while !display.is_char_boundary(end) {
                    end -= 1;
                }
                out.push(format!(
                    "{}:{}: {}…",
                    path.display(),
                    i + 1,
                    &display[..end]
                ));
            } else {
                out.push(format!("{}:{}: {}", path.display(), i + 1, display));
            }
        }
    }
    true
}

/// 显式单文件扫描上限。目录走查的 1MB 帽是走查噪声过滤；显式路径是明确
/// 意图，放宽到 20MB——read_to_string 全量入内存，仍需有界。超限诚实报
/// 告（不静默跳过），指路 read_file 分段。
const MAX_GREP_SINGLE_FILE_BYTES: u64 = 20_000_000;

// 跨子文件引用（机械拆分）：ShellOutputCapture/console_creation_flags 定义在 exec.rs
use super::exec::ShellOutputCapture;
#[cfg(target_os = "windows")]
use super::exec::console_creation_flags;
