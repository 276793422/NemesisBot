//! guest 工具子集实现（宿主 MOVE_TOOLS 同形语义，musl Linux 栖息地）。
//!
//! 语义对齐锚点（文案同形，让模型对 guest 与本地 executor 无感知差异）：
//! - `exec` ↔ `nemesis-agent::loop_tools::ExecTool`（stdout / "(no output)" /
//!   "Exit code: N\nstdout: ...\nstderr: ..."；spawn 失败与超时为 Err）；
//! - 文件工具 ↔ `nemesis-tools/src/filesystem.rs`（"wrote N bytes to ..."、
//!   "created directory: ..."、"deleted file: ..." 等）+ append_file 对齐
//!   `loop_tools::AppendFileTool`（"Appended N bytes to ..."）。
//!
//! 差异（刻意，guest 栖息地语义）：
//! - shell 恒 `sh -c`（musl Linux）；宿主直跑（Windows 测试形态）退
//!   `cmd /C`——与宿主 ExecTool 的平台分派同形；
//! - 边界归一化用 std canonicalize（无 8.3 短名问题——VM 内没有）；
//! - env 注入前先剥继承凭据类变量的宿主 sanitize 不进来（VM 内无宿主
//!   凭据可泄），显式 env 照常透传。

use std::path::Path;
use std::time::Duration;

use crate::resolve_in_workspace;

/// exec 超时：缺省 30s、上限 600s（对齐宿主 ExecTool 的
/// `exec_timeout_secs` 口径）。
const EXEC_DEFAULT_TIMEOUT_SECS: u64 = 30;
const EXEC_MAX_TIMEOUT_SECS: u64 = 600;

/// 输出截断（guest 侧独立于宿主 spill/prune 体系——VM 通道的 spill 落盘
/// 归宿主管；这里只防单响应撑爆 TCP 帧的极端形态，head+tail 同宿主
/// shell.rs 的截断纪律）。
const TRUNCATE_HEAD_CHARS: usize = 4000;
const TRUNCATE_TAIL_CHARS: usize = 4000;

/// 派发一个工具调用。`args` 是 LLM 产出的原始 JSON 串（协议原样透传）。
pub async fn dispatch(tool: &str, args: &str, workspace: &Path) -> Result<String, String> {
    let val: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {e}"))?;
    match tool {
        "exec" => exec(&val, workspace).await,
        "read_file" => read_file(&val, workspace).await,
        "write_file" => write_file(&val, workspace).await,
        "append_file" => append_file(&val, workspace).await,
        "list_dir" => list_dir(&val, workspace).await,
        "create_dir" => create_dir(&val, workspace).await,
        "delete_file" => delete_file(&val, workspace).await,
        "delete_dir" => delete_dir(&val, workspace).await,
        // 诚实拒绝未实现项（edit_file/multiedit/grep/git 往后续补）。
        other => Err(format!(
            "unknown tool: {other} (guest subset: exec, read_file, write_file, append_file, \
             list_dir, create_dir, delete_file, delete_dir)"
        )),
    }
}

// ---------------------------------------------------------------------------
// exec（对齐宿主 ExecTool）
// ---------------------------------------------------------------------------

async fn exec(val: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let command = val
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'command' argument".to_string())?;

    let timeout_secs = val
        .get("timeout")
        .and_then(|v| v.as_u64())
        .unwrap_or(EXEC_DEFAULT_TIMEOUT_SECS)
        .min(EXEC_MAX_TIMEOUT_SECS);

    let cwd = match val.get("cwd").and_then(|v| v.as_str()) {
        Some(dir) => resolve_in_workspace(workspace, dir)?,
        None => workspace.to_path_buf(),
    };

    let (shell, flag) = if cfg!(target_os = "windows") {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let mut cmd = tokio::process::Command::new(shell);
    cmd.arg(flag).arg(command).current_dir(&cwd);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // B2 同款纪律：管道读取独立任务并发 drain——只 wait 会在写满管道缓冲时
    // 死锁；把管道移进被 timeout 包住的 future 会在超时取消时丢残余输出。
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to execute command: {e}"))?;
    let out_task = tokio::spawn(drain_pipe(child.stdout.take()));
    let err_task = tokio::spawn(drain_pipe(child.stderr.take()));

    let waited = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait()).await;
    match waited {
        Ok(Ok(status)) => {
            let stdout = out_task.await.unwrap_or_default();
            let stderr = err_task.await.unwrap_or_default();
            if status.success() {
                Ok(if stdout.trim().is_empty() {
                    "(no output)".to_string()
                } else {
                    truncate(stdout.trim())
                })
            } else {
                // 非零退出按宿主 ExecTool 语义 = Ok 带 Exit code 文本
                // （模型靠文本自纠，不是传输层错误）。
                Ok(truncate(&format!(
                    "Exit code: {}\nstdout: {}\nstderr: {}",
                    status.code().unwrap_or(-1),
                    stdout,
                    stderr
                )))
            }
        }
        Ok(Err(e)) => Err(format!("Failed to execute command: {e}")),
        Err(_) => Err(format!("command timed out after {timeout_secs}s")),
    }
}

