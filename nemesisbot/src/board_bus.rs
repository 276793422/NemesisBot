//! Swarm M3：master 侧 `nb_bus` 装配（board 信封协议的 gateway 侧 glue）。
//!
//! 职责（impl-plan §5.1-§5.3）：RPC 层只注册一个 [`nemesis_cluster::envelope::NB_BUS_ACTION`]
//! action，信封内 `ns/op` 路由——加功能不加 action。
//!
//! - `board.comment.post`（worker 上行发言）：幂等预检 → 额度三闸 → 落库
//!   （G12 幂等 + seq 台账）→ 立即返回首响，裁决器 + wake 下行在 tokio
//!   任务里异步跑（handler 不等投递）。
//! - `board.sync`（worker 上线补拉）：`since_seq` → 台账增量。
//! - wake 下行（master → worker）：裁决器产出目标 → 组装 §5.2② 唤醒包
//!   （线程上下文 20 条 + reply_hint.max_turns_left + seq）→ 逐个 RPC 投递；
//!   离线/额度耗尽跳过（board.sync 兜底 / 推人）。
//! - 规则③（频道无 @ → 主持人）：master 本地 agent 直调裁决（[SILENT] 或
//!   @某人，回复照常落库 + 再走裁决器定点唤醒）。额度是死循环熔断闸：
//!   线程额度耗尽后 worker 发言被拒、主持人不再投递——链路自然收敛。
//!
//! estop：主持人裁决走主 AgentLoop（`process_direct`），estop 冻结即停。
#![cfg(all(feature = "board", feature = "cluster"))]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use nemesis_board::Actor;
use nemesis_board::arbitrator::{
    NodeCandidate, WakeInput, WakePlan, has_mentions, mentions_node, resolve_wake_targets,
};
use nemesis_board::models::PostedMessage;
use nemesis_board::models::thread_kind;
use nemesis_board::quota::{QuotaDenied, QuotaLedger};
use nemesis_cluster::envelope::{self, Envelope, EnvelopeError, EnvelopeResponse};
use nemesis_cluster::rpc_types::{ActionType, RPCRequest};
use nemesis_types::cluster::{DiscussionCtxMessage, DiscussionEvent};

/// 上行 handler 组装依赖（gateway 装配点注入）。
pub struct MasterBusDeps {
    /// 看板存储（master 单写者权威）。
    pub store: Arc<nemesis_board::BoardStore>,
    /// 讨论额度台账（进程内存态）。
    pub quota: Arc<QuotaLedger>,
    /// 集群编排（节点表投影 + RPC 下行）。
    pub cluster: Arc<nemesis_cluster::cluster::Cluster>,
    /// 主 AgentLoop 后置装配桥：nb_bus 注册时 agent_loop 尚未构建
    /// （gateway 装配顺序），构建完成后 `set()` 填入；主持人裁决经它直调。
    pub moderator_loop: Arc<OnceLock<Arc<nemesis_agent::r#loop::AgentLoop>>>,
    /// G（goal P3）：master workspace 根——交付回传文件的落盘父目录
    /// （`board/files/issue_<id>/`，与附件通道同目录约定）。
    pub workspace: std::path::PathBuf,
    /// 交付落盘时间戳源（`{ms}_{safe_name}` 命名前缀）。缺省墙钟；测试
    /// 注入固定值使「预埋同名目录→写必失败」成为确定性事件。2026-10-10
    /// run170 s1 假红根治：旧的静态 8s 时间窗赌处理器取点落窗内，慢机上
    /// 布阵+前置开销吃穿窗沿（实录该测试墙钟 20.4s）；布阵线程又追不上
    /// 爬行时钟——一切铺窗策略治不了这个类，时间可注入才能根治。
    pub now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
}

/// 线程上下文随 wake 包下发的条数（impl-plan §5.2②：默认 20）。
const WAKE_CONTEXT_MESSAGES: usize = 20;

/// 注册 master 侧 `nb_bus` handler（gateway 在 cluster Arc 化之后调用）。
pub fn build_master_nb_bus_handler(
    deps: MasterBusDeps,
) -> nemesis_cluster::rpc::server::RpcHandlerFn {
    Box::new(move |payload| handle_nb_bus(&deps, payload))
}

/// 信封路由 + 三 op 处理（同步部分；异步投递 spawn）。
fn handle_nb_bus(
    deps: &MasterBusDeps,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    // 解析失败也要带回路由字段（corr_id 透传是协议礼貌）。
    let env = match envelope::parse_envelope(&payload) {
        Ok(e) => e,
        Err(err) => {
            let fallback = fallback_envelope(&payload);
            return Ok(EnvelopeResponse::failure(&fallback, err).to_json());
        }
    };

    // D0b（goal P2）：ns="task" 路由——worker 出队开跑上报（queued vs
    // executing 分界，重平衡只挪 queued 单）。
    if env.ns == "task" {
        let reply = match env.op.as_str() {
            "started" => handle_task_started(deps, &env, rpc_from_node(&payload)),
            "delivery.files" => handle_delivery_files(deps, &env, rpc_from_node(&payload)),
            other => EnvelopeResponse::failure(
                &env,
                EnvelopeError::new(
                    envelope::error_code::UNKNOWN_OP,
                    format!("unknown op: {other}"),
                ),
            ),
        };
        return Ok(reply.to_json());
    }

    // M5（2026-10-10 集群无人值守硬化）：ns="approval"——worker ask 上行
    // 登记/摘除（跨节点批复桥的 master 侧数据源；注册表与 SSE 发布在
    // nemesis_web::remote_approval，board_bus 只做信封路由）。
    if env.ns == nemesis_web::remote_approval::NS_APPROVAL {
        let reply = match env.op.as_str() {
            "ask_register" => handle_approval_ask_register(deps, &env, rpc_from_node(&payload)),
            "ask_closed" => handle_approval_ask_closed(&env),
            other => EnvelopeResponse::failure(
                &env,
                EnvelopeError::new(
                    envelope::error_code::UNKNOWN_OP,
                    format!("unknown op: {other}"),
                ),
            ),
        };
        return Ok(reply.to_json());
    }

    if env.ns != "board" {
        return Ok(EnvelopeResponse::failure(
            &env,
            EnvelopeError::new(
                envelope::error_code::UNKNOWN_NS,
                format!("unknown ns: {} (want board)", env.ns),
            ),
        )
        .to_json());
    }

    let reply = match env.op.as_str() {
        "comment.post" => handle_comment_post(deps, &env),
        "comment.list" => handle_comment_list(deps, &env),
        "sync" => handle_sync(deps, &env),
        "claimable.list" => handle_claimable_list(deps, &env, rpc_from_node(&payload)),
        "claim" => handle_claim(deps, &env, rpc_from_node(&payload)),
        other => EnvelopeResponse::failure(
            &env,
            EnvelopeError::new(
                envelope::error_code::UNKNOWN_OP,
                format!("unknown op: {other}"),
            ),
        ),
    };
    Ok(reply.to_json())
}

/// 提取 body 字符串字段（缺键/非串 = 空串——展示字段容忍缺省）。
fn str_field(body: &serde_json::Value, key: &str) -> String {
    body.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// M5：worker ask 上行登记（ns="approval" op="ask_register"）。`_rpc.from`
/// 即 worker 身份（RPC server 注入，防伪造）；登记进远程审批注册表并广播
/// SSE `approval-remote-requested`（master Dashboard 卡片数据源）。同
/// request_id 重登 = 覆盖（防御性幂等——裁决真相源仍在 worker 的 pending
/// 表，先到先得）。
///
/// §2 免打扰（无人值守计划 Part 2）：开关开 = 不登记不广播不弹卡，直接
/// spawn 下发 `approved=true` 给 worker（send_verdict 失败只 warn——worker
/// 侧照常按 300s 超时拒绝，无人值守语义退化为「暂缓」而非卡死 master）。
fn handle_approval_ask_register(
    deps: &MasterBusDeps,
    env: &Envelope,
    from_node: &str,
) -> EnvelopeResponse {
    if from_node.is_empty() {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, "missing _rpc.from"),
        );
    }
    let Some(request_id) = env
        .body
        .get("request_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
    else {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, "missing request_id"),
        );
    };
    if nemesis_web::handlers::security::approval_auto_state().is_some_and(|s| s.enabled()) {
        tracing::info!(
            target: "board_bus",
            worker = %from_node,
            request_id = %request_id,
            "[nb_bus] remote approval auto-approved (unattended mode), forwarding verdict"
        );
        let cluster = deps.cluster.clone();
        let worker = from_node.to_string();
        let rid = request_id.clone();
        // nb_bus handler 跑在 RPC 派发的阻塞线程里——不在 runtime 上下文时
        // 跳过 spawn（worker 超时兜底；与 send_ask_envelope 同款守卫）。
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                match nemesis_web::remote_approval::send_verdict(&cluster, &worker, &rid, true, None).await {
                    Ok(()) => tracing::info!(target: "board_bus", request_id = %rid,
                        worker = %worker, "[nb_bus] auto-approve verdict forwarded"),
                    Err(e) => tracing::warn!(target: "board_bus", request_id = %rid,
                        worker = %worker, "[nb_bus] auto-approve verdict forward failed: {e} (worker resolves by timeout)"),
                }
            });
        }
        return EnvelopeResponse::success(
            env,
            serde_json::json!({ "registered": false, "auto_approved": true }),
        );
    }
    let entry = nemesis_web::remote_approval::RemoteAskEntry {
        request_id: request_id.clone(),
        worker_node: from_node.to_string(),
        operation: str_field(&env.body, "operation"),
        target: str_field(&env.body, "target"),
        risk_level: str_field(&env.body, "risk_level"),
        reason: str_field(&env.body, "reason"),
        issue_number: env.body.get("issue_number").and_then(|v| v.as_i64()),
        timeout_secs: env
            .body
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(300),
        created_at: std::time::Instant::now(),
    };
    let short_id = nemesis_web::remote_approval::registry().register(entry.clone());
    tracing::info!(
        target: "board_bus",
        worker = %from_node,
        request_id = %request_id,
        short_id = %short_id,
        "[nb_bus] remote approval registered"
    );
    nemesis_web::remote_approval::publish_requested(nemesis_web::remote_approval::entry_json(
        &short_id, &entry,
    ));
    EnvelopeResponse::success(
        env,
        serde_json::json!({ "registered": true, "short_id": short_id }),
    )
}

/// M5：worker ask 摘除上行（ns="approval" op="ask_closed"）——本节点裁决
/// 生效（本地 Dashboard 批复）后通知 master 同步关卡。幂等：条目不存在 =
/// success {closed:false}（重复上行不报错；超时自动拒在 worker 侧不产生
/// 本上行——注册表 TTL 到期自然过期）。
fn handle_approval_ask_closed(env: &Envelope) -> EnvelopeResponse {
    let Some(request_id) = env
        .body
        .get("request_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
    else {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, "missing request_id"),
        );
    };
    let approved = env
        .body
        .get("approved")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let removed = nemesis_web::remote_approval::registry().remove_by_request_id(&request_id);
    let closed = removed.is_some();
    tracing::info!(
        target: "board_bus",
        request_id = %request_id,
        approved,
        closed,
        "[nb_bus] remote approval closed"
    );
    if let Some(short_id) = removed {
        nemesis_web::remote_approval::publish_resolved(serde_json::json!({
            "request_id": request_id,
            "short_id": short_id,
            "decision": if approved { "approved" } else { "denied" },
            "source": "worker",
        }));
    }
    EnvelopeResponse::success(env, serde_json::json!({ "closed": closed }))
}

