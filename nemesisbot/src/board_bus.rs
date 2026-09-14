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
        "sync" => handle_sync(deps, &env),
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
        Ok(false) => EnvelopeResponse::failure(
            env,
            EnvelopeError::new(
                envelope::error_code::VALIDATION,
                format!("task {task_id} 不在 dispatched 态或 worker 不匹配"),
            ),
        ),
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
        let stored_name = format!("{}_{}", chrono::Utc::now().timestamp_millis(), safe_name);
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
/// 立即返回首响，投递不占 ACK）。worker RPC 上行（[`handle_comment_post`]）
/// 与 dashboard 本地发言（[`post_discussion_locally`]）共用，G3/G12 语义同源。
fn post_discussion_core(
    deps: &MasterBusDeps,
    sender: &Actor,
    thread_kind: &str,
    thread_id: i64,
    client_msg_id: &str,
    content: &str,
    reply_to: Option<i64>,
    kind_tag: &str,
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
    tokio::spawn(async move {
        deliver_wakeups(deps_for_task, ctx, plan, nodes).await;
    });

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
    // 给自己发 wake.post）。
    let mut need_moderator = plan.to_moderator;
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

    // 规则③（或主持人被直接点名）：主持人裁决（master 本地 agent；回复
    // 照常落库并定点唤醒被 @ 的节点）。额度已耗尽时不进裁决（见上方 return）。
    if need_moderator && let Err(e) = run_moderator(deps, ctx, &nodes, &self_node_id).await {
        tracing::warn!(target: "board_bus", thread = %thread_key, error = %e,
            "[nb_bus] moderator adjudication failed");
    }
}

/// 主持人裁决：线程上下文 + 新消息 → 主 AgentLoop 直调一轮 → 解析回复
/// （[SILENT] / @tokens + 正文）→ 正文落库（origin=主持人）+ @目标唤醒。
///
/// §9.6 注意点 1：主 agent 人格 system prompt 可能污染结构化输出——解析
/// 从宽（全文扫 @token / [SILENT] 子串），不做严格格式断言。
async fn run_moderator(
    deps: DepsForTask,
    ctx: WakeContext,
    nodes: &[NodeCandidate],
    self_node_id: &str,
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
    let prompt = format!(
        "{thread_ctx}\n\n\
         # Node roster (you may @mention nodes by name to call on them):\n{roster}\n\n\
         # New message from {}:\n{}\n\n\
         You are the discussion moderator. Decide whether this needs your action:\n\
         - If someone should be called on, reply with your message using @node-name to mention them.\n\
         - If nothing needs to be said, reply exactly [SILENT].",
        ctx.sender.id, ctx.content
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
        content: String,
        at: i64,
    }
    let mut out: Vec<CtxMessage> = Vec::new();
    if ctx.thread_kind == thread_kind::ISSUE {
        for c in store.list_comments(ctx.thread_id)? {
            out.push(CtxMessage {
                sender: c.author.id,
                content: c.content,
                at: c.created_at,
            });
        }
    } else {
        for m in store.list_channel_messages(ctx.thread_id, 0, i64::MAX)? {
            out.push(CtxMessage {
                sender: m.sender.id,
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
                m.get("sender").and_then(|v| v.as_str()).unwrap_or("?"),
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
        let event = DiscussionEvent {
            thread_kind: kind.to_string(),
            thread_id: tid,
            thread_title: String::new(),
            messages: Vec::new(),
            event: "sync_backfill".to_string(),
            from_node: target.clone(),
            new_sender: sender.to_string(),
            new_content: content.to_string(),
            new_at: at,
            reply_to: Some(seq),
            max_turns_left: u32::MAX,
            seq,
        };
        if deps.inbox.send(event).is_ok() {
            deps.wake_state.commit(&thread_key, seq);
            enqueued += 1;
        }
    }
    // 游标推进到 master latest_seq：见过的（含被过滤的）不重拉。
    deps.wake_state.advance_watermark(latest);
    Ok(enqueued)
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

#[cfg(test)]
mod tests;
