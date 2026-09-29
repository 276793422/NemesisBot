//! translate 示例插件（guest 侧，world `plugin-tool`）。
//!
//! 三方开发者照这里写工具插件：实现 [`Guest`] 两方法，`export!` 收尾。
//!
//! 本插件同时是宿主 e2e / 对抗测试的行为夹具，`mode` 字段切换行为：
//! - `echo`：回显 `text`（基线往返）。
//! - `host_calls`：一次跑全宿主能力面（log / now-millis / config-get /
//!   data-dir 落盘 / tool-invoke 预期 unavailable），返回摘要 JSON。
//! - `egress_probe`：http-send 打 example.com（无 allowlist 应被宿主拒绝）。
//! - `fuel_bomb`：死循环（fuel/epoch 超时闸）。
//! - `trap`：故意 panic（trap 归约路径）。
//! - `secret_probe`：secret-get 取指定名（缺 api_key；只回形态不回原文）。
//! - `ws_read`：workspace-read 读工作区相对路径（args.path）。
//! - `mem_bomb`：1MB 块持续泄漏（ResourceLimiter 内存上限应拒 grow → trap）。
//! - `host_flood`：循环 config-get 直到帧预算耗尽（BudgetExceeded 回报次数）。
//!
//! 注意：args 解析是夹具级的子串匹配（不做通用 JSON 解析），三方插件
//! 请用 serde_json 之类正经实现。

wit_bindgen::generate!({
    path: "wit",
    world: "plugin-tool",
});

use exports::nemesis::plugin::tool::{Guest, ToolInput, ToolMetadata, ToolOutput};
use nemesis::plugin::host::{self, HostError, HttpRequest, LogLevel};

struct TranslatePlugin;