/// D0b：worker 出队开跑上报（ns="task" op="started"）。`_rpc.from` 必须与
/// 派发 worker_id 一致（防伪造）；非 dispatched 态 = 诚实 failure（重复
/// 上报/已终态）。queued vs executing 的分界由此确立。
fn handle_task_started(deps: &MasterBusDeps, env: &Envelope, from_node: &str) -> EnvelopeResponse {
    let Some(task_id) = env
        .body
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(String::from)
    else {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, "missing task_id"),
        );
    };
    if from_node.is_empty() {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(
                envelope::error_code::VALIDATION,
                "missing _rpc.from (sender identity)",
            ),
        );
    }
    match deps.store.mark_dispatch_running(&task_id, from_node) {
        Ok(true) => {
            tracing::debug!(target: "board_bus", task_id = %task_id, worker = %from_node,
                "[nb_bus] task started (dispatch → running)");
            EnvelopeResponse::success(env, serde_json::json!({ "running": true }))
        }
        Ok(false) => {
            // claim 竞速（T43 实证）：dispatch_issue_core 的派发行在
            // submit ACK 之后才落库（board.rs try_claim_dispatch 在
            // submit_peer_chat 之后），而 worker 空闲即取即跑——started
            // 上报可跑赢插行，此刻任务**正在执行**。行不存在 = 可重试
            // （UNAVAILABLE，worker 退避重报）；行已存在但状态/worker
            // 不符才是真 VALIDATION（终态/重复上报/伪造，不重试）。
            match deps.store.get_dispatch(&task_id) {
                Ok(None) => EnvelopeResponse::failure(
                    env,
                    EnvelopeError::new(
                        envelope::error_code::UNAVAILABLE,
                        format!("task {task_id} 的派发行尚未落库（claim 竞速），请重试"),
                    ),
                ),
                Ok(Some(_)) => EnvelopeResponse::failure(
                    env,
                    EnvelopeError::new(
                        envelope::error_code::VALIDATION,
                        format!("task {task_id} 不在 dispatched 态或 worker 不匹配"),
                    ),
                ),
                Err(e) => EnvelopeResponse::failure(
                    env,
                    EnvelopeError::new(envelope::error_code::INTERNAL, e),
                ),
            }
        }
        Err(e) => {
            EnvelopeResponse::failure(env, EnvelopeError::new(envelope::error_code::INTERNAL, e))
        }
    }
}

/// G（goal P3）交付文件回传（ns="task" op="delivery.files"）：worker 把
/// 交付清单里的文件（base64）推给 master，master 落盘 `board/files/issue_<id>/`
/// 并 add_attachment 登记——前端详情弹窗附件区即可下载（与 attachment.add
/// 同一落点）。
///
/// 校验：`_rpc.from` 必须与派发 worker_id 一致；单文件 ≤8MB、≤20 个；
/// 文件名取 basename 防路径穿越。落库后系统评论留痕。
fn handle_delivery_files(
    deps: &MasterBusDeps,
    env: &Envelope,
    from_node: &str,
) -> EnvelopeResponse {
    use base64::Engine as _;
    const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
    const MAX_FILES: usize = 20;

    let Some(task_id) = env
        .body
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(String::from)
    else {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, "missing task_id"),
        );
    };
    // 派发记录校验：来源 worker 与登记一致（防伪造）；顺带拿 issue_id。
    let dispatch = match deps.store.get_dispatch(&task_id) {
        Ok(Some(d)) if d.worker_id == from_node => d,
        _ => {
            return EnvelopeResponse::failure(
                env,
                EnvelopeError::new(
                    envelope::error_code::VALIDATION,
                    format!("task {task_id} 无此 worker 的在途派发（或来源不匹配）"),
                ),
            );
        }
    };
    let issue_id = dispatch.issue_id;
    let files = env
        .body
        .get("files")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if files.is_empty() {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, "files 为空"),
        );
    }
    if files.len() > MAX_FILES {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(
                envelope::error_code::VALIDATION,
                format!("files 超上限（{}/{}）", files.len(), MAX_FILES),
            ),
        );
    }

    // 逐文件解码落盘（basename 防穿越；重名毫秒戳前缀防覆盖）。
    let mut stored: Vec<String> = Vec::new();
    for f in &files {
        let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let b64 = f.get("content_b64").and_then(|v| v.as_str()).unwrap_or("");
        let bytes = match base64::engine::general_purpose::STANDARD.decode(b64.trim()) {
            Ok(b) => b,
            Err(e) => {
                return EnvelopeResponse::failure(
                    env,
                    EnvelopeError::new(
                        envelope::error_code::VALIDATION,
                        format!("文件 {name:?} base64 解码失败: {e}"),
                    ),
                );
            }
        };
        if bytes.len() > MAX_FILE_BYTES {
            return EnvelopeResponse::failure(
                env,
                EnvelopeError::new(
                    envelope::error_code::VALIDATION,
                    format!("文件 {name:?} 超过 8MB 上限"),
                ),
            );
        }
        let safe_name: String = name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("unnamed")
            .to_string();
        if safe_name.is_empty() || safe_name.starts_with('.') {
            return EnvelopeResponse::failure(
                env,
                EnvelopeError::new(envelope::error_code::VALIDATION, "非法文件名"),
            );
        }
        let dir = deps
            .workspace
            .join("board")
            .join("files")
            .join(format!("issue_{issue_id}"));
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return EnvelopeResponse::failure(
                env,
                EnvelopeError::new(envelope::error_code::INTERNAL, format!("落盘失败: {e}")),
            );
        }
        let stored_name = format!("{}_{}", (deps.now_ms)(), safe_name);
        if let Err(e) = std::fs::write(dir.join(&stored_name), &bytes) {
            return EnvelopeResponse::failure(
                env,
                EnvelopeError::new(envelope::error_code::INTERNAL, format!("写入失败: {e}")),
            );
        }
        let rel_path = format!("board/files/issue_{issue_id}/{stored_name}");
        if let Err(e) =
            deps.store
                .add_attachment(issue_id, &safe_name, &rel_path, bytes.len() as i64)
        {
            return EnvelopeResponse::failure(
                env,
                EnvelopeError::new(envelope::error_code::INTERNAL, format!("登记失败: {e}")),
            );
        }
        stored.push(safe_name);
    }

    // 留痕：系统评论（附件区可见 + 决策/活动可追溯）。
    let _ = deps.store.add_comment(nemesis_board::NewComment {
        issue_id,
        author: Actor::system("board"),
        content: format!(
            "📥 交付回传 {} 个文件（{from_node}）：{}",
            stored.len(),
            stored.join("、")
        ),
        parent_id: None,
        ctype: nemesis_board::CommentType::System,
    });

    EnvelopeResponse::success(
        env,
        serde_json::json!({ "stored": stored.len(), "files": stored }),
    )
}

/// 解析失败时的降级信封（尽力回带 ns/op/corr_id，供对端关联）。
fn fallback_envelope(payload: &serde_json::Value) -> Envelope {
    Envelope {
        ns: payload
            .get("ns")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        op: payload
            .get("op")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        corr_id: payload
            .get("corr_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        ..Envelope::default()
    }
}

/// `board.comment.post`：幂等 → 额度 → 落库 → 异步裁决投递。
fn handle_comment_post(deps: &MasterBusDeps, env: &Envelope) -> EnvelopeResponse {
    let body = &env.body;
    let client_msg_id = match envelope::client_msg_id(body) {
        Some(id) => id.to_string(),
        None => {
            return EnvelopeResponse::failure(
                env,
                EnvelopeError::new(
                    envelope::error_code::VALIDATION,
                    "missing client_msg_id (upstream idempotency key)",
                ),
            );
        }
    };
    let Some(thread_kind) = body
        .get("thread")
        .and_then(|t| t.get("kind"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
    else {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, "missing thread.kind"),
        );
    };
    let Some(thread_id) = body
        .get("thread")
        .and_then(|t| t.get("id"))
        .and_then(|v| v.as_i64())
    else {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, "missing thread.id"),
        );
    };
    let sender_type = body
        .get("sender")
        .and_then(|s| s.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("agent");
    let Some(sender_id) = body
        .get("sender")
        .and_then(|s| s.get("id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
    else {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, "missing sender.id"),
        );
    };
    let Some(content) = body
        .get("content")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
    else {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, "missing content"),
        );
    };
    let reply_to = body.get("reply_to").and_then(|v| v.as_i64());
    let kind_tag = body
        .get("kind_tag")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // D11-1（2026-10-13 第 6 轮审计）：远端 kind_tag 白名单——store 侧
    // `CommentType::from_str(kind_tag)` 对未知值回退 Comment，但
    // "system"/"status_change"/"comment" 三值**有**映射：worker 上行可自
    // 选 ctype 落成系统/状态变更/普通评论形态（讨论区伪装系统痕迹、伪造
    // 状态变更时间线）。worker 正常路径只用 discussion/text（见
    // cluster_agent build_comment_post_envelope）；question/delivery 留
    // 协议余量。dashboard 本地发言不经此路径，不受影响。
    if matches!(kind_tag.as_str(), "system" | "status_change" | "comment") {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(
                envelope::error_code::VALIDATION,
                format!("kind_tag `{kind_tag}` 不允许远端上行（保留类型）"),
            ),
        );
    }

    let sender = Actor::new(sender_type, &sender_id);
    match post_discussion_core(
        deps,
        &sender,
        &thread_kind,
        thread_id,
        &client_msg_id,
        &content,
        reply_to,
        &kind_tag,
        false,
    ) {
        Ok(p) => EnvelopeResponse::success(env, p.response),
        Err(PostError::Quota(denied)) => {
            tracing::warn!(
                target: "board_bus",
                node = %sender.id,
                thread = %format!("{thread_kind}:{thread_id}"),
                code = %denied.error_code(),
                "[nb_bus] comment.post denied by quota"
            );
            EnvelopeResponse::failure(
                env,
                EnvelopeError::new(denied.error_code(), denied.message()),
            )
        }
        Err(PostError::Store(e)) => {
            EnvelopeResponse::failure(env, EnvelopeError::new(envelope::error_code::VALIDATION, e))
        }
        Err(PostError::DedupCheck(e)) => {
            EnvelopeResponse::failure(env, EnvelopeError::new(envelope::error_code::INTERNAL, e))
        }
    }
}