async fn drain_pipe(pipe: Option<impl tokio::io::AsyncRead + Unpin + Send>) -> String {
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    let mut buf = Vec::new();
    if tokio::io::AsyncReadExt::read_to_end(&mut pipe, &mut buf)
        .await
        .is_ok()
    {
        String::from_utf8_lossy(&buf).into_owned()
    } else {
        String::new()
    }
}

fn truncate(s: &str) -> String {
    let total = s.chars().count();
    if total <= TRUNCATE_HEAD_CHARS + TRUNCATE_TAIL_CHARS {
        return s.to_string();
    }
    let head: String = s.chars().take(TRUNCATE_HEAD_CHARS).collect();
    let tail: String = s.chars().skip(total - TRUNCATE_TAIL_CHARS).collect();
    format!(
        "{head}\n... (truncated, {} chars omitted in the middle; full output had {total} chars — \
         narrow the command or redirect to a file and read it with offset/limit)\n...{tail}",
        total - TRUNCATE_HEAD_CHARS - TRUNCATE_TAIL_CHARS
    )
}

// ---------------------------------------------------------------------------
// 文件工具（对齐 nemesis-tools filesystem.rs / loop_tools AppendFileTool）
// ---------------------------------------------------------------------------

fn arg_path(val: &serde_json::Value) -> Result<&str, String> {
    val.get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'path' argument".to_string())
}

async fn read_file(val: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let path = resolve_in_workspace(workspace, arg_path(val)?)?;
    tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("failed to read file: {e}"))
}

async fn write_file(val: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let path_str = arg_path(val)?.to_string();
    let content = val
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("missing 'content' argument")?;
    let path = resolve_in_workspace(workspace, &path_str)?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("failed to create directories: {e}"))?;
    }
    tokio::fs::write(&path, content)
        .await
        .map_err(|e| format!("failed to write file: {e}"))?;
    Ok(format!("wrote {} bytes to {}", content.len(), path_str))
}

async fn append_file(val: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let path_str = arg_path(val)?.to_string();
    let content = val
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("missing 'content' argument")?;
    let path = resolve_in_workspace(workspace, &path_str)?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("failed to create directories: {e}"))?;
    }
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .map_err(|e| format!("Failed to open file: {e}"))?;
    file.write_all(content.as_bytes())
        .await
        .map_err(|e| format!("Failed to append to file: {e}"))?;
    file.flush()
        .await
        .map_err(|e| format!("Failed to flush file: {e}"))?;
    Ok(format!(
        "Appended {} bytes to {}",
        content.len(),
        path.display()
    ))
}

async fn list_dir(val: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let path_str = val.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let path = resolve_in_workspace(workspace, path_str)?;
    let mut entries = tokio::fs::read_dir(&path)
        .await
        .map_err(|e| format!("failed to list directory: {e}"))?;
    let mut listing = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        let suffix = match entry.file_type().await {
            Ok(ft) if ft.is_dir() => "/",
            _ => "",
        };
        listing.push(format!("{name}{suffix}"));
    }
    // 有序消费 read_dir 结果（diff 纪律：目录顺序是平台实现细节）。
    listing.sort();
    Ok(listing.join("\n"))
}

async fn create_dir(val: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let path_str = arg_path(val)?.to_string();
    let path = resolve_in_workspace(workspace, &path_str)?;
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(|e| format!("failed to create directory: {e}"))?;
    Ok(format!("created directory: {path_str}"))
}

async fn delete_file(val: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let path_str = arg_path(val)?.to_string();
    let path = resolve_in_workspace(workspace, &path_str)?;
    tokio::fs::remove_file(&path)
        .await
        .map_err(|e| format!("failed to delete file: {e}"))?;
    Ok(format!("deleted file: {path_str}"))
}

async fn delete_dir(val: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let path_str = arg_path(val)?.to_string();
    let path = resolve_in_workspace(workspace, &path_str)?;
    match tokio::fs::metadata(&path).await {
        Ok(meta) if !meta.is_dir() => return Err(format!("'{path_str}' is not a directory")),
        Ok(_) => {}
        Err(e) => return Err(format!("failed to access directory '{path_str}': {e}")),
    }
    tokio::fs::remove_dir_all(&path)
        .await
        .map_err(|e| format!("failed to delete directory: {e}"))?;
    Ok(format!("Directory deleted: {path_str}"))
}
