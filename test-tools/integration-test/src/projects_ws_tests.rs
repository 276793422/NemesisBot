//! L6++ G4（2026-09-08）— projects WSAPI 真进程链路套件。
//!
//! crate 级单测（nemesis-web projects_tests / nemesisbot manager 测试）已
//! 钉住各命令臂与 resolve 两分支；本套件补 **真 gateway 进程** 的端到端
//! 断言（goal G4 验收门）：
//!   create → list 带 project 字段 → sessions.create 绑定 → chat.send
//!   （TestAIServer marker）→ 回复落同一 sid（jsonl 落盘 + projectId
//!   回填）→ remove → list 不再含；移除后向仍绑定的会话发消息 → 诚实
//!   出站错误（不静默丢）。
//!
//! 运行位置：main.rs 末位（UI batch series 之后）——本套件不动 config，
//! 但沿「改 config 的套件末位」纪律排尾，避免任何顺序耦合。
//! 前置：gateway 装配了 ProjectsBridge（gateway 模式恒装配）+ 项目 loop
//! 的 LLM 与主 loop 同源（TestAIServer testai-1.1）。

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use test_harness::*;
use tokio_tungstenite::tungstenite::Message;

use crate::ui_batch_series::WsApi;

/// 消息协议 chat.send（带 session_id）的一种结局。
enum ChatOutcome {
    /// chat/receive 正常回复。
    Reply(String),
    /// system/error（不可路由诚实报错等）。
    Error(String),
}

/// 裸 WS 连接上发一条带 session_id 的 chat.send，等回复或错误帧。
/// 与 test-harness `ws_send_and_recv` 同协议，但显式会话寻址 + 把
/// system/error 当作**一等结局**返回（L6++ 不可路由诚实报错断言用）。
async fn ws_chat_with_session(
    content: &str,
    sid: &str,
    timeout_secs: u64,
) -> Result<ChatOutcome, String> {
    let mut stream = ws_connect(WS_PORT, AUTH_TOKEN)
        .await
        .map_err(|e| e.to_string())?;
    let msg = json!({
        "type": "message",
        "module": "chat",
        "cmd": "send",
        "data": { "content": content, "session_id": sid },
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string(),
    });
    stream
        .send(Message::Text(msg.to_string().into()))
        .await
        .map_err(|e| format!("ws send failed: {e}"))?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let Ok(v) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                let t = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                let m = v.get("module").and_then(|m| m.as_str()).unwrap_or("");
                let c = v.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
                if t == "message" && m == "chat" && c == "receive" {
                    return Ok(ChatOutcome::Reply(
                        v["data"]["content"].as_str().unwrap_or("").to_string(),
                    ));
                }
                if t == "system" && m == "error" {
                    return Ok(ChatOutcome::Error(
                        v["data"]["content"]
                            .as_str()
                            .unwrap_or("unknown error")
                            .to_string(),
                    ));
                }
                // push / 其他帧：跳过继续等。
            }
            Ok(Some(Ok(_))) => continue, // ping/pong/binary
            Ok(Some(Err(e))) => return Err(format!("ws error: {e}")),
            Ok(None) => return Err("ws closed".into()),
            Err(_) => return Err(format!("timeout ({timeout_secs}s) waiting chat outcome")),
        }
    }
}