/// 讨论管线错误（传输层各自映射：RPC 信封 error code / WSAPI 人读文案）。
pub enum PostError {
    /// 额度三闸拒绝。
    Quota(QuotaDenied),
    /// 落库失败。
    Store(String),
    /// 幂等预检读写失败。
    DedupCheck(String),
}

/// wake 计划 → 首响摘要（goal P1/F3）。独立纯函数：幂等命中与新落库两条
/// 路径共用同一形态——「相同首响」契约（G12）由同源生成保证。
fn wake_summary_json(plan: &WakePlan) -> serde_json::Value {
    serde_json::json!({
        "woke": plan.targets,
        "to_moderator": plan.to_moderator,
        "skipped": plan.skipped.iter()
            .map(|s| serde_json::json!({"node": s.node_id, "reason": s.reason}))
            .collect::<Vec<_>>(),
    })
}

/// 讨论管线共享核心（单一真相）：① 幂等预检（重复 → 缓存首响，不扣
/// 额度）→ ② 额度三闸（拒绝不落库——worker/用户明确知道被护栏拦下）→
/// ③ 落库（全副作用 + seq 台账）→ ④ 新消息异步裁决 + wake 下行（handler
/// 立即返回首响，投递不占 ACK）。worker RPC 上行（[`handle_comment_post`]，
/// `from_local=false`）与 dashboard 本地发言（[`post_discussion_locally`]，
/// `from_local=true`）共用，G3/G12 语义同源。
///
/// `from_local` 同时是 M5 审批回源闸（D11-2，2026-10-13 第 6 轮审计）：
/// `/approve|/deny <短号>` → 远程审批 verdict 是**人**的裁决——曾对一切
/// 来源开放，worker 经 board.comment.post 上行一条同形文本即可批复自己
/// 的待审批卡（审批闸被执行方自答击穿）。回源只走 dashboard 本地发言
/// （web 层登录鉴权）。
#[allow(clippy::too_many_arguments)]
fn post_discussion_core(
    deps: &MasterBusDeps,
    sender: &Actor,
    thread_kind: &str,
    thread_id: i64,
    client_msg_id: &str,
    content: &str,
    reply_to: Option<i64>,
    kind_tag: &str,
    from_local: bool,
) -> Result<PostedMessage, PostError> {
    // ⓪ wake 裁决前置（goal P1/F3）：裁决器是纯函数，先于落库计算——
    // 幂等命中与新落库的首响都带 wake 摘要，G12「相同首响」契约不破；
    // 异步任务只做投递 + 主持人裁决。
    let mut ctx = WakeContext {
        thread_kind: thread_kind.to_string(),
        thread_id,
        sender: sender.clone(),
        content: content.to_string(),
        reply_to,
        seq: 0, // 落库后回填（deliver_wakeups 的 wake 信封/审计需要真 seq）。
        at: chrono::Utc::now().timestamp(),
    };
    let nodes: Vec<NodeCandidate> = deps
        .cluster
        .list_nodes()
        .iter()
        .map(|n| NodeCandidate {
            id: n.base.id.clone(),
            name: n.base.name.clone(),
            role: n.base.role.as_role_str().to_string(),
            category: n.base.category.clone(),
            online: n.is_online(),
        })
        .collect();
    let issue_assignee = if ctx.thread_kind == thread_kind::ISSUE {
        deps.store
            .get_issue(ctx.thread_id)
            .ok()
            .and_then(|issue| match issue.assignee {
                Some(nemesis_board::AssignmentType::Worker) => issue.assignee_id,
                _ => None,
            })
    } else {
        None
    };
    let plan = resolve_wake_targets(
        &WakeInput {
            thread_kind: &ctx.thread_kind,
            content: &ctx.content,
            sender_id: &ctx.sender.id,
            issue_assignee: issue_assignee.as_deref(),
            moderator_id: deps.cluster.node_id(),
        },
        &nodes,
    );

    // ① 幂等预检（不扣额度）：重复请求直接返回缓存首响 + 当前 wake 摘要（G12）。
    match deps.store.check_duplicate(&sender.id, client_msg_id) {
        Ok(Some(cached)) => {
            let mut response = cached;
            if let Some(obj) = response.as_object_mut() {
                obj.insert("wake".to_string(), wake_summary_json(&plan));
            }
            return Ok(PostedMessage {
                is_new: false,
                message_id: 0,
                seq: 0,
                response,
            });
        }
        Ok(None) => {}
        Err(e) => return Err(PostError::DedupCheck(e)),
    }

    // ② 额度三闸（限速 / 小时 / 线程）。
    let thread_key = format!("{thread_kind}:{thread_id}");
    let now = chrono::Utc::now().timestamp();
    if let Err(denied) = deps.quota.try_consume_post(&thread_key, &sender.id, now) {
        tracing::warn!(
            target: "board_bus",
            node = %sender.id,
            thread = %thread_key,
            code = %denied.error_code(),
            "[nb_bus] comment.post denied by quota"
        );
        return Err(PostError::Quota(denied));
    }

    // ③ 落库（全副作用 + seq 台账；幂等窗口内的并发重复由 store 层消化）。
    let mut posted = deps
        .store
        .post_discussion_envelope(
            &sender.id,
            client_msg_id,
            thread_kind,
            thread_id,
            sender,
            content,
            reply_to,
            kind_tag,
        )
        .map_err(PostError::Store)?;
    if !posted.is_new {
        // 预检与落库之间的并发重复：额度已多扣一次（防刷语义可接受），
        // 响应仍是首响（附当前 wake 摘要，保持响应形状一致）。
        if let Some(obj) = posted.response.as_object_mut() {
            obj.insert("wake".to_string(), wake_summary_json(&plan));
        }
        return Ok(posted);
    }

    // ④ 异步投递 + 主持人裁决：plan/nodes 已在 ⓪ 裁决段算好（move 进任务），
    // wake 摘要已随首响带回前端（F3）——本任务只做投递 + 主持人裁决。
    ctx.seq = posted.seq;

    let deps_for_task = DepsForTask {
        store: deps.store.clone(),
        quota: deps.quota.clone(),
        cluster: deps.cluster.clone(),
        moderator_loop: deps.moderator_loop.clone(),
    };
    // 首响附 wake 摘要（F3）——在 spawn 前（plan 被 move 进异步任务）。
    let wake_summary = wake_summary_json(&plan);
    // panic 留痕（2026-10-11 planner 事故固化）：裁决投递链 panic 不再
    // 无声——ERROR 日志 + issue 线程系统评论（讨论线程无单可挂，只记日志）。
    // 取消（关机/estop）= 正常流不留痕。
    let handle = tokio::spawn(async move {
        deliver_wakeups(deps_for_task, ctx, plan, nodes).await;
    });
    {
        let watch_store = deps.store.clone();
        let is_issue_thread = thread_kind == thread_kind::ISSUE;
        tokio::spawn(async move {
            if let Err(je) = handle.await {
                if !je.is_panic() {
                    return; // 任务被取消（关机/estop）= 正常流
                }
                let msg = nemesis_utils::string_utils::panic_message(je.into_panic());
                let short: String = msg.chars().take(300).collect();
                tracing::error!("[BoardBus] wake 投递/裁决链 panic thread={thread_id}: {short}");
                if is_issue_thread {
                    let _ = watch_store.add_comment(nemesis_board::NewComment {
                        issue_id: thread_id,
                        author: Actor::system("board"),
                        content: format!(
                            "❌ 讨论投递链后台任务异常终止（内部错误 panic）：{short}——本轮 wake 未完成投递，请重发或人工处置"
                        ),
                        parent_id: None,
                        ctype: nemesis_board::CommentType::System,
                    });
                }
            }
        });
    }

    // M5（2026-10-10 集群无人值守硬化）：看板评论回源——issue 线程的
    // `/approve|/deny <短号>` 评论转发远程审批（短号在注册表才动作，杂散
    // 同形文本零影响；失败留痕为系统评论）。与 deliver_wakeups 并行的旁路
    // spawn，不阻塞首响。D11-2：只认本地发言（见 fn doc）。
    if from_local
        && thread_kind == thread_kind::ISSUE
        && nemesis_web::remote_approval::looks_like_approval_reply(content)
    {
        let store = deps.store.clone();
        let cluster = deps.cluster.clone();
        let content = content.to_string();
        tokio::spawn(async move {
            nemesis_web::remote_approval::try_remote_approval_reply(
                &store, &cluster, thread_id, &content,
            )
            .await;
        });
    }

    let mut response = posted.response;
    if let Some(obj) = response.as_object_mut() {
        obj.insert("wake".to_string(), wake_summary);
    }
    posted.response = response;
    Ok(posted)
}

/// Dashboard/本地发言入口（批次 E `board.channel.post` 桥的实现底座）：
/// 与 worker 上行同一条管线（幂等 → 额度 → 落库 → 异步裁决投递），错误
/// 映射人读文案。返回首响 `{comment_id|message_id, seq}`。
#[allow(clippy::too_many_arguments)]
pub fn post_discussion_locally(
    deps: &MasterBusDeps,
    sender: &Actor,
    thread_kind: &str,
    thread_id: i64,
    client_msg_id: &str,
    content: &str,
    reply_to: Option<i64>,
    kind_tag: &str,
) -> Result<serde_json::Value, String> {
    match post_discussion_core(
        deps,
        sender,
        thread_kind,
        thread_id,
        client_msg_id,
        content,
        reply_to,
        kind_tag,
        true,
    ) {
        Ok(p) => Ok(p.response),
        Err(PostError::Quota(denied)) => {
            Err(format!("[{}] {}", denied.error_code(), denied.message()))
        }
        Err(PostError::Store(e)) | Err(PostError::DedupCheck(e)) => Err(e),
    }
}

/// [`nemesis_board::service::DiscussionIngress`] 实现：dashboard 发言桥
/// （gateway 装配进 BoardService，web 层 `board.channel.post` 经它入管线）。
pub struct LocalDiscussionIngress {
    pub deps: MasterBusDeps,
}

impl nemesis_board::service::DiscussionIngress for LocalDiscussionIngress {
    fn post(
        &self,
        sender: &Actor,
        thread_kind: &str,
        thread_id: i64,
        client_msg_id: &str,
        content: &str,
        reply_to: Option<i64>,
        kind_tag: &str,
    ) -> Result<serde_json::Value, String> {
        post_discussion_locally(
            &self.deps,
            sender,
            thread_kind,
            thread_id,
            client_msg_id,
            content,
            reply_to,
            kind_tag,
        )
    }
}

