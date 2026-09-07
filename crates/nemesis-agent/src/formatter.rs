//! A6（2026-09-06 devtool-upgrade 阶段 5）：format-on-save 格式化钩子。
//!
//! `write_file` / `edit_file` 成功后按文件扩展名选格式化工具（声明式表，
//! 常用格式化器子集），spawn 带 3s 超时；成功且文件实际变化时重读 diff
//! （复用 A2 [`crate::loop_tools::edit_hint::unified_diff`]），把
//! `[format] reformatted by {tool}` + diff 追加到工具结果尾部——模型同轮
//! 看到格式化后的真实形态。全部失败/未命中路径**静默原样返回**（开关关 /
//! 非文本文件 / 无匹配格式化器 / spawn 失败 / 超时 / 内容未变）——格式化
//! 永不拖垮工具调用。
//!
//! 调用点在 loop.rs 工具瀑布（K1a）内部、`run_post_hooks`（cc_hooks
//! PostToolUse 等）**之前**——用户自定义 hook 看到的是格式化后的文件
//! （计划明文的正交语义）；外层 C3 诊断带在瀑布之后，因此诊断同样作用于
//! 格式化后的文件（先 format 后 diagnostics 的次序）。
//!
//! 信任边界（诚实声明）：spawn 的格式化工具来自 PATH 或用户 config 覆盖表
//! ——用户本机终端同级信任（同 K3 `!`cmd`` 注入），**不过** 9 层安全管线
//! （管线管的是 LLM 工具调用；格式化是已批准 write/edit 的 config 门控后
//! 处理，与 LSP 服务器 spawn 同级）。`executor.enabled=true` 时诚实停用：
//! write/edit 落在子进程（Layer 2 甚至在 Sandboxie 盒内），gateway 侧后
//! 格式化会读到陈旧真盘内容甚至绕盒写。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nemesis_config::FormatOnSaveConfig;

/// 单次格式化的墙钟预算（超时 kill 子进程，静默放行）。
const FORMAT_TIMEOUT_SECS: u64 = 3;

/// 一条格式化命令：工具名 + argv 模板（`{file}` 占位符替换为目标路径）。
#[derive(Debug, Clone, PartialEq)]
pub struct FormatterSpec {
    pub tool: String,
    pub args: Vec<String>,
}

/// 内置声明式表（常用工具的默认覆盖；扩展名小写）。
fn builtin_spec(ext: &str) -> Option<FormatterSpec> {
    let (tool, args): (&str, &[&str]) = match ext {
        "rs" => ("rustfmt", &["--edition", "2021", "{file}"]),
        "go" => ("gofmt", &["-w", "{file}"]),
        "ts" | "tsx" | "js" | "jsx" | "vue" | "json" | "md" => ("prettier", &["--write", "{file}"]),
        "py" => ("black", &["-q", "{file}"]),
        "c" | "h" | "cpp" => ("clang-format", &["-i", "{file}"]),
        _ => return None,
    };
    Some(FormatterSpec {
        tool: tool.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
    })
}

/// 解析格式化器：配置覆盖表按条目优先（首元素 = 工具名），未命中回落内置
/// 表。空 argv 条目视为未配置（回落内置——用户写 `["py"]: []` 不该炸）。
pub fn resolve_spec(ext: &str, overrides: &BTreeMap<String, Vec<String>>) -> Option<FormatterSpec> {
    if let Some(argv) = overrides.get(ext)
        && let [tool, rest @ ..] = argv.as_slice()
        && !tool.is_empty()
    {
        return Some(FormatterSpec {
            tool: tool.clone(),
            args: rest.to_vec(),
        });
    }
    builtin_spec(ext)
}

/// argv 模板渲染：`{file}` 占位符替换为目标路径（原样其余参数）。
pub fn render_args(args: &[String], file: &str) -> Vec<String> {
    args.iter().map(|a| a.replace("{file}", file)).collect()
}

/// 工具结果注记（纯函数）：`[format] reformatted by {tool}` + unified diff。
fn annotate_reformat(result: &str, tool: &str, path: &str, old: &str, new: &str) -> String {
    let diff = crate::loop_tools::edit_hint::unified_diff(path, old, new);
    format!("{}\n\n[format] reformatted by {}\n{}", result, tool, diff)
}

/// 从 config.json 原样读 `agents.defaults.format_on_save`（config_path 每次
/// 新鲜读，同 C3 `current_diagnostics_loop` 模式——运行中可翻转开关）。
/// 缺段 / standalone / 坏 JSON → 全默认（enabled=false）。
fn resolve_config(config_path: Option<&Path>) -> (FormatOnSaveConfig, bool) {
    let Some(p) = config_path else {
        return (FormatOnSaveConfig::default(), false);
    };
    let parsed = std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    match parsed {
        Some(v) => {
            let cfg = v
                .get("agents")
                .and_then(|a| a.get("defaults"))
                .and_then(|d| d.get("format_on_save"))
                .and_then(|s| serde_json::from_value::<FormatOnSaveConfig>(s.clone()).ok())
                .unwrap_or_default();
            // executor 分离（Layer 1/2）开启 → 格式化诚实停用（见模块注释）。
            let executor_on = v
                .get("executor")
                .and_then(|e| e.get("enabled"))
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            (cfg, executor_on)
        }
        None => (FormatOnSaveConfig::default(), false),
    }
}

/// format-on-save 入口（loop.rs 瀑布 Ok 臂调用）。`config_path` = AgentLoop
/// 持有的 config.json 路径（None = standalone）；`result` = 工具成功结果。
/// 返回装饰后（或原样）的工具结果。
pub async fn format_on_save(config_path: Option<PathBuf>, path: &str, result: &str) -> String {
    let (cfg, executor_on) = resolve_config(config_path.as_deref());
    if !cfg.enabled || executor_on {
        return result.to_string();
    }
    let Some(ext) = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
    else {
        return result.to_string();
    };
    let Some(spec) = resolve_spec(&ext, &cfg.formatters) else {
        return result.to_string();
    };
    // 前置读（拿到格式化前内容）：非 UTF-8 / 读不到 → 静默（文本表之外或
    // 文件已消失，格式化器自己也无能为力）。
    let Ok(old) = std::fs::read_to_string(path) else {
        return result.to_string();
    };
    if !format_file(&spec, path, Duration::from_secs(FORMAT_TIMEOUT_SECS)).await {
        return result.to_string();
    }
    let Ok(new) = std::fs::read_to_string(path) else {
        return result.to_string();
    };
    if old == new {
        return result.to_string();
    }
    annotate_reformat(result, &spec.tool, path, &old, &new)
}

/// spawn 格式化工具 + 超时 kill。spawn 失败（工具不在 PATH）/ 超时 / 非零
/// 退出一律 false（调用方静默）。工具不存在不预探测——直接 spawn，NotFound
/// 与其它失败同走静默路径（可观测行为与 which 预探测一致，省一次子进程）。
async fn format_file(spec: &FormatterSpec, path: &str, timeout: Duration) -> bool {
    let mut cmd = tokio::process::Command::new(&spec.tool);
    cmd.args(render_args(&spec.args, path))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // 超时分支 drop Child 时兜底 kill（不留孤儿格式化进程）。
        .kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) => out.status.success(),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