/// 项目目录：home 下、workspace 外（registry 重叠拒绝 workspace 子目录）。
fn project_dir(ws: &TestWorkspace, tag: &str) -> std::path::PathBuf {
    let dir = ws.path().join(format!("itproj_{tag}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---------------------------------------------------------------------------
// 套件 1：CRUD + 绑定生命周期（create → list 字段 → sessions.create →
// remove → list 不再含 → 未知 id 诚实报错）
// ---------------------------------------------------------------------------

pub async fn test_projects_crud_lifecycle(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "projects/crud_lifecycle";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };
    let proj_dir = project_dir(ws, "crud");

    // 1. create：返回 project 投影。
    let (dat, err) = api
        .call(
            "projects",
            "create",
            Some(json!({ "name": "IT项目CRUD", "path": proj_dir.to_string_lossy() })),
        )
        .await;
    let Some(dat) = dat else {
        results.push(fail(
            &format!("{suite}/create"),
            format!("create failed: {:?} / {err:?}", err.clone()),
        ));
        return results;
    };
    let Some(pid) = dat
        .pointer("/project/id")
        .and_then(|v| v.as_str())
        .map(String::from)
    else {
        results.push(fail(&format!("{suite}/create"), format!("no project.id: {dat}")));
        return results;
    };
    if pid.starts_with("p-") {
        results.push(pass(&format!("{suite}/create"), format!("id={pid}")));
    } else {
        results.push(fail(&format!("{suite}/create"), format!("unexpected id shape: {pid}")));
    }

    // 2. list：带 project 字段（id/name/path/created_at/running）。
    let (dat, err) = api.call("projects", "list", None).await;
    let found = dat
        .as_ref()
        .and_then(|d| d.get("projects"))
        .and_then(|p| p.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|p| p.get("id").and_then(|v| v.as_str()) == Some(pid.as_str()))
        })
        .cloned();
    match found {
        Some(p) => {
            let fields_ok = p.get("name").and_then(|v| v.as_str()) == Some("IT项目CRUD")
                && p.get("path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.contains("itproj_crud"))
                    .unwrap_or(false)
                && p.get("created_at").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty())
                && p.get("running").and_then(|v| v.as_bool()) == Some(true);
            if fields_ok {
                results.push(pass(&format!("{suite}/list_fields"), "all project fields present"));
            } else {
                results.push(fail(&format!("{suite}/list_fields"), format!("fields wrong: {p}")));
            }
        }
        None => {
            results.push(fail(
                &format!("{suite}/list_fields"),
                format!("project {pid} missing from list ({err:?})"),
            ));
        }
    }

    // 3. sessions.create 绑定项目（bridge 校验 + 烧归属）。
    let (dat, err) = api
        .call("sessions", "create", Some(json!({ "project_id": pid })))
        .await;
    let Some(sid) = dat
        .as_ref()
        .and_then(|d| d.get("session_id"))
        .and_then(|v| v.as_str())
        .map(String::from)
    else {
        results.push(fail(
            &format!("{suite}/create_session"),
            format!("sessions.create failed: {err:?}"),
        ));
        return results;
    };
    results.push(pass(&format!("{suite}/create_session"), format!("sid={sid}")));

    // 3b. 未知 project_id 的 sessions.create 必须诚实报错。
    let (_, err) = api
        .call("sessions", "create", Some(json!({ "project_id": "p_nope000" })))
        .await;
    match err {
        Some(e) if e.contains("不存在") => {
            results.push(pass(&format!("{suite}/create_session_unknown"), "honest error"));
        }
        other => {
            results.push(fail(
                &format!("{suite}/create_session_unknown"),
                format!("expected honest error, got {other:?}"),
            ));
        }
    }

    // 4. remove：返回被移除条目 + 诚实 note。
    let (dat, err) = api
        .call("projects", "remove", Some(json!({ "project_id": pid })))
        .await;
    match dat {
        Some(d) => {
            let ok = d.pointer("/removed/id").and_then(|v| v.as_str()) == Some(pid.as_str())
                && d.get("note")
                    .and_then(|v| v.as_str())
                    .is_some_and(|n| n.contains("未删除"));
            if ok {
                results.push(pass(&format!("{suite}/remove"), "removed + honest note"));
            } else {
                results.push(fail(&format!("{suite}/remove"), format!("shape wrong: {d}")));
            }
        }
        None => {
            results.push(fail(
                &format!("{suite}/remove"),
                format!("remove failed: {err:?}"),
            ));
        }
    }

    // 5. list 不再含。
    let (dat, _) = api.call("projects", "list", None).await;
    let still_there = dat
        .as_ref()
        .and_then(|d| d.get("projects"))
        .and_then(|p| p.as_array())
        .map(|arr| arr.iter().any(|p| p.get("id").and_then(|v| v.as_str()) == Some(pid.as_str())))
        .unwrap_or(true);
    if still_there {
        results.push(fail(&format!("{suite}/list_after_remove"), format!("{pid} still listed")));
    } else {
        results.push(pass(&format!("{suite}/list_after_remove"), "gone from list"));
    }

    // 6. 对已移除 id 再 remove → 诚实报错。
    let (_, err) = api
        .call("projects", "remove", Some(json!({ "project_id": pid })))
        .await;
    match err {
        Some(e) if e.contains("不存在") => {
            results.push(pass(&format!("{suite}/remove_missing"), "honest error"));
        }
        other => {
            results.push(fail(
                &format!("{suite}/remove_missing"),
                format!("expected honest error, got {other:?}"),
            ));
        }
    }

    results
}