/// `board.comment.list`（D2 读取臂）：worker 轮询讨论线程——纯查表回
/// 消息清单，不落库、不占额度三闸（额度管的是写侧噪声，读是廉价操作）。
/// 错误处理沿 comment.post 的 envelope 先例：字段缺失 = VALIDATION，
/// 存储失败 = INTERNAL。limit 收敛 1..=500（防单次全量拖库；线程本身
/// 受写侧额度约束不会大，收敛是防御性的）。
fn handle_comment_list(deps: &MasterBusDeps, env: &Envelope) -> EnvelopeResponse {
    let body = &env.body;
    let Some(thread_kind) = body
        .get("thread")
        .and_then(|t| t.get("kind"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
    else {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, "missing thread.kind"),
        );
    };
    let Some(thread_id) = body
        .get("thread")
        .and_then(|t| t.get("id"))
        .and_then(|v| v.as_i64())
    else {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, "missing thread.id"),
        );
    };
    let limit = body
        .get("limit")
        .and_then(|v| v.as_i64())
        .unwrap_or(200)
        .clamp(1, 500);

    let listed = match thread_kind.as_str() {
        "issue" => deps.store.list_comments(thread_id).map(|comments| {
            comments
                .into_iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "sender": {"type": c.author.kind, "id": c.author.id},
                        "content": c.content,
                        "reply_to": c.parent_id,
                        "kind_tag": c.ctype.as_str(),
                        "created_at": c.created_at,
                    })
                })
                .collect::<Vec<_>>()
        }),
        "channel" => deps
            .store
            .list_channel_messages(thread_id, 0, limit)
            .map(|msgs| {
                msgs.into_iter()
                    .map(|m| {
                        serde_json::json!({
                            "id": m.id,
                            "sender": {"type": m.sender.kind, "id": m.sender.id},
                            "content": m.content,
                            "reply_to": m.parent_id,
                            "kind_tag": m.mtype,
                            "created_at": m.created_at,
                        })
                    })
                    .collect::<Vec<_>>()
            }),
        other => {
            return EnvelopeResponse::failure(
                env,
                EnvelopeError::new(
                    envelope::error_code::VALIDATION,
                    format!("unknown thread kind: {other}"),
                ),
            );
        }
    };
    match listed {
        Ok(mut messages) => {
            // issue 线程查表无 limit 参数（历史 API），取回后统一截断——
            // 新消息在尾部（ORDER BY id/seq ASC），保尾部 = 保最新。
            if messages.len() > limit as usize {
                messages.drain(..messages.len() - limit as usize);
            }
            EnvelopeResponse::success(
                env,
                serde_json::json!({
                    "thread": {"kind": thread_kind, "id": thread_id},
                    "messages": messages,
                }),
            )
        }
        Err(e) => {
            EnvelopeResponse::failure(env, EnvelopeError::new(envelope::error_code::INTERNAL, e))
        }
    }
}

/// `board.sync`：增量补拉（G8 的 master 侧）。
fn handle_sync(deps: &MasterBusDeps, env: &Envelope) -> EnvelopeResponse {
    let since_seq = env
        .body
        .get("since_seq")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let limit = env
        .body
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(500)
        .clamp(1, 2000) as i64;
    match deps.store.list_messages_since(since_seq, limit) {
        Ok(entries) => {
            let latest = deps.store.latest_seq().unwrap_or(0);
            EnvelopeResponse::success(
                env,
                serde_json::json!({
                    "messages": entries,
                    "latest_seq": latest,
                }),
            )
        }
        Err(e) => {
            EnvelopeResponse::failure(env, EnvelopeError::new(envelope::error_code::INTERNAL, e))
        }
    }
}

/// spawn 任务的依赖快照（Arc 克隆进 tokio 任务）。
struct DepsForTask {
    store: Arc<nemesis_board::BoardStore>,
    quota: Arc<QuotaLedger>,
    cluster: Arc<nemesis_cluster::cluster::Cluster>,
    moderator_loop: Arc<OnceLock<Arc<nemesis_agent::r#loop::AgentLoop>>>,
}

/// 新消息上下文（裁决 + wake 包组装的输入）。
struct WakeContext {
    thread_kind: String,
    thread_id: i64,
    sender: Actor,
    content: String,
    reply_to: Option<i64>,
    seq: i64,
    at: i64,
}

/// wake 下行 + 主持人裁决（tokio 任务主体）。plan/nodes 由同步段算好传入
/// （F3：裁决已随首响带回前端，本任务只做投递 + 主持人裁决）。
async fn deliver_wakeups(
    deps: DepsForTask,
    ctx: WakeContext,
    plan: WakePlan,
    nodes: Vec<NodeCandidate>,
) {
    let self_node_id = deps.cluster.node_id().to_string();
    let thread_key = format!("{}:{}", ctx.thread_kind, ctx.thread_id);

    // 审计日志（G3 出口：谁被唤醒 / 谁被跳过 / 为什么）。
    let skipped_summary: Vec<String> = plan
        .skipped
        .iter()
        .map(|s| format!("{}({})", s.node_id, s.reason))
        .collect();
    tracing::info!(
        target: "board_bus",
        seq = ctx.seq,
        thread = %thread_key,
        sender = %ctx.sender.id,
        woke = ?plan.targets,
        moderator = plan.to_moderator,
        skipped = ?skipped_summary,
        "[nb_bus] wake decision"
    );

    // 线程额度耗尽 → 不再投递只推人（护栏握在 master 手里；dashboard
    // 通知在批次 E 接线，此处先日志可见）。
    let turns_left = deps.quota.turns_left(&thread_key);
    if turns_left == 0 {
        tracing::warn!(
            target: "board_bus",
            thread = %thread_key,
            "[nb_bus] thread quota exhausted — wake suppressed, human attention needed"
        );
        return;
    }

    // wake 事件标签（worker 侧 prompt 的「唤醒原因」）：无 @ + issue 指派
    // → assignee_comment；其余（含 @ 点名/角色点名）→ mention。
    let issue_assignee = if ctx.thread_kind == thread_kind::ISSUE {
        deps.store
            .get_issue(ctx.thread_id)
            .ok()
            .and_then(|issue| match issue.assignee {
                Some(nemesis_board::AssignmentType::Worker) => issue.assignee_id,
                _ => None,
            })
    } else {
        None
    };
    let event_label = match ctx.thread_kind == thread_kind::ISSUE
        && issue_assignee.is_some()
        && !has_mentions(&ctx.content)
    {
        true => "assignee_comment",
        false => "mention",
    };

    // wake.post 下行：逐个在线目标投递；失败 warn（board.sync 兜底）。
    // 被点名的是主持人自己 → 本地裁决（master 没有 worker handler，不
    // 给自己发 wake.post）；此时 master 以**参与者**身份回话（@all/@点名
    // 已把其他人直接唤醒，master 不该再以裁决者口吻转达点名——2026-10-23
    // 用户实录：@all 后 master 把消息转述成对 @Product Manager 的喊话）。
    let mut need_moderator = plan.to_moderator;
    let mut master_woken = false;
    if !plan.targets.is_empty() {
        let rpc = match deps.cluster.rpc_client_arc() {
            Some(c) => c,
            None => {
                tracing::warn!(target: "board_bus", "[nb_bus] rpc client unavailable, wake skipped");
                return;
            }
        };
        for target in &plan.targets {
            if *target == self_node_id {
                need_moderator = true;
                master_woken = true;
                continue;
            }
            let payload = match build_wake_envelope(&deps.store, &ctx, turns_left, event_label) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(target: "board_bus", target = %target, error = %e,
                        "[nb_bus] wake envelope build failed");
                    continue;
                }
            };
            let request = RPCRequest {
                id: uuid::Uuid::new_v4().to_string(),
                action: ActionType::Custom(envelope::NB_BUS_ACTION.to_string()),
                payload,
                source: self_node_id.clone(),
                target: Some(target.clone()),
            };
            match rpc.call(target, request).await {
                Ok(_) => {
                    tracing::debug!(target: "board_bus", target = %target, seq = ctx.seq,
                        "[nb_bus] wake delivered");
                }
                Err(e) => {
                    tracing::warn!(target: "board_bus", target = %target, seq = ctx.seq,
                        error = %e, "[nb_bus] wake delivery failed (board.sync will backfill)");
                }
            }
        }
    }

    // 规则③（或主持人被直接点名）：主持人本地 agent。to_moderator（无 @）
    // → 裁决者模式（可点名他人）；被 @/@all 显式唤醒 → 参与者模式（正常
    // 回话）。回复照常落库并定点唤醒被 @ 的节点。额度已耗尽时不进裁决
    // （见上方 return）。
    if need_moderator
        && let Err(e) =
            run_moderator(deps, ctx, &nodes, &self_node_id, master_woken, event_label).await
    {
        tracing::warn!(target: "board_bus", thread = %thread_key, error = %e,
            "[nb_bus] moderator adjudication failed");
    }
}

/// 主持人 prompt 双模式构造（纯函数，单测直接钉行为）。
/// - 裁决者（participant=false）：可点名他人（契约不变；发送者行同步升级
///   为带身份标注的 sender_label，见 actor_label）；
/// - 参与者（participant=true）：与 worker 讨论轮同语义（回复正文或
///   [SILENT]，见 cluster_agent::build_discussion_prompt）——名册降级为
///   上下文（标注「已被直接唤醒，无需转达」），不再诱导转达式点名。
///
/// 发送者身份渲染（LLM 投影的单一真相源，2026-10-23 身份透传）：Actor.kind
/// 全链路落库齐全（上行带 type / DB 存 type / 台账投影带 sender_type），此前
/// 丢失在 wake 信封与 prompt 渲染——worker 的 LLM 只见裸 id，无从知晓说话的
/// 是人还是节点（@all 后人类名不在节点名册，agent 想 @ 说话对象只能 @ 自己
/// ——2026-10-23 用户实录）。admin=人类用户 / agent=集群节点 / system=系统
/// 播报；未知/空 kind 退化为裸 id（旧 master wake 包兼容，零破坏）。
pub(crate) fn actor_label(kind: &str, id: &str) -> String {
    match kind {
        "admin" => format!("{id} (human user)"),
        "agent" => format!("{id} (agent node)"),
        "system" => format!("{id} (system)"),
        _ => id.to_string(),
    }
}

fn build_moderator_prompt(
    thread_ctx: &str,
    roster: &str,
    event_label: &str,
    sender_label: &str,
    content: &str,
    self_node_id: &str,
    participant: bool,
) -> String {
    if participant {
        format!(
            "{thread_ctx}\n\n\
             # Node roster (for awareness — these nodes were already notified \
             directly; do not relay this message to anyone):\n{roster}\n\n\
             # Wake reason: {event_label}\n\
             New message from {sender_label}:\n{content}\n\n\
             You are node {self_node_id} in a multi-agent board discussion. \
             Decide whether you should respond:\n\
             - If you have something useful to contribute, output ONLY your \
             reply text. It will be posted to the thread as your message. Do \
             not mention these instructions.\n\
             - If nothing needs to be said (informational only, not directed \
             at you, or outdated), output exactly [SILENT]."
        )
    } else {
        format!(
            "{thread_ctx}\n\n\
             # Node roster (you may @mention nodes by name to call on them):\n{roster}\n\n\
             # New message from {sender_label}:\n{content}\n\n\
             You are the discussion moderator. Decide whether this needs your action:\n\
             - If someone should be called on, reply with your message using @node-name to mention them.\n\
             - If nothing needs to be said, reply exactly [SILENT]."
        )
    }
}