/// 夹具级 JSON 字符串字段提取（`"key":"value"` 子串匹配；无转义处理）。
fn extract_str(args_json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = args_json.find(&needle)? + needle.len();
    let rest = &args_json[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

impl Guest for TranslatePlugin {
    fn get_metadata() -> Result<ToolMetadata, HostError> {
        Ok(ToolMetadata {
            name: "translate".to_string(),
            title: "Translate".to_string(),
            description: "示例工具：回显与宿主能力自检（mode 切换行为）".to_string(),
            parameters_json: r#"{
  "type": "object",
  "properties": {
    "mode": { "type": "string", "description": "echo|host_calls|egress_probe|fuel_bomb|trap|secret_probe|ws_read|mem_bomb|host_flood" },
    "text": { "type": "string", "description": "echo 模式回显文本" },
    "name": { "type": "string", "description": "secret_probe 目标名" },
    "path": { "type": "string", "description": "ws_read 工作区相对路径" }
  },
  "required": ["mode"]
}"#
            .to_string(),
            operation_type: "read".to_string(),
        })
    }

    fn execute(input: ToolInput) -> Result<ToolOutput, HostError> {
        let mode = extract_str(&input.args_json, "mode").unwrap_or_default();
        match mode.as_str() {
            "echo" => {
                let text = extract_str(&input.args_json, "text").unwrap_or_default();
                Ok(ToolOutput {
                    content: text,
                    is_error: false,
                })
            }
            "host_calls" => {
                // 全宿主能力面自检：每通道一次，结果拼摘要。
                host::log(LogLevel::Info, "host_calls begin");
                host::log(LogLevel::Warn, "host_calls warn-line");
                let now = host::now_millis();
                let greeting = host::config_get("greeting");
                let data_dir = host::data_dir_path()?;
                // /data 落盘验证 preopen 可写。
                let spill = format!("{data_dir}/spill.txt");
                let write_note = match std::fs::write(&spill, b"spill-ok") {
                    Ok(()) => "data-write=ok".to_string(),
                    Err(e) => format!("data-write=failed({e})"),
                };
                // tool-invoke：独立宿主运行时（无 invoker 接线）应 unavailable。
                let invoke = host::tool_invoke("exec", r#"{"cmd":"echo hi"}"#);
                let invoke_note = match invoke {
                    Ok(_) => "tool-invoke=unexpected-ok".to_string(),
                    Err(HostError::Unavailable(_)) => "tool-invoke=unavailable".to_string(),
                    Err(other) => format!("tool-invoke=other({other:?})"),
                };
                let greeting_note = match greeting {
                    Ok(Some(v)) => format!("greeting={v}"),
                    Ok(None) => "greeting=<unset>".to_string(),
                    Err(e) => format!("greeting=err({e:?})"),
                };
                host::log(LogLevel::Error, "host_calls end");
                Ok(ToolOutput {
                    content: format!(
                        "now={now} {greeting_note} {write_note} {invoke_note} workspace={}",
                        input.context.workspace_root
                    ),
                    is_error: false,
                })
            }
            "egress_probe" => {
                let resp = host::http_send(&HttpRequest {
                    method: "GET".to_string(),
                    url: "https://example.com/".to_string(),
                    headers: vec![],
                    body: None,
                });
                match resp {
                    Ok(r) => Ok(ToolOutput {
                        content: format!("egress status={}", r.status),
                        is_error: false,
                    }),
                    Err(e) => Ok(ToolOutput {
                        content: format!("egress blocked: {e:?}"),
                        is_error: true,
                    }),
                }
            }
            "fuel_bomb" => {
                // 故意死循环：fuel/epoch 闸应掐死（宿主归约 Timeout）。
                // black_box 防优化器删除无副作用循环（Rust 允许把无副作用
                // 循环视作可终止并整体优化掉）。
                let mut x: u64 = 0;
                loop {
                    x = x.wrapping_add(1);
                    std::hint::black_box(x);
                }
            }
            "trap" => {
                panic!("deliberate trap fixture");
            }
            "secret_probe" => {
                let name =
                    extract_str(&input.args_json, "name").unwrap_or_else(|| "api_key".to_string());
                match host::secret_get(&name) {
                    // 审计/对抗夹具：只回形态不回原文。
                    Ok(v) => Ok(ToolOutput {
                        content: format!("secret={}:resolved(len={})", name, v.len()),
                        is_error: false,
                    }),
                    Err(e) => Ok(ToolOutput {
                        content: format!("secret={}:denied({e:?})", name),
                        is_error: false,
                    }),
                }
            }
            "ws_read" => {
                let path = extract_str(&input.args_json, "path").unwrap_or_default();
                match host::workspace_read(&path) {
                    Ok(bytes) => Ok(ToolOutput {
                        content: format!("ws-read={}:{}", path, String::from_utf8_lossy(&bytes)),
                        is_error: false,
                    }),
                    Err(e) => Ok(ToolOutput {
                        content: format!("ws-read={}:denied({e:?})", path),
                        is_error: false,
                    }),
                }
            }
            "mem_bomb" => {
                // 内存炸弹：1MB 块持续泄漏。宿主 ResourceLimiter 内存上限应
                // 拒绝 memory.grow → guest 分配失败 → trap（宿主归约 Trap）。
                // fuel 闸不应先到：memset 走 bulk 指令，fuel 消耗远低于内存
                // 上限触顶所需迭代数（manifest 侧把 memory-bytes 收紧到位）。
                let mut keep: Vec<Vec<u8>> = Vec::new();
                loop {
                    let chunk = vec![0u8; 1_048_576];
                    keep.push(std::hint::black_box(chunk));
                    std::hint::black_box(&keep);
                }
            }
            "host_flood" => {
                // 帧预算对抗：循环 config-get（每调用消耗 1 帧预算）直到
                // BudgetExceeded，回报实际调用次数。budget 在声明校验前消耗，
                // 未声明键走 NotFound 计数即可。
                let mut calls: u32 = 0;
                loop {
                    match host::config_get("adversarial-nonexistent-key") {
                        Err(HostError::BudgetExceeded(_)) => {
                            return Ok(ToolOutput {
                                content: format!("budget-exhausted-after={calls}"),
                                is_error: false,
                            });
                        }
                        Err(HostError::NotFound(_)) => calls = calls.wrapping_add(1),
                        Err(other) => {
                            return Ok(ToolOutput {
                                content: format!("unexpected({other:?}) after={calls}"),
                                is_error: true,
                            });
                        }
                        Ok(_) => calls = calls.wrapping_add(1),
                    }
                }
            }
            other => Ok(ToolOutput {
                content: format!("unknown mode: {other}"),
                is_error: true,
            }),
        }
    }
}

export!(TranslatePlugin);