// ---------------------------------------------------------------------------
// 套件 2：项目会话对话回流（chat.send marker → 回复 → jsonl 落同一 sid +
// meta 绑定幸存 + session_list projectId 回填）
// ---------------------------------------------------------------------------

pub async fn test_projects_session_chat_roundtrip(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "projects/session_chat_roundtrip";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };
    let proj_dir = project_dir(ws, "chat");

    // 建项目 + 绑定会话。
    let (dat, err) = api
        .call(
            "projects",
            "create",
            Some(json!({ "name": "IT项目对话", "path": proj_dir.to_string_lossy() })),
        )
        .await;
    let Some(pid) = dat.and_then(|d| {
        d.pointer("/project/id").and_then(|v| v.as_str()).map(String::from)
    }) else {
        results.push(fail(
            &format!("{suite}/setup"),
            format!("projects.create failed: {err:?}"),
        ));
        return results;
    };
    let (dat, err) = api
        .call("sessions", "create", Some(json!({ "project_id": pid })))
        .await;
    let Some(sid) = dat
        .as_ref()
        .and_then(|d| d.get("session_id"))
        .and_then(|v| v.as_str())
        .map(String::from)
    else {
        results.push(fail(
            &format!("{suite}/setup"),
            format!("sessions.create failed: {err:?}"),
        ));
        return results;
    };

    // chat.send（TestAIServer marker）→ 等回复。
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let marker = format!("ITPROJ_MARKER_{nanos}");
    match ws_chat_with_session(&format!("{marker} hello project"), &sid, 90).await {
        Ok(ChatOutcome::Reply(content)) => {
            if content.is_empty() {
                results.push(fail(&format!("{suite}/reply"), "empty reply"));
            } else {
                results.push(pass(
                    &format!("{suite}/reply"),
                    format!("{} bytes via project loop", content.len()),
                ));
            }
        }
        Ok(ChatOutcome::Error(e)) => {
            results.push(fail(&format!("{suite}/reply"), format!("error frame: {e}")));
        }
        Err(e) => {
            results.push(fail(&format!("{suite}/reply"), e));
            return results;
        }
    }

    // 回复落同一 sid：jsonl 里要有 marker（user 行落盘在项目会话键下）。
    let safe = sanitize_ws_sid(&sid);
    let jsonl = ws
        .workspace()
        .join("logs")
        .join("session_logs")
        .join(format!("agent_main_session_{safe}.jsonl"));
    match std::fs::read_to_string(&jsonl) {
        Ok(body) if body.contains(&marker) => {
            results.push(pass(&format!("{suite}/jsonl_sid"), "marker in project session jsonl"));
        }
        Ok(body) => {
            results.push(fail(
                &format!("{suite}/jsonl_sid"),
                format!("jsonl exists but marker missing ({} bytes)", body.len()),
            ));
        }
        Err(e) => {
            results.push(fail(
                &format!("{suite}/jsonl_sid"),
                format!("jsonl not found at {}: {e}", jsonl.display()),
            ));
        }
    }

    // 绑定幸存：meta sidecar 的 project_id 仍在。
    let meta = jsonl.with_extension("meta.json");
    let meta_ok = std::fs::read_to_string(&meta)
        .ok()
        .and_then(|d| serde_json::from_str::<Value>(&d).ok())
        .and_then(|m| {
            m.get("project_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .is_some_and(|v| v == pid);
    if meta_ok {
        results.push(pass(&format!("{suite}/binding_survives"), "meta project_id intact"));
    } else {
        results.push(fail(
            &format!("{suite}/binding_survives"),
            format!("meta project_id missing/changed: {}", meta.display()),
        ));
    }

    // session_list 回填 projectId（logs.rs L6++ 回填）。
    let (dat, _) = api.call("logs", "session_list", Some(json!({ "limit": 200 }))).await;
    let entry = dat
        .as_ref()
        .and_then(|d| d.get("sessions"))
        .and_then(|s| s.as_array())
        .and_then(|arr| {
            arr.iter().find(|s| {
                s.get("id").and_then(|v| v.as_str()) == Some(format!("agent_main_session_{safe}").as_str())
            })
        })
        .cloned();
    match entry {
        Some(e) => {
            if e.get("projectId").and_then(|v| v.as_str()) == Some(pid.as_str()) {
                results.push(pass(&format!("{suite}/session_list_backfill"), "projectId present"));
            } else {
                results.push(fail(
                    &format!("{suite}/session_list_backfill"),
                    format!("projectId missing/wrong: {e}"),
                ));
            }
        }
        None => {
            results.push(fail(
                &format!("{suite}/session_list_backfill"),
                "session not in session_list after chat",
            ));
        }
    }

    results
}

/// 镜像 sanitize_session_id（uuid sid 只含 [-0-9a-f]，恒等即可，但显式
/// 走同一规则防未来 sid 形态变化）。
fn sanitize_ws_sid(sid: &str) -> String {
    sid.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 套件 3：移除项目后向仍绑定的会话发消息 → 诚实出站错误（不静默丢）
// ---------------------------------------------------------------------------

pub async fn test_projects_unroutable_after_remove(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "projects/unroutable_after_remove";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };
    let proj_dir = project_dir(ws, "gone");

    // 建项目 + 绑定会话 + 移除项目（sidecar 保留 = 孤儿语义）。
    let (dat, err) = api
        .call(
            "projects",
            "create",
            Some(json!({ "name": "IT项目移除", "path": proj_dir.to_string_lossy() })),
        )
        .await;
    let Some(pid) = dat.and_then(|d| {
        d.pointer("/project/id").and_then(|v| v.as_str()).map(String::from)
    }) else {
        results.push(fail(
            &format!("{suite}/setup"),
            format!("projects.create failed: {err:?}"),
        ));
        return results;
    };
    let (dat, err) = api
        .call("sessions", "create", Some(json!({ "project_id": pid })))
        .await;
    let Some(sid) = dat
        .as_ref()
        .and_then(|d| d.get("session_id"))
        .and_then(|v| v.as_str())
        .map(String::from)
    else {
        results.push(fail(
            &format!("{suite}/setup"),
            format!("sessions.create failed: {err:?}"),
        ));
        return results;
    };
    let (dat, err) = api
        .call("projects", "remove", Some(json!({ "project_id": pid })))
        .await;
    if dat.is_none() {
        results.push(fail(
            &format!("{suite}/setup"),
            format!("projects.remove failed: {err:?}"),
        ));
        return results;
    }

    // 向仍绑定的会话发消息 → 诚实报错（不静默丢）。传输形态按设计走
    // bus 出站链路 = chat/receive 帧（manager.rs `send_unroutable_error`
    // 的注释：前端按现状错误消息显示），system/error 控制帧不是本路径
    // 的载体——两种帧都收，内容必须含「不可用」；真回复 = silent mis-route。
    match ws_chat_with_session("ITGONE_MARKER after remove", &sid, 60).await {
        Ok(ChatOutcome::Error(e)) => {
            if e.contains("不可用") {
                results.push(pass(
                    &format!("{suite}/honest_error"),
                    format!("honest unroutable error: {e}"),
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/honest_error"),
                    format!("error frame but unexpected text: {e}"),
                ));
            }
        }
        Ok(ChatOutcome::Reply(c)) => {
            if c.contains("不可用") {
                results.push(pass(
                    &format!("{suite}/honest_error"),
                    format!("honest unroutable via outbound chat: {c}"),
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/honest_error"),
                    format!("got a normal reply from a removed project (silent mis-route?): {c}"),
                ));
            }
        }
        Err(e) => {
            results.push(fail(
                &format!("{suite}/honest_error"),
                format!("neither reply nor error within timeout: {e}"),
            ));
        }
    }

    results
}