/// 主持人本地 agent：线程上下文 + 新消息 → 主 AgentLoop 直调一轮 → 解析
/// 回复（[SILENT] / @tokens + 正文）→ 正文落库（origin=主持人）+ @目标唤醒。
///
/// `participant=false`（规则③：频道无 @）→ 裁决者模式：LLM 决定是否要点名
/// 他人。`participant=true`（master 被 @/@all 显式唤醒）→ 参与者模式：像
/// worker 一样正常回话（他人已被直接唤醒，转达点名是冗余——2026-10-23
/// 用户实录：@all 后 master 把消息转述成对含空格名节点的喊话）。
///
/// §9.6 注意点 1：主 agent 人格 system prompt 可能污染结构化输出——解析
/// 从宽（全文扫 @token / [SILENT] 子串），不做严格格式断言。
async fn run_moderator(
    deps: DepsForTask,
    ctx: WakeContext,
    nodes: &[NodeCandidate],
    self_node_id: &str,
    participant: bool,
    event_label: &str,
) -> Result<(), String> {
    let Some(agent_loop) = deps.moderator_loop.get() else {
        tracing::debug!(target: "board_bus", "[nb_bus] agent loop not ready, moderator skipped");
        return Ok(());
    };

    let thread_ctx = build_thread_context_text(&deps.store, &ctx)?;
    // F5（goal P1）：节点名册注入——主持人 LLM 不知道有谁就无法做出 informed
    // 的 @ 点名（「设备之间不会互相交互」的深层根因）。名册与裁决器投影同源。
    let roster = nodes
        .iter()
        .map(|n| {
            format!(
                "- {} (role: {}, category: {}, {})",
                n.name,
                n.role,
                n.category,
                if n.online { "online" } else { "offline" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = build_moderator_prompt(
        &thread_ctx,
        &roster,
        event_label,
        &actor_label(&ctx.sender.kind, &ctx.sender.id),
        &ctx.content,
        self_node_id,
        participant,
    );
    let session_key = format!(
        "board:moderator:{thread_kind}",
        thread_kind = ctx.thread_kind
    );
    let reply = agent_loop.process_direct(&prompt, &session_key).await?;
    let trimmed = reply.trim();

    // [SILENT] 判定（从宽：子串匹配，防人格前后缀）。
    if trimmed.contains("[SILENT]") {
        tracing::debug!(target: "board_bus", thread = %format!("{}:{}", ctx.thread_kind, ctx.thread_id),
            "[nb_bus] moderator chose silence");
        return Ok(());
    }

    // 主持人回复落库（origin=主持人节点；幂等键从 seq 派生——重演安全）。
    let client_msg_id = format!("moderator-{}", uuid::Uuid::new_v4());
    let kind_tag = if ctx.thread_kind == thread_kind::ISSUE {
        "discussion"
    } else {
        "text"
    };
    let posted = deps.store.post_discussion_envelope(
        self_node_id,
        &client_msg_id,
        &ctx.thread_kind,
        ctx.thread_id,
        &Actor::new("agent", self_node_id),
        trimmed,
        None,
        kind_tag,
    )?;

    // 主持人回复里的 @tokens 走同一裁决器定点唤醒。
    let mod_ctx = WakeContext {
        thread_kind: ctx.thread_kind.clone(),
        thread_id: ctx.thread_id,
        sender: Actor::new("agent", self_node_id),
        content: trimmed.to_string(),
        reply_to: None,
        seq: posted.seq,
        at: chrono::Utc::now().timestamp(),
    };
    let input = WakeInput {
        thread_kind: &mod_ctx.thread_kind,
        content: &mod_ctx.content,
        sender_id: self_node_id,
        issue_assignee: None, // 主持人点名规则不走指派兜底。
        moderator_id: self_node_id,
    };
    let plan = resolve_wake_targets(&input, nodes);
    if plan.targets.is_empty() {
        return Ok(());
    }
    let turns_left = deps
        .quota
        .turns_left(&format!("{}:{}", ctx.thread_kind, ctx.thread_id));
    if turns_left == 0 {
        return Ok(()); // 已在 warn 日志里（外层调用点），此处静默收敛。
    }
    let Some(rpc) = deps.cluster.rpc_client_arc() else {
        return Ok(());
    };
    for target in &plan.targets {
        let payload = build_wake_envelope(&deps.store, &mod_ctx, turns_left, "moderator_call")?;
        let request = RPCRequest {
            id: uuid::Uuid::new_v4().to_string(),
            action: ActionType::Custom(envelope::NB_BUS_ACTION.to_string()),
            payload,
            source: self_node_id.to_string(),
            target: Some(target.clone()),
        };
        if let Err(e) = rpc.call(target, request).await {
            tracing::warn!(target: "board_bus", target = %target,
                error = %e, "[nb_bus] moderator wake delivery failed");
        }
    }
    Ok(())
}

/// 组装 §5.2② wake.post 下行信封（唤醒原因 + 线程上下文 + 新消息 +
/// reply_hint + seq）。issue 线程随带 title / prd_summary（描述截断），
/// worker 醒来一次拿全背景。
fn build_wake_envelope(
    store: &nemesis_board::BoardStore,
    ctx: &WakeContext,
    max_turns_left: u32,
    event_label: &str,
) -> Result<serde_json::Value, String> {
    let messages = thread_context_json(store, ctx)?;
    // F7（goal P1）：频道名透传——channel 线程此前 title 恒空，worker 只见
    // 不透明的 "channel:N"，不知道自己在 #dev 还是 #general。
    let (title, prd_summary) = if ctx.thread_kind == thread_kind::ISSUE {
        match store.get_issue(ctx.thread_id) {
            Ok(issue) => (issue.title.clone(), truncate_chars(&issue.description, 500)),
            Err(_) => (String::new(), String::new()),
        }
    } else {
        let channel_name = store
            .list_channels()
            .ok()
            .and_then(|cs| {
                cs.iter()
                    .find(|c| c.id == ctx.thread_id)
                    .map(|c| format!("#{}", c.name))
            })
            .unwrap_or_default();
        (channel_name, String::new())
    };
    Ok(EnvelopeResponse::success(
        &Envelope {
            ns: "board".to_string(),
            op: "wake.post".to_string(),
            corr_id: uuid::Uuid::new_v4().to_string(),
            ..Envelope::default()
        },
        serde_json::json!({
            "event": event_label,
            "thread": {
                "kind": ctx.thread_kind,
                "id": ctx.thread_id,
                "title": title,
                "prd_summary": prd_summary,
                "messages": messages,
            },
            "new_message": {
                "sender": ctx.sender.id,
                "sender_kind": ctx.sender.kind,
                "content": ctx.content,
                "at": ctx.at,
            },
            "reply_hint": {
                "reply_to": ctx.reply_to,
                "max_turns_left": max_turns_left,
            },
            "seq": ctx.seq,
        }),
    )
    .to_json())
}

/// 按字符数截断（char boundary 安全；超出补省略号）。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let t: String = s.chars().take(max).collect();
    format!("{t}…")
}

/// 线程上下文最近 N 条（§5.2②：messages 数组）。issue → 评论；channel →
/// 频道消息。
fn thread_context_json(
    store: &nemesis_board::BoardStore,
    ctx: &WakeContext,
) -> Result<serde_json::Value, String> {
    #[derive(serde::Serialize)]
    struct CtxMessage {
        sender: String,
        kind: String,
        content: String,
        at: i64,
    }
    let mut out: Vec<CtxMessage> = Vec::new();
    if ctx.thread_kind == thread_kind::ISSUE {
        for c in store.list_comments(ctx.thread_id)? {
            out.push(CtxMessage {
                sender: c.author.id,
                kind: c.author.kind,
                content: c.content,
                at: c.created_at,
            });
        }
    } else {
        for m in store.list_channel_messages(ctx.thread_id, 0, i64::MAX)? {
            out.push(CtxMessage {
                sender: m.sender.id,
                kind: m.sender.kind,
                content: m.content,
                at: m.created_at,
            });
        }
    }
    let start = out.len().saturating_sub(WAKE_CONTEXT_MESSAGES);
    serde_json::to_value(&out[start..]).map_err(|e| format!("serialize thread context: {e}"))
}

/// 主持人 prompt 用的纯文本线程上下文（与 wake 包同源，20 条）。
fn build_thread_context_text(
    store: &nemesis_board::BoardStore,
    ctx: &WakeContext,
) -> Result<String, String> {
    let json = thread_context_json(store, ctx)?;
    let mut lines = Vec::new();
    if let Some(arr) = json.as_array() {
        for m in arr {
            lines.push(format!(
                "- {}: {}",
                actor_label(
                    m.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
                    m.get("sender").and_then(|v| v.as_str()).unwrap_or("?"),
                ),
                m.get("content").and_then(|v| v.as_str()).unwrap_or("")
            ));
        }
    }
    let title = if ctx.thread_kind == thread_kind::ISSUE {
        store
            .get_issue(ctx.thread_id)
            .map(|i| i.title)
            .unwrap_or_default()
    } else {
        String::new()
    };
    Ok(format!(
        "# Thread ({} {}) {}\n{}",
        ctx.thread_kind,
        ctx.thread_id,
        title,
        lines.join("\n")
    ))
}

// ---------------------------------------------------------------------------
// Worker 侧 nb_bus（G4 被动响应 + G8 离线补拉）
// ---------------------------------------------------------------------------

/// board.sync 补拉周期（兜底通道节拍；实时靠 wake.post 推送，60s 一拍
/// 只是为了不丢——离线错过的讨论最迟一个节拍补上）。
const BOARD_SYNC_INTERVAL_SECS: u64 = 60;
/// 单次补拉上限（与 master 侧 handle_sync 的 clamp 上限对齐）。
const BOARD_SYNC_PULL_LIMIT: i64 = 500;

/// worker 下行幂等状态（impl-plan §5.2②：每线程记录已处理最大 seq，
/// `seq ≤` 已处理的直接丢弃——RPC 重发/TCP 重传天然免疫）。进程内存态
/// （重启清零；重启后 board.sync 从 0 重拉，agent 自行判断过时——impl-plan
/// §5.2③ 的诚实边界）。
pub struct WorkerWakeState {
    inner: std::sync::Mutex<WorkerWakeInner>,
    /// 盘上快照路径（Some = 每次 commit/advance 原子落盘；None = 纯内存，
    /// 测试/无需持久化场景）。G8（2026-09-09 双机）：worker 重启水位接续，
    /// 历史 @ 不再全量重放（此前内存态归零 → board.sync since_seq=0 →
    /// 已处理的讨论整批重跑，重复发言只被模型沉默行为侥幸掩盖）。
    persist_path: Option<std::path::PathBuf>,
}

/// 盘上快照形态（ participated 可由 threads.keys() 重建，不落盘）。
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct WorkerWakeSnapshot {
    threads: HashMap<String, i64>,
    watermark: i64,
}

#[derive(Default)]
struct WorkerWakeInner {
    /// thread_key → 已处理最大 seq。
    threads: HashMap<String, i64>,
    /// 参与过的线程（wake 处理过即算；sync 过滤「我参与的线程」用）。
    participated: HashSet<String>,
    /// 全局已见最大 seq（board.sync 增量游标；每次 sync 成功推进到
    /// master 的 latest_seq，与每线程水位互补）。
    watermark: i64,
}

impl WorkerWakeState {
    /// 纯内存构造（测试 / 无需持久化场景；生产 gateway 走 `load_or_create`）。
    /// 恢复方法：把 gateway worker 分支换回 `new()` 即回到内存态行为。
    #[allow(dead_code)] // 仅测试消费（tests.rs）；生产入口=load_or_create
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(WorkerWakeInner::default()),
            persist_path: None,
        }
    }

    /// G8：从盘上快照恢复（无文件/损坏 → 空账起步，不炸）。之后每次
    /// commit/advance 落盘——重启后 peek/since_seq 从上次位置继续，
    /// 已处理的历史 wake 不再重新入队执行。
    pub fn load_or_create(snapshot_path: std::path::PathBuf) -> Self {
        let snapshot = std::fs::read_to_string(&snapshot_path)
            .ok()
            .and_then(|s| serde_json::from_str::<WorkerWakeSnapshot>(&s).ok())
            .unwrap_or_default();
        tracing::info!(
            "[BoardBus] wake state restored: {} threads, watermark={} (path={})",
            snapshot.threads.len(),
            snapshot.watermark,
            snapshot_path.display()
        );
        let participated = snapshot.threads.keys().cloned().collect();
        Self {
            inner: std::sync::Mutex::new(WorkerWakeInner {
                threads: snapshot.threads,
                participated,
                watermark: snapshot.watermark,
            }),
            persist_path: Some(snapshot_path),
        }
    }

    /// 预检（不记账）：seq 比该线程已处理的大才要处理。
    pub fn peek(&self, thread_key: &str, seq: i64) -> bool {
        seq > self.lock().threads.get(thread_key).copied().unwrap_or(0)
    }

    /// 记账（**入队成功后**调用）：推进线程水位 + 标记参与线程。
    /// 先 peek 再 send 再 commit——send 失败不记账，board.sync 下轮重捞。
    pub fn commit(&self, thread_key: &str, seq: i64) {
        let snapshot = {
            let mut g = self.lock();
            let cur = g.threads.entry(thread_key.to_string()).or_insert(0);
            if seq > *cur {
                *cur = seq;
            }
            g.participated.insert(thread_key.to_string());
            WorkerWakeSnapshot {
                threads: g.threads.clone(),
                watermark: g.watermark,
            }
        };
        self.persist_snapshot(&snapshot);
    }

    /// sync 增量游标。
    pub fn watermark(&self) -> i64 {
        self.lock().watermark
    }

    /// 推进全局游标（master 返回的 latest_seq；即使条目全被过滤）。
    pub fn advance_watermark(&self, seq: i64) {
        let snapshot = {
            let mut g = self.lock();
            if seq > g.watermark {
                g.watermark = seq;
            }
            WorkerWakeSnapshot {
                threads: g.threads.clone(),
                watermark: g.watermark,
            }
        };
        self.persist_snapshot(&snapshot);
    }

    /// 「我参与的线程」判定（sync 过滤）。
    pub fn is_participated(&self, thread_key: &str) -> bool {
        self.lock().participated.contains(thread_key)
    }

    /// 盘上落盘（锁外 IO；失败只 warn 不上抛——持久化是韧性增强，绝不
    /// 反噬 wake 主链路；丢快照的最坏后果=回到重启重放的旧行为）。
    fn persist_snapshot(&self, snapshot: &WorkerWakeSnapshot) {
        let Some(path) = &self.persist_path else {
            return;
        };
        let Ok(json) = serde_json::to_string(snapshot) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if let Err(e) = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")))
            .and_then(|_| std::fs::write(&tmp, json))
            .and_then(|_| std::fs::rename(&tmp, path))
        {
            tracing::warn!(
                "[BoardBus] wake state persist failed: {} (continuing in-memory)",
                e
            );
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WorkerWakeInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// worker 侧 nb_bus 组装依赖（gateway 装配点注入）。
#[derive(Clone)]
pub struct WorkerBusDeps {
    pub self_node_id: String,
    pub node_name: String,
    /// 拓扑角色（coordinator/worker；@role: 匹配参与）。
    pub node_role: String,
    /// 功能类别（qa/dev/general；@role: 匹配参与）。
    pub node_category: String,
    /// 讨论事件入站箱（桥到 cluster agent loop 第三臂）。
    pub inbox: Arc<crate::cluster_agent::DiscussionInbox>,
    /// 下行 seq 幂等状态。
    pub wake_state: Arc<WorkerWakeState>,
}

/// 注册 worker 侧 `nb_bus` handler（gateway 在 cluster Arc 化之后调用；
/// master 形态注册的是 [`build_master_nb_bus_handler`]，同名 action 并存
/// 于不同节点）。
pub fn build_worker_nb_bus_handler(
    deps: WorkerBusDeps,
) -> nemesis_cluster::rpc::server::RpcHandlerFn {
    Box::new(move |payload| handle_worker_nb_bus(&deps, payload))
}

fn handle_worker_nb_bus(
    deps: &WorkerBusDeps,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let env = match envelope::parse_envelope(&payload) {
        Ok(e) => e,
        Err(err) => {
            let fallback = fallback_envelope(&payload);
            return Ok(EnvelopeResponse::failure(&fallback, err).to_json());
        }
    };
    if env.ns != "board" {
        return Ok(EnvelopeResponse::failure(
            &env,
            EnvelopeError::new(
                envelope::error_code::UNKNOWN_NS,
                format!("unknown ns: {} (want board)", env.ns),
            ),
        )
        .to_json());
    }
    match env.op.as_str() {
        "wake.post" => Ok(handle_wake_post(deps, &env, rpc_from_node(&payload)).to_json()),
        other => Ok(EnvelopeResponse::failure(
            &env,
            EnvelopeError::new(
                envelope::error_code::UNKNOWN_OP,
                format!("unknown op: {other}"),
            ),
        )
        .to_json()),
    }
}

/// RPC 帧元数据里的发送者（server 注入 `_rpc.from`，伪造不了）。
fn rpc_from_node(payload: &serde_json::Value) -> &str {
    payload
        .get("_rpc")
        .and_then(|r| r.get("from"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// wake.post 下行：解析 → seq 幂等 → 投进 cluster agent loop（越薄越好：
/// JSON → DiscussionEvent → inbox.send，LLM 一概不碰）。
fn handle_wake_post(deps: &WorkerBusDeps, env: &Envelope, from_node: &str) -> EnvelopeResponse {
    let event = match parse_wake_body(env, from_node) {
        Ok(e) => e,
        Err(msg) => {
            return EnvelopeResponse::failure(
                env,
                EnvelopeError::new(envelope::error_code::VALIDATION, msg),
            );
        }
    };
    let thread_key = format!("{}:{}", event.thread_kind, event.thread_id);
    let seq = event.seq;
    if !deps.wake_state.peek(&thread_key, seq) {
        // 幂等丢弃不是错误：RPC 层重发的重复唤醒，诚实返回已处理。
        return EnvelopeResponse::success(env, serde_json::json!({"duplicate": true}));
    }
    match deps.inbox.send(event) {
        Ok(()) => {
            deps.wake_state.commit(&thread_key, seq);
            EnvelopeResponse::success(env, serde_json::json!({"queued": true}))
        }
        Err(e) => EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::UNAVAILABLE, e),
        ),
    }
}

/// §5.2② wake 包 → DiscussionEvent（缺字段 = validation 错误）。
fn parse_wake_body(env: &Envelope, from_node: &str) -> Result<DiscussionEvent, String> {
    let body = &env.body;
    let Some(thread_kind) = body
        .pointer("/thread/kind")
        .and_then(|v| v.as_str())
        .map(String::from)
    else {
        return Err("missing thread.kind".to_string());
    };
    let Some(thread_id) = body.pointer("/thread/id").and_then(|v| v.as_i64()) else {
        return Err("missing thread.id".to_string());
    };
    let Some(new_sender) = body
        .pointer("/new_message/sender")
        .and_then(|v| v.as_str())
        .map(String::from)
    else {
        return Err("missing new_message.sender".to_string());
    };
    let Some(new_content) = body
        .pointer("/new_message/content")
        .and_then(|v| v.as_str())
        .map(String::from)
    else {
        return Err("missing new_message.content".to_string());
    };
    let Some(seq) = body.get("seq").and_then(|v| v.as_i64()) else {
        return Err("missing seq (downstream idempotency cursor)".to_string());
    };
    let messages = body
        .pointer("/thread/messages")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|m| DiscussionCtxMessage {
                    sender: m
                        .get("sender")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    // 旧 master wake 包无 kind → 空串（渲染退化为裸 id）。
                    kind: m
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    content: m
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    at: m.get("at").and_then(|v| v.as_i64()).unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(DiscussionEvent {
        event: body
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("mention")
            .to_string(),
        thread_title: body
            .pointer("/thread/title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        messages,
        new_at: body
            .pointer("/new_message/at")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        reply_to: body
            .pointer("/reply_hint/reply_to")
            .and_then(|v| v.as_i64()),
        max_turns_left: body
            .pointer("/reply_hint/max_turns_left")
            .and_then(|v| v.as_u64())
            .unwrap_or(u32::MAX as u64) as u32,
        thread_kind,
        thread_id,
        from_node: from_node.to_string(),
        new_sender,
        sender_kind: body
            .pointer("/new_message/sender_kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        new_content,
        seq,
    })
}

/// G8 周期补拉：常驻任务，每 [`BOARD_SYNC_INTERVAL_SECS`] 一拍向在线
/// coordinator 发 board.sync，把「@我的 / 我参与的线程」里离线错过的
/// 讨论补进 cluster agent loop。找不到 coordinator / 无 RPC / 请求失败
/// → 本拍跳过（下一拍再试），不炸不重试风暴。
pub fn spawn_worker_sync_loop(
    deps: WorkerBusDeps,
    cluster: Arc<nemesis_cluster::cluster::Cluster>,
) {
    tokio::spawn(async move {
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(BOARD_SYNC_INTERVAL_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match worker_sync_once(&deps, &cluster).await {
                Ok(n) if n > 0 => {
                    tracing::info!(target: "board_bus", enqueued = n,
                        "[nb_bus] board.sync backfill enqueued");
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!(target: "board_bus", error = %e,
                        "[nb_bus] board.sync tick skipped");
                }
            }
        }
    });
}

/// 单拍补拉：找在线 coordinator → board.sync → 过滤入队 → 推进游标。
async fn worker_sync_once(
    deps: &WorkerBusDeps,
    cluster: &nemesis_cluster::cluster::Cluster,
) -> Result<usize, String> {
    // master 地址每次现查（节点表是鲜活的）；自己是 coordinator 说明
    // master 形态误装了 worker 通道，防御性跳过（主持人走本地裁决）。
    let target = cluster
        .list_nodes()
        .into_iter()
        .find(|n| {
            n.base.role == nemesis_types::cluster::NodeRole::Coordinator
                && n.base.id != deps.self_node_id
                && n.is_online()
        })
        .map(|n| n.base.id)
        .ok_or_else(|| "no online coordinator known yet".to_string())?;
    let rpc = cluster
        .rpc_client_arc()
        .ok_or_else(|| "rpc client unavailable".to_string())?;
    let payload = serde_json::json!({
        "v": envelope::ENVELOPE_VERSION,
        "ns": "board",
        "op": "sync",
        "corr_id": uuid::Uuid::new_v4().to_string(),
        "body": {
            "since_seq": deps.wake_state.watermark(),
            "limit": BOARD_SYNC_PULL_LIMIT,
        },
    });
    let request = RPCRequest {
        id: uuid::Uuid::new_v4().to_string(),
        action: ActionType::Custom(envelope::NB_BUS_ACTION.to_string()),
        payload,
        source: deps.self_node_id.clone(),
        target: Some(target.clone()),
    };
    let resp = rpc
        .call(&target, request)
        .await
        .map_err(|e| format!("sync rpc: {e}"))?;
    let body = resp.result.unwrap_or(serde_json::Value::Null);
    if !body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let code = body
            .pointer("/error/code")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        return Err(format!("sync rejected: {code}"));
    }
    let latest = body
        .pointer("/body/latest_seq")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let entries = body
        .pointer("/body/messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut enqueued = 0usize;
    // 投递失败条目的最小 seq（2026-09-18 T26 根因）：sync 第一拍可能跑得
    // 比 cluster agent loop 就绪更早（重启后 ticker 立即 tick，loop 要等
    // 装配完成才 set_sender）→ inbox.send 失败（no active loop）。此前
    // advance_watermark 无条件推进 latest → 该条目被跳过且永不重拉——
    // 补拉机制自己吞掉了 wake 事件，离线韧性失效。改为游标只推进到
    // 失败条目之前：下一拍重拉重试，loop 就绪后自然补投。
    let mut first_undelivered_seq: Option<i64> = None;
    for entry in &entries {
        let seq = entry.get("seq").and_then(|v| v.as_i64()).unwrap_or(0);
        let kind = entry
            .get("thread_kind")
            .and_then(|v| v.as_str())
            .unwrap_or("channel");
        let tid = entry.get("thread_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let sender = entry
            .get("sender_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // 台账投影自带 sender_type（LedgerEntry Serialize）——身份透传到
        // 补拉事件，与 wake 推送同语义。
        let sender_kind = entry
            .get("sender_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let content = entry.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let at = entry
            .get("created_at")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let thread_key = format!("{kind}:{tid}");
        if !sync_entry_targets_me(deps, content, sender, &thread_key) {
            continue;
        }
        if !deps.wake_state.peek(&thread_key, seq) {
            continue;
        }
        // 补拉事件不带线程历史（master 台账里查不到 per-thread 投影）——
        // agent 拿到的就是补投到醒为止的最小信息，过时与否自行判断
        // （impl-plan §5.2③）。
        // reply_to 必须是 master 台账 message id（DiscussionEvent 契约）；
        // sync 台账投影里只有线程 seq，冒充 id 会与全局评论 id 空间相撞
        // ——worker 原样回传 post，parent_id 错挂到别的单子的评论上
        // （run4 实录 2026-10-14：NB-43 讨论回帖 parent=2/3/4/5 全是
        // NB-1 时代的评论 id）。补拉是批量追赶形态，回复落根评论
        // （reply_to=None）即诚实且无碰撞。
        let event = DiscussionEvent {
            thread_kind: kind.to_string(),
            thread_id: tid,
            thread_title: String::new(),
            messages: Vec::new(),
            event: "sync_backfill".to_string(),
            from_node: target.clone(),
            new_sender: sender.to_string(),
            sender_kind: sender_kind.to_string(),
            new_content: content.to_string(),
            new_at: at,
            reply_to: None,
            max_turns_left: u32::MAX,
            seq,
        };
        if deps.inbox.send(event).is_ok() {
            deps.wake_state.commit(&thread_key, seq);
            enqueued += 1;
        } else {
            first_undelivered_seq = Some(match first_undelivered_seq {
                Some(m) => m.min(seq),
                None => seq,
            });
        }
    }
    // 游标推进到 master latest_seq：见过的（含被过滤的）不重拉；投递失败
    // 的条目停在原地（min 也压住排在失败条目之后的已过滤条目——下一拍
    // 重拉时会被同样的过滤条件跳过，只是多拉一点，语义无损）。
    let advance_to = sync_advance_target(first_undelivered_seq, latest);
    deps.wake_state.advance_watermark(advance_to);
    Ok(enqueued)
}

/// G8 游标推进目标：无投递失败 → 推进到 master `latest`；有失败条目 →
/// 只推进到失败条目之前（下一拍重拉重试，agent loop 就绪后自然补投），
/// 且永不越过 `latest`（`fail-1` 可能大于 latest 当响应乱序时）。
fn sync_advance_target(first_undelivered_seq: Option<i64>, latest: i64) -> i64 {
    match first_undelivered_seq {
        Some(fail) => (fail - 1).min(latest),
        None => latest,
    }
}

/// G8 过滤：补拉条目只入队「@我的 / 我参与的线程」。@ 匹配与 master
/// 裁决器同源（[`nemesis_board::arbitrator::mentions_node`]：id/name 精准，
/// @role: 命中本节点拓扑角色或功能类别——@role:qa fan-out 时离线的节点
/// 靠这条兜回来）。自己的发言不唤醒自己。
fn sync_entry_targets_me(
    deps: &WorkerBusDeps,
    content: &str,
    sender: &str,
    thread_key: &str,
) -> bool {
    if sender == deps.self_node_id {
        return false;
    }
    deps.wake_state.is_participated(thread_key)
        || mentions_node(
            content,
            &deps.self_node_id,
            &deps.node_name,
            &deps.node_role,
            &deps.node_category,
        )
}

// ======================================================================
// 集群任务认领（board.claimable.list / board.claim，2026-10-22 计划 §3）
//
// 讨论组「认领制」的 master 侧：worker 上行浏览待领单 / 认领。原子置位
// （store.claim_issue）→ 派发单一入口落队（dispatch_issue_core，estop/
// 冻结/重复派发/父单计划/RPC 全闸与预算保险丝自然继承）→ 播报双落
// （issue 系统评论 + #team 频道）。硬闸与派发器完全同源（rank_peers
// 严格档打单个候选）——松弛是派发器兜底的特权，自评认领不松弛（Q2）。
// ======================================================================

/// `board.claimable.list`：worker 上行浏览待领单（Q3：全量可领单 +
/// 硬闸满足标记）。资格闸不过的单直接不出现；硬闸不过的单带
/// `blocked_reason` 照常出现（LLM 自评「谁看合适」的依据）。
fn handle_claimable_list(
    deps: &MasterBusDeps,
    env: &Envelope,
    from_node: &str,
) -> EnvelopeResponse {
    if from_node.is_empty() {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(
                envelope::error_code::VALIDATION,
                "missing _rpc.from (caller identity)",
            ),
        );
    }
    let limit = env
        .body
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .clamp(1, 200) as usize;
    let filter = nemesis_board::IssueFilter {
        status: None,
        assignee: None,
        project_id: None,
        priority: None,
        query: None,
        exclude_archived_projects: false,
        exclude_cancelled: false,
        exclude_hidden: false,
    };
    let all = match deps.store.list_issues(&filter) {
        Ok(v) => v,
        Err(e) => {
            return EnvelopeResponse::failure(
                env,
                EnvelopeError::new(envelope::error_code::INTERNAL, format!("读取看板失败: {e}")),
            );
        }
    };
    // 认领人投影（注册表缺席 = 全部标 ineligible + 原因——浏览不硬拒）。
    let claimer = deps
        .cluster
        .get_peer(from_node)
        .map(|info| claimer_candidate(&info));
    let mut items = Vec::new();
    for issue in all {
        // 资格闸 Err = 状态/被领/父单/在途派发/依赖未完，browse 一律跳过。
        if deps.store.claimable_check(issue.id).is_err() {
            continue;
        }
        let (eligible, blocked_reason) = match &claimer {
            Some(c) => match claim_gate(&issue, c) {
                Ok(()) => (true, None),
                Err(reason) => (false, Some(reason)),
            },
            None => (
                false,
                Some(format!(
                    "本节点（{from_node}）不在 master 节点注册表，无法认领"
                )),
            ),
        };
        items.push(serde_json::json!({
            "issue_id": issue.id,
            "number": issue.number,
            "title": issue.title,
            "priority": issue.priority,
            "status": issue.status.as_str(),
            "required_role": issue.required_role,
            "required_profession": issue.required_profession,
            "eligible": eligible,
            "blocked_reason": blocked_reason,
        }));
        if items.len() >= limit {
            break;
        }
    }
    let count = items.len();
    EnvelopeResponse::success(env, serde_json::json!({ "items": items, "count": count }))
}

/// `board.claim`：worker 上行认领（§3.4 动作序列）。全程快速同步——
/// 派发落队走 dispatch_issue_core 内部既有异步链，handler 不等投递。
fn handle_claim(deps: &MasterBusDeps, env: &Envelope, from_node: &str) -> EnvelopeResponse {
    // ① 身份：RPC server 注入 `_rpc.from`，空 = 伪造面/本地误调。
    if from_node.is_empty() {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(
                envelope::error_code::VALIDATION,
                "missing _rpc.from (caller identity)",
            ),
        );
    }
    // ② master 本尊不认领（本尊直写 store，与 board_discuss 不装 master 同理）。
    if from_node == deps.cluster.node_id() {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(
                envelope::error_code::VALIDATION,
                "master 节点不走认领取件（本尊可直接操作看板）",
            ),
        );
    }
    // ③ 解析 issue 引用（纯数字 = 内部 id，否则按人读编号）。
    let Some(issue_ref) = env
        .body
        .get("issue")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, "missing issue"),
        );
    };
    let issue = match resolve_issue_ref(&deps.store, issue_ref) {
        Ok(i) => i,
        Err(msg) => {
            return EnvelopeResponse::failure(
                env,
                EnvelopeError::new(envelope::error_code::VALIDATION, msg),
            );
        }
    };
    // ④ 资格闸（browse/claim 同源单一真相源；拒绝原因即 claimable_check 文案）。
    if let Err(msg) = deps.store.claimable_check(issue.id) {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, msg),
        );
    }
    // ⑤ 认领人必须在节点注册表（在线宣告过身份的节点才谈得上自评硬闸）。
    let Some(info) = deps.cluster.get_peer(from_node) else {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(
                envelope::error_code::VALIDATION,
                format!("节点 {from_node} 不在 master 节点注册表，无法认领"),
            ),
        );
    };
    let claimer = claimer_candidate(&info);
    // ⑥ 硬闸（Q2：强制 profession/tier 匹配、严格不松弛、原因明示）。
    if let Err(reason) = claim_gate(&issue, &claimer) {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, reason),
        );
    }
    // ⑦ 原子认领（先到先得：双 worker 并发 claim 恰好一胜）。
    let actor = Actor::new("agent", from_node);
    if let Err(msg) = deps.store.claim_issue(issue.id, from_node, &actor) {
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(envelope::error_code::VALIDATION, msg),
        );
    }
    // ⑧ 派发落队（单一入口，全闸继承）。dispatch_issue_core 的全部 Err
    // 路径都在状态转移之前 → 回滚只需还原 assignee，单子回到待领池。
    // 守卫式回滚（rollback_claim）：仅当 assignee 仍是本认领者时清空——
    // claim→dispatch 窗口内并发派发赢家（手动派发/补派抢先占用并回填
    // assignee）的指派不得被无条件清空踩掉（字段与事实脱节回归）。
    if let Err(e) = nemesis_web::handlers::board::dispatch_issue_core(
        &deps.store,
        Some(&deps.cluster),
        issue.id,
        from_node,
        &actor,
        None,
    ) {
        match deps
            .store
            .rollback_claim(issue.id, from_node, &Actor::system("board"))
        {
            Ok(true) => {}
            Ok(false) => tracing::info!(
                target: "board_bus",
                issue = %issue.number,
                "[nb_bus] claim dispatch failed but assignee already moved on, rollback skipped (issue belongs to the concurrent dispatch winner)"
            ),
            Err(re) => tracing::warn!(
                target: "board_bus",
                issue = %issue.number,
                error = %re,
                "[nb_bus] claim rollback failed"
            ),
        }
        tracing::warn!(
            target: "board_bus",
            node = %from_node,
            issue = %issue.number,
            error = %e,
            "[nb_bus] claim dispatch failed, assignee rolled back"
        );
        return EnvelopeResponse::failure(
            env,
            EnvelopeError::new(
                envelope::error_code::UNAVAILABLE,
                format!("认领后派发失败，已回滚（单子回到待领池）：{e}"),
            ),
        );
    }
    // ⑨ 播报双落（best-effort：播报缺失不阻断主链——认领已成立）。
    announce_claim(&deps.store, &issue, from_node);
    tracing::info!(
        target: "board_bus",
        node = %from_node,
        issue = %issue.number,
        "[nb_bus] claim accepted"
    );
    EnvelopeResponse::success(
        env,
        serde_json::json!({
            "claimed": true,
            "issue_id": issue.id,
            "number": issue.number,
            "title": issue.title,
        }),
    )
}

/// issue 引用解析：纯数字 = 内部 id，否则按人读编号（NB-15）。
fn resolve_issue_ref(
    store: &Arc<nemesis_board::BoardStore>,
    issue_ref: &str,
) -> Result<nemesis_board::Issue, String> {
    if let Ok(id) = issue_ref.parse::<i64>() {
        return store.get_issue(id);
    }
    store.get_issue_by_number(issue_ref)
}

/// 认领人投影：ExtendedNodeInfo → PeerCandidate（与 web 侧
/// project_dispatch_candidates 同投影：role 词表化、tags ∪ category、
/// professions/tier/runtime_backend 直传）。
fn claimer_candidate(
    info: &nemesis_cluster::types::ExtendedNodeInfo,
) -> nemesis_board::PeerCandidate {
    nemesis_board::PeerCandidate {
        id: info.base.id.clone(),
        name: info.base.name.clone(),
        role: match info.base.role {
            nemesis_types::cluster::NodeRole::Coordinator => "coordinator",
            nemesis_types::cluster::NodeRole::Worker => "worker",
        }
        .to_string(),
        tags: {
            let mut t = info.tags.clone();
            let c = info.base.category.trim();
            if !c.is_empty() && !t.iter().any(|x| x.eq_ignore_ascii_case(c)) {
                t.push(c.to_string());
            }
            t
        },
        capabilities: info.capabilities.clone(),
        professions: info.professions.clone(),
        tier: info.tier.clone(),
        runtime_backend: info.runtime_backend.clone(),
    }
}

/// Issue → matcher 入参（与派发匹配面同字段投影）。
fn claim_match_input(issue: &nemesis_board::Issue) -> nemesis_board::MatchInput<'_> {
    nemesis_board::MatchInput {
        required_role: issue.required_role.as_deref(),
        required_tags: &issue.required_tags,
        required_profession: issue.required_profession.as_deref(),
        required_capabilities: &issue.required_capabilities,
        required_backend: issue.required_backend.as_deref(),
        description: &issue.description,
    }
}

/// 认领硬闸（Q2 拍板：严格不松弛）。判定与派发器完全同源：
/// [`nemesis_board::rank_peers`] 严格档打单个候选，空 = 硬条件 1-6 至少
/// 一条不过。Err 文案 = [`claim_gate_reason`] 人读逐条对照。
fn claim_gate(
    issue: &nemesis_board::Issue,
    claimer: &nemesis_board::PeerCandidate,
) -> Result<(), String> {
    let input = claim_match_input(issue);
    let pass = !nemesis_board::rank_peers(&input, std::slice::from_ref(claimer), &HashMap::new())
        .is_empty();
    if pass {
        Ok(())
    } else {
        Err(claim_gate_reason(&input, claimer))
    }
}

/// 硬闸不符的人读原因（逐条对照 rank_peers 硬条件 1-6；Q2 拍板「拒绝
/// 原因明示」，如「需职能 dev:cpp，本节点 [ui-design]」）。收集全部不符
/// 项（一次看清为什么不行，不首条即停）。
fn claim_gate_reason(
    input: &nemesis_board::MatchInput<'_>,
    peer: &nemesis_board::PeerCandidate,
) -> String {
    let mut reasons: Vec<String> = Vec::new();
    // 硬条件 1：角色。
    if let Some(need) = input.required_role.map(str::trim).filter(|s| !s.is_empty())
        && !peer.role.eq_ignore_ascii_case(need)
    {
        reasons.push(format!("需角色 {need}，本节点 {}", peer.role));
    }
    // 硬条件 2：标签交集。
    if !input.required_tags.is_empty()
        && !input
            .required_tags
            .iter()
            .any(|t| peer.tags.iter().any(|pt| pt.eq_ignore_ascii_case(t)))
    {
        reasons.push(format!(
            "需标签 [{}]，本节点 [{}]",
            input.required_tags.join(","),
            peer.tags.join(",")
        ));
    }
    // 硬条件 3：职能精确 + 硬条件 4：tier 门槛（只挂内置目录职能）。
    if let Some(need) = input
        .required_profession
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let need = nemesis_prompts::professions::meta::normalize_slug(need);
        if !peer
            .professions
            .iter()
            .any(|p| nemesis_prompts::professions::meta::normalize_slug(p) == need)
        {
            reasons.push(format!(
                "需职能 {need}，本节点 [{}]",
                peer.professions.join(",")
            ));
        }
        if let Some(meta) = nemesis_prompts::professions::meta::find_builtin(&need)
            && !nemesis_prompts::professions::meta::tier_allows(peer.tier.as_deref(), meta.min_tier)
        {
            reasons.push(format!(
                "职能 {need} 最低档 {}，本节点档 {}",
                meta.min_tier,
                peer.tier.as_deref().unwrap_or("未宣告")
            ));
        }
    }
    // 硬条件 5：能力全量命中（列出缺失项）。
    if !input.required_capabilities.is_empty() {
        let missing: Vec<&str> = input
            .required_capabilities
            .iter()
            .map(String::as_str)
            .map(str::trim)
            .filter(|need| {
                !need.is_empty()
                    && !peer
                        .capabilities
                        .iter()
                        .any(|c| c.trim().eq_ignore_ascii_case(need))
            })
            .collect();
        if !missing.is_empty() {
            reasons.push(format!(
                "缺能力 [{}]，本节点 [{}]",
                missing.join(","),
                peer.capabilities.join(",")
            ));
        }
    }
    // 硬条件 6：backend 精确（未宣告 = fail-closed，同 rank_peers）。
    if let Some(need) = input
        .required_backend
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && !peer
            .runtime_backend
            .as_deref()
            .is_some_and(|b| b.trim().eq_ignore_ascii_case(need))
    {
        reasons.push(format!(
            "需后端 {need}，本节点 {}",
            peer.runtime_backend.as_deref().unwrap_or("未宣告")
        ));
    }
    // 理论不可达（rank_peers 空 = 至少一条硬闸不过）；诚实兜底不瞎编。
    if reasons.is_empty() {
        return "节点硬条件不满足（详见节点宣告与单子需求）".to_string();
    }
    reasons.join("；")
}

/// 认领播报双落（§3.4-4）：issue 系统评论（权威留痕）+ `#team` 频道系统
/// 消息（Q4 团队频道）。全 best-effort：认领与派发在播报前已成立，播报
/// 失败只 WARN 不回滚。
fn announce_claim(
    store: &Arc<nemesis_board::BoardStore>,
    issue: &nemesis_board::Issue,
    claimer: &str,
) {
    let text = format!("🙋 {} 已被 {claimer} 认领", issue.number);
    // ① issue 系统评论（决策/活动可追溯）。
    if let Err(e) = store.add_comment(nemesis_board::NewComment {
        issue_id: issue.id,
        author: Actor::system("board"),
        content: text.clone(),
        parent_id: None,
        ctype: nemesis_board::CommentType::System,
    }) {
        tracing::warn!(
            target: "board_bus",
            issue = %issue.number,
            error = %e,
            "[nb_bus] claim 系统评论落库失败"
        );
    }
    // ② #team 频道播报；频道缺席补种（INSERT OR IGNORE 幂等，防手工删
    // 频道的库），仍失败只少一条播报。
    let channel = match store.get_channel_by_name(CLAIM_ANNOUNCE_CHANNEL) {
        Ok(Some(ch)) => Some(ch),
        Ok(None) => match store
            .ensure_default_channels()
            .and_then(|_| store.get_channel_by_name(CLAIM_ANNOUNCE_CHANNEL))
        {
            Ok(Some(ch)) => Some(ch),
            _ => {
                tracing::warn!(
                    target: "board_bus",
                    "[nb_bus] #team 频道不可用，认领播报仅落 issue 评论"
                );
                None
            }
        },
        Err(e) => {
            tracing::warn!(
                target: "board_bus",
                error = %e,
                "[nb_bus] 读取 #team 频道失败，认领播报仅落 issue 评论"
            );
            None
        }
    };
    if let Some(ch) = channel
        && let Err(e) = store.append_channel_message(nemesis_board::NewChannelMessage {
            channel_id: ch.id,
            sender: Actor::system("board"),
            content: text,
            parent_id: None,
            mtype: nemesis_board::channel_message_type::SYSTEM.to_string(),
        })
    {
        tracing::warn!(
            target: "board_bus",
            issue = %issue.number,
            error = %e,
            "[nb_bus] claim 频道播报失败"
        );
    }
}

/// 认领播报落点频道（Q4 拍板：#team = 团队）。
const CLAIM_ANNOUNCE_CHANNEL: &str = "#team";

#[cfg(test)]
mod tests;
