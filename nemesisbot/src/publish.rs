//! 外循环发布 + unknown 对账（宿主通用骨架，域协议声明化，2026-11-02）：
//! 交付分支推远端 + 发布条目创建 + 三分支状态机（成功记外部引用 / 确定失败
//! 诚实可重试 / 未知回读对账）。
//!
//! **域知识边界（外循环域剥离批 C-3）**：本模块只保留发布通用骨架——git 半
//! 边、前置核验、账本、对账状态机；「创建什么」（PR / MR / 任意远端条目）的
//! op 名与 args 形状是域知识，只由 flow 模板 [`PublishProtocol`] 声明带入。
//! 宿主无内置发布协议：protocol 缺席 = 诚实 `Err`。
//!
//! 发布链（全部在 master 侧，操作对象 = 任务副本）：
//! 1. **前置核验**：`{task_id}:branch` 交付行在场、非 rejected、payload 无
//!    external_ref（幂等——已发布直接回 already_published）；副本 HEAD ==
//!    payload.repo_base_sha（版本绑定的发布侧消费——基线漂移拒绝发布，绝不
//!    把 patch 糊到漂移基线上）；内联 patch 在场且未截断。
//! 2. **git 半边**：副本仓内 HEAD 建分支 `nemesis/task-<task_id>` + 应用
//!    patch + commit（[`nemesis_board::git_repo:apply_patch_new_branch`]，
//!    libgit2 原生零外部 git）→ `push_branch` 推 origin（vault PAT，同
//!    clone 凭据语义；push 有 TCP 确认，失败重推幂等——同内容快进）。
//! 3. **发布条目创建**：protocol.create_op + create_args（模板声明，运行期
//!    仅补 `{{commit}}`/`{{base_sha}}` 等运行时 token；走主 loop
//!    handle_tool_call 完整 dispatch 瀑布——estop/审批/安全 8 层全生效）。
//! 4. **unknown 对账**：create 结果解析不出引用（超时/传输断/HTTP 错误/
//!    截断）→ protocol.reconcile_op 按声明翻页回查，`reconcile_match` 比较式
//!    逐条匹配——命中 = 实际已建成（记账收敛为成功，不瞎重试）；扫尽无命中
//!    = 确定失败（错误原文留 detail，可重试：重试先撞 payload 幂等再撞对
//!    账，不重复建条目）。
//! 5. **落账**：`mark_delivery_published_with_payload`（published 终态 +
//!    payload 单事务同落）+ external_ref/external_url/published_*。
//!
//! 续作留痕（2026-11-02 复评 移植）：交付 payload 的 `publish_stage`
//! 单调记账（commit_generated → push_completed → pr_pending → published，
//! **只前进不后退**——pr_pending 续作跳过中间态写入，防把现场覆盖回退）。
//! push/创建阶段失败后的重试凭此续作：分支已存在走内容级幂等复用（git 层
//! 探针 + payload tree 锚双层）；到达创建阶段（pr_pending）的重试**先对账
//! 再决定**——已建成收敛落账，扫尽不在才重建，扫不尽保守失败。
//!
//! 诚实边界：副本基线核验只认 repo_base_sha（旧形态交付缺该键 = 拒绝
//! 发布，不静默放行）；对账翻页有硬上限（页数封顶 = Inconclusive 保守判
//! 失败，绝不冒充「扫尽不在」）。
//!
//! **op 路由双臂（2026-11-04 插件自持a）**：create_op/reconcile_op
//! 各自可走 connector 原语（`connector_invoke` 工具面）或
//! `plugin:<slug>:<op>` guest 步引用（`run_flow_step`，与 on-transition/
//! report 形态同源）——connector 字段随之转可选（仅 Native 形态需要）；
//! git 推送凭据别名从 create_op 路由推导（Plugin → `plugin/<slug>/pat`，
//! Native → `connector/<id>/pat`）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::AgentLoop;
use nemesis_board::store::BoardStore;
use nemesis_taskstore::TaskDb;
use serde_json::{Value, json};

/// 发布终态词表（响应 `status` 字段）。
const ST_PUBLISHED: &str = "published";
const ST_ALREADY: &str = "already_published";
const ST_FAILED: &str = "failed";

/// 「请求确定未发出」错误标记（插件自持a）：plugin 路由层失败（插件
/// 未装配/停用/词表对账不过）与 dispatch 层拒（`⛔` 前缀）同语义——都不
/// 可能产生远端副作用，重试安全。guest 四态 failed/running/malformed =
/// guest 已执行过（可能已发 HTTP），**不带**本标记 → 走对账保守分支。
const NOT_SENT_MARKER: &str = "[not-sent]";

/// plugin 子系统句柄类型别名（R8 复评整改）：wasm 形态 = 真注册表类型；
/// 非 wasm 裁剪形态 = 空枚举（**不可构造**——`Option<&Arc<_>>` 只能是
/// `None`，plugin 路由臂恒走「子系统未编译」诚实失败）。调用点签名
/// （publish_task_delivery / invoke_route / reconcile_by_match）以别名
/// 书写，两形态零改动。
#[cfg(feature = "plugins-wasm")]
type PublishPluginManager = nemesis_plugins_wasm::registry::PluginManager;
#[cfg(not(feature = "plugins-wasm"))]
enum PublishPluginManager {}

/// 发布协议声明（flow 模板 publish 步 args 解析产物；哑结构——全部是渲染
/// 后的字面量，宿主不做任何域解释）。域剥离批 C-3：创建/对账的 op 名、
/// args 形状、结果引用路径、匹配比较式全部来自模板声明。
#[derive(Debug)]
pub(crate) struct PublishProtocol {
    /// 创建发布条目的 connector op（如 `create_pull`——域知识在模板）。
    pub create_op: String,
    /// 创建 op 的请求体（模板声明 + parse 期 token 渲染；值恒为字符串）。
    pub create_args: BTreeMap<String, String>,
    /// 结果 JSON 中取「外部条目号」的 JSON path（`$.number` 形态）。
    pub result_ref: String,
    /// 结果 JSON 中取「人读链接」的 JSON path（缺席 = 不取链接）。
    pub result_url: Option<String>,
    /// unknown 对账的 connector op（如 `pr_list`）。
    pub reconcile_op: String,
    /// 对账 op 的请求体基参（翻页键由宿主按页注入/覆盖）。
    pub reconcile_args: BTreeMap<String, String>,
    /// 对账逐条匹配比较式（`$.head.ref == nemesis/task-x` 形态；`$.` 侧
    /// 按条目 JSON path 取标量，另一侧字面量）。
    pub reconcile_match: String,
    /// 对翻页 limit 参数键名（缺省 `limit`）。
    pub reconcile_limit_key: String,
    /// 对翻页 page 参数键名（缺省 `page`）。
    pub reconcile_page_key: String,
    /// 每页条数（缺省 20）。
    pub reconcile_limit: usize,
    /// 页数硬上限（缺省 5；封顶 = Inconclusive）。
    pub reconcile_max_pages: usize,
    /// Gerrit 臂提交信息模板（可选；push_ref 以 `refs/for/` 开头时消费）。
    /// parse 期已渲染条目 token（ext_key/title/board_issue/report_path），
    /// 发布侧 rt 期再渲染 change_id/branch/task_id；模板未带 Change-Id
    /// footer 由发布侧补齐（Gerrit 缺 footer 拒收）。
    pub commit_message: Option<String>,
}

impl Default for PublishProtocol {
    fn default() -> Self {
        Self {
            create_op: String::new(),
            create_args: BTreeMap::new(),
            result_ref: String::new(),
            result_url: None,
            reconcile_op: String::new(),
            reconcile_args: BTreeMap::new(),
            reconcile_match: String::new(),
            reconcile_limit_key: "limit".to_string(),
            reconcile_page_key: "page".to_string(),
            reconcile_limit: 20,
            reconcile_max_pages: 5,
            commit_message: None,
        }
    }
}

/// 发布 op 路由（插件自持a）：connector 原语 op（走
/// `connector_invoke` 工具面）或 `plugin:<slug>:<op>` guest 步引用（走
/// `run_flow_step`——与 on-transition/report 形态 100% 同源）。创建与对账
/// 可各自选臂（混搭合法：创建走 guest、对账走 connector 均是合法声明）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum OpRoute {
    Native(String),
    Plugin { slug: String, op: String },
}

/// `plugin:<slug>:<op>` 步骤引用解析（cfg 双版，R8 复评整改）：wasm 形态
/// 委托单一真相源 [`nemesis_plugins_wasm::manifest::parse_plugin_step_ref`]；
/// 非 wasm 裁剪形态用本地等价实现（纯字符串解析——`plugin:` 前缀 + slug/op
/// 两段，slug 段 `[a-z0-9-]{1,64}` 不以 - 开头结尾且不为 `all`，op 段非空
/// ≤64 字节无空白/控制字符；漂移由 wasm 形态单测钉住两版对若干形态同判）。
/// 非 wasm 形态解析仍然 full-fidelity：声明坏 loud 拒的运行期契约不因裁剪
/// 缺席而放宽。
#[cfg(feature = "plugins-wasm")]
fn parse_plugin_step_ref(kind: &str) -> Option<(&str, &str)> {
    nemesis_plugins_wasm::manifest::parse_plugin_step_ref(kind)
}

#[cfg(not(feature = "plugins-wasm"))]
fn parse_plugin_step_ref(kind: &str) -> Option<(&str, &str)> {
    fn valid_slug(slug: &str) -> bool {
        (1..=64).contains(&slug.len())
            && slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !slug.starts_with('-')
            && !slug.ends_with('-')
            && slug != "all"
    }
    fn valid_step_op(op: &str) -> bool {
        !op.is_empty()
            && op.len() <= 64
            && op.chars().all(|c| !c.is_whitespace() && !c.is_control())
    }
    let rest = kind.strip_prefix("plugin:")?;
    let (slug, op) = rest.split_once(':')?;
    if !valid_slug(slug) || !valid_step_op(op) {
        return None;
    }
    Some((slug, op))
}

/// 路由解析（单点）：`plugin:` 前缀经 `parse_plugin_step_ref` 判定，解析
/// 失败 = 声明坏 loud 拒（不给运行期留「当原语名静默错」黑洞）；其余 =
/// Native 自由文本（词表裁决在 connector 侧，宿主不域解释）。
pub(crate) fn parse_op_route(op: &str) -> Result<OpRoute, String> {
    if let Some((slug, step_op)) = parse_plugin_step_ref(op) {
        return Ok(OpRoute::Plugin {
            slug: slug.to_string(),
            op: step_op.to_string(),
        });
    }
    if op.trim().starts_with("plugin:") {
        return Err(format!(
            "publish op '{op}' 是 plugin 引用形态但解析失败（需 plugin:<slug>:<op>，段 [a-z0-9_-]+）"
        ));
    }
    Ok(OpRoute::Native(op.trim().to_string()))
}

/// git 推送凭据别名推导（单点，从 **create_op 路由**推导——推送是宿主侧
/// git_repo 动作，凭据供给跟随创建方身份；对账臂不涉推送不取凭据）：
/// Plugin → `plugin/<slug>/pat`（自持插件 PAT）；Native →
/// `connector/<id>/pat`（connector 原语语义）。
pub(crate) fn credential_alias(
    connector: Option<&str>,
    create_route: &OpRoute,
) -> Result<String, String> {
    match create_route {
        OpRoute::Plugin { slug, .. } => Ok(format!("plugin/{slug}/pat")),
        OpRoute::Native(_) => connector
            .map(|c| format!("connector/{c}/pat"))
            .ok_or_else(|| {
                "connector 原语 create_op 缺 connector 声明（推送凭据别名无法推导）".to_string()
            }),
    }
}

/// 交付发布桥实现（nemesis-web 侧 trait + 槽，依赖反转同 deliveries 查询面）。
pub(crate) struct DeliveryPublisher {
    db: Arc<TaskDb>,
    store: Arc<BoardStore>,
    agent_loop: Arc<AgentLoop>,
    workspace: PathBuf,
}

/// gateway post_agent 装配（幂等覆盖；与 flow 引擎同闸同点——
/// board+security+connectors 三 feature）。
pub(crate) fn install(
    db: Arc<TaskDb>,
    store: Arc<BoardStore>,
    agent_loop: Arc<AgentLoop>,
    workspace: PathBuf,
) {
    nemesis_web::handlers::deliveries::install_delivery_publish_bridge(Arc::new(
        DeliveryPublisher {
            db,
            store,
            agent_loop,
            workspace,
        },
    ));
}

#[async_trait::async_trait]
impl nemesis_web::handlers::deliveries::DeliveryPublishBridge for DeliveryPublisher {
    async fn publish(&self, args: &Value) -> Result<Value, String> {
        // 域剥离批 C-3：WSAPI 桥无 flow 模板声明面，协议缺席 → 诚实报错
        // （域知识只在 flow 模板；dashboard 发布表单随域退役并处理）。
        publish_task_delivery(
            &self.db,
            &self.store,
            &self.agent_loop,
            &self.workspace,
            args,
            None,
            None,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// 编排
// ---------------------------------------------------------------------------

/// 前置核验产物（同步、可单测——不触碰 agent loop）。
#[derive(Debug)]
enum Preflight {
    /// 幂等短路：payload 已带 external_ref，直接回 already_published 响应。
    Already(Value),
    Proceed {
        copy_dir: PathBuf,
        patch: String,
        expected_base: String,
        title: String,
        /// 第三臂放行：交付已并入副本 main（ingest 先于 publish 的常态时序）
        /// ——git 半边直接从 HEAD 建分支，patch 免再套。
        already_merged: bool,
        /// 交付内容树（payload `tree` 原样透传；缺席/空 = None）——git 半边
        /// 重试幂等判断的等值锚（分支树 == 交付树 → 上次产物复用）。
        delivery_tree: Option<String>,
        /// 交付基线树（payload `base_tree` 透传；缺席/空 = None）——Gerrit
        /// 臂三方合并的基树锚（缺席时回落基线提交的树）。
        base_tree: Option<String>,
        /// 交付 payload 的 `publish_stage`（续作留痕）：`pr_pending` =
        /// 上次已到达发布条目创建阶段（进程崩溃/落账失败），重试先对账防
        /// 重复建条目；其余值（commit_generated/push_completed/published/
        /// 缺省）走常规路径。
        stage: Option<String>,
        /// 意图已前进（R1 复评整改）：payload 已带非空 `published_commit`
        /// 或 `published_branch`——发布意图曾真实落盘（git 半边已完成）。
        /// 与 `stage` 解耦判定续作形态：旧账本/半行账（stage 缺失但意图
        /// 在场）同样走对账先行的保守路径，不再单点信任 stage 字段。
        intent_advanced: bool,
        /// Gerrit 重整锚（payload `recast_tree` 透传；缺席/空 = None，R3
        /// 复评整改）：上次 Gerrit 臂重整产物的树——重试凭「分支树 ==
        /// 重整锚」幂等复用重整 commit，不再被「分支树 ≠ 交付树」假失败
        /// 卡死（patch 套在重整内容上必炸）。
        recast_tree: Option<String>,
    },
}

/// 结构化失败响应（发布结果是第一公民结果非传输错误）。
fn fail_outcome(task_id: &str, branch: &str, detail: String) -> Value {
    json!({
        "task_id": task_id,
        "status": ST_FAILED,
        "external_ref": Value::Null,
        "external_url": Value::Null,
        "commit": Value::Null,
        "branch": branch,
        "detail": detail,
    })
}

/// 前置核验（① 交付行核验 + ② 副本/基线核验 + patch 提取 + 标题解析）。
/// 参数合法性（task_id/connector/owner/repo）由调用方先验后传入——
/// 本函数 `Err` 全部是「确定失败可重试」的前置失败面（编排层包
/// fail_outcome），不再混入请求形态错误。
fn preflight(
    db: &Arc<TaskDb>,
    store: &Arc<BoardStore>,
    task_id: &str,
    args: &Value,
) -> Result<Preflight, String> {
    validate_task_id_for_branch(task_id)?;
    let branch = format!("nemesis/task-{task_id}");
    let branch_key = format!("{task_id}:branch");

    let rec = db
        .delivery_by_key(&branch_key)?
        .ok_or_else(|| format!("无分支交付记录: {branch_key}（任务未走交付管线，无可发布物）"))?;
    if rec.kind != "branch" {
        return Err(format!(
            "交付行 {branch_key} kind={} ≠ branch——键冲突，拒绝发布",
            rec.kind
        ));
    }
    if rec.status == "rejected" {
        return Err(PREFLIGHT_REJECTED.to_string());
    }
    if let Some(n) = rec.payload.get("external_ref").and_then(|v| v.as_i64()) {
        return Ok(Preflight::Already(json!({
            "task_id": task_id,
            "status": ST_ALREADY,
            "external_ref": n,
            "external_url": rec.payload.get("external_url").cloned().unwrap_or(Value::Null),
            "commit": rec.payload.get("published_commit").cloned().unwrap_or(Value::Null),
            "branch": rec
                .payload
                .get("published_branch")
                .cloned()
                .unwrap_or(Value::String(branch)),
            "detail": "该交付已发布（幂等跳过，不重复建条目）",
        })));
    }

    // 副本与基线核验。基线等值双臂：commit 等值（主仓直捕路径）或 tree
    // 等值（exec 捕获路径——repo_base_sha 是 worker 侧 exec 仓的 fresh-init
    // 基线 commit，主仓任何 commit 都对不上；patch 是 tree-diff，副本 HEAD
    // 内容态 == 交付基线内容态即可干净套用，`base_tree` 随 wire 同步落账）。
    let copy_dir = resolve_copy_dir(store, task_id, args.get("copy_path"))?;
    let head = match nemesis_board::git_repo::head_commit_hex(&copy_dir) {
        Ok(Some(h)) => h,
        Ok(None) => return Err(PREFLIGHT_EMPTY_REPO.to_string()),
        Err(e) => return Err(e),
    };
    let expected_base = rec
        .payload
        .get("repo_base_sha")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PREFLIGHT_NO_BASE_SHA.to_string())?
        .to_string();
    let expected_tree = rec
        .payload
        .get("base_tree")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);
    let head_tree = nemesis_board::git_repo::head_tree_hex(&copy_dir)?.unwrap_or_default();
    let base_match = head == expected_base
        || expected_tree
            .as_deref()
            .is_some_and(|et| !head_tree.is_empty() && et == head_tree);
    // 第三臂（真链发现）：交付已并入副本（ingest 在任务完成时即
    // 合并进副本 main，publish 人工闸在后是常态时序）——副本内容态等值交付
    // 内容态（交付 payload `tree` = worker 交付后内容树，随 checks_binds.head_tree
    // 同源落账）时 patch 再套必失败：发布分支直接取 HEAD 内容。等值是内容级
    // 判定，与 patch 路径产出的分支树一致（基线+patch == 交付树）。
    let delivery_tree = rec
        .payload
        .get("tree")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);
    let already_merged = !base_match
        && delivery_tree
            .as_deref()
            .is_some_and(|dt| !head_tree.is_empty() && dt == head_tree);
    if !base_match && !already_merged {
        return Err(base_drift_detail(
            &head,
            &expected_base,
            Some(head_tree.as_str()),
            expected_tree.as_deref(),
        ));
    }
    if rec
        .payload
        .get("patch_truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(PREFLIGHT_PATCH_TRUNCATED.to_string());
    }
    // 复评：空白判定用 `trim().is_empty()`，但**存储/解析
    // 保留原始字节**——`.map(str::trim)` 会剥掉 patch 末尾换行，而末尾换行
    // 是标准文本 diff 解析格式的一部分（实测 git2::Diff:from_buffer 拒绝
    // 「无尾换行」的合法 patch）。
    let patch = rec
        .payload
        .get("patch")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| PREFLIGHT_NO_PATCH.to_string())?
        .to_string();
    let stage = rec
        .payload
        .get("publish_stage")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    // 意图前进判定（R1）：published_commit/published_branch 任一非空 =
    // git 半边意图曾落盘。与 stage 双锚——半行账（stage 丢/旧格式）不漏判。
    let intent_advanced = ["published_commit", "published_branch"].iter().any(|k| {
        rec.payload
            .get(k)
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty())
    });
    // Gerrit 重整锚（R3）：payload `recast_tree` 透传（Gerrit 臂随两次
    // 意图留痕写入）；非 Gerrit 臂恒缺席。
    let recast_tree = rec
        .payload
        .get("recast_tree")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);

    let issue_title = resolve_issue_title(store, task_id);
    let title = arg_str(args, "title")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .or(issue_title)
        .unwrap_or_else(|| format!("任务 {task_id} 交付"));
    Ok(Preflight::Proceed {
        copy_dir,
        patch,
        expected_base,
        title,
        already_merged,
        delivery_tree,
        base_tree: expected_tree,
        stage,
        intent_advanced,
        recast_tree,
    })
}

/// 前置失败词表（preflight → 编排层 fail_outcome 的控制信号；tests 逐条
/// 钉语义防文案漂移破坏对账判断）。
const PREFLIGHT_REJECTED: &str = "交付已被拒绝（rejected 终态）——拒绝的交付不发布";
const PREFLIGHT_EMPTY_REPO: &str = "副本仓库无 HEAD（空仓）——副本物化异常，请重新物化";
const PREFLIGHT_NO_BASE_SHA: &str = "交付 payload 缺 repo_base_sha（旧形态交付，无版本绑定）——无法核验副本基线，拒绝发布；请重跑任务产出带版本绑定的交付";
const PREFLIGHT_PATCH_TRUNCATED: &str =
    "交付 patch 已截断（超 1MB 上限）——截断 patch 发布会产出残缺 PR，拒绝发布";
const PREFLIGHT_NO_PATCH: &str =
    "交付 payload 无内联 patch 文本——无可发布物（patch_path 留底在 worker 侧，master 不可达）";

/// git 半边分支复用决策（重试幂等，纯函数，双层校验的上层快路）：
/// - 分支缺席 = `Ok(None)` → 常规建分支；
/// - 分支在场且指向树等值交付树（payload `tree` 锚）→ `Ok(Some(commit))`
///   直接复用上次产物（免 git 层探针）；
/// - 分支在场、交付树锚缺席 = `Ok(None)` → 落入
///   [`nemesis_board::git_repo:apply_patch_new_branch`] 的内容级复用探针
///   （父基线一致 + patch-over-HEAD tree 等值， 复评 移植）——
///   载荷无 `tree` 锚也能安全续作，等值校验由 git 层完成；
/// - 分支在场且树等值 Gerrit 重整锚（payload `recast_tree`——上次重整
///   commit 的树，R3 复评整改加第三参）→ 重整产物复用（重试不被
///   「分支树 ≠ 交付树」假失败卡死）；
/// - 分支在场且树不等值（有锚可比）→ Err 诚实拒绝不猜（分支被其他内容
///   占用；人为改分支仍拒绝，验收保全）。
///
/// ⚠ Gerrit 调用点对拒绝形态（含锚在场不等）追加重整候选树复核
///（`recast_candidate_tree` 现算等值即复用）——见调用臂注释。
fn branch_reuse_decision(
    existing: Option<&(String, String)>,
    delivery_tree: Option<&str>,
    recast_tree: Option<&str>,
) -> Result<Option<String>, String> {
    let Some((commit, branch_tree)) = existing else {
        return Ok(None);
    };
    if delivery_tree == Some(branch_tree.as_str()) || recast_tree == Some(branch_tree.as_str()) {
        return Ok(Some(commit.clone()));
    }
    if delivery_tree.is_none() && recast_tree.is_none() {
        return Ok(None);
    }
    Err(format!(
        "分支已存在且指向树 {branch_tree} ≠ 交付树{}——分支被其他内容占用，拒绝覆盖（Gerrit 重试场景若为历史重整产物且锚缺席，人工删除本地交付分支后重试可重铸）",
        recast_tree
            .map(|rt| format!(" ≠ 重整锚 {rt}"))
            .unwrap_or_default()
    ))
}

fn base_drift_detail(
    head: &str,
    expected_base: &str,
    head_tree: Option<&str>,
    expected_tree: Option<&str>,
) -> String {
    let ht = head_tree
        .filter(|t| !t.is_empty())
        .map(|t| format!("（tree {t}）"))
        .unwrap_or_default();
    let et = expected_tree
        .map(|t| format!("（tree {t}）"))
        .unwrap_or_default();
    format!(
        "副本基线漂移：副本 HEAD {head}{ht} ≠ 交付基线 {expected_base}{et}——把 patch 糊到漂移基线会产出错误交付，拒绝发布；请重物化副本或人工处理"
    )
}

/// 发布主流程。返回值恒为结构化 outcome（`status` = published /
/// already_published / failed + detail）——发布结果是第一公民结果不是传输
/// 错误；`Err` 只用于请求形态非法（缺必填参数 / protocol 缺席 / op 路由
/// 声明非法）。
/// `pub(crate)`：flow 引擎 publish step 与外部入口（web）共用同一实现。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn publish_task_delivery(
    db: &Arc<TaskDb>,
    store: &Arc<BoardStore>,
    agent_loop: &Arc<AgentLoop>,
    workspace: &Path,
    args: &Value,
    protocol: Option<&PublishProtocol>,
    plugin_manager: Option<&Arc<PublishPluginManager>>,
) -> Result<Value, String> {
    // 域剥离批 C-3：发布协议（创建/对账 op + args 形状）只来自 flow 模板
    // 声明；宿主无内置协议，缺席 = 诚实报错不猜。
    let protocol = protocol.ok_or_else(|| {
        "publish 协议声明缺失：publish 步须声明 create/reconcile op（域知识只在 flow 模板）——宿主无内置发布协议".to_string()
    })?;
    let task_id = arg_str(args, "task_id")?;
    let owner = arg_str(args, "owner")?;
    let repo = arg_str(args, "repo")?;
    // op 路由双臂解析（a）。声明面问题 = Err 上抛（请求形态非法），
    // 不进结构化失败响应——装载期校验（parse_publish_step_protocol）已
    // 挡住绝大多数，这里是运行期最后一道。
    let create_route = parse_op_route(&protocol.create_op)?;
    let reconcile_route = parse_op_route(&protocol.reconcile_op)?;
    // connector 可选化：仅任一 op 是 Native 形态才必填——全 plugin 形态的
    // 自持插件无 connector 概念（connector.toml 已退役）。
    let connector = match (&create_route, &reconcile_route) {
        (OpRoute::Native(_), _) | (_, OpRoute::Native(_)) => Some(arg_str(args, "connector")?),
        _ => None,
    };
    validate_task_id_for_branch(&task_id)?;
    // plugin 路由臂的审计归因键（a）：web 桥 protocol=None 在路由前
    // 已诚实报错，够不着本段。
    let session_key = format!("publish:{task_id}");
    let branch = format!("nemesis/task-{task_id}");
    let branch_key = format!("{task_id}:branch");
    // 批 B（SC 套件 Gerrit 臂 + 代理）：可选步参（模板声明面，值已装载期
    // config 渲染）——
    // - push_ref：推送 refspec（缺省 refs/heads/{branch}；`refs/for/` 前缀
    //   = Gerrit 评审形态：push 本身建 change，交付重整单提交 + Change-Id）；
    // - proxy：git2 网络代理（socks5h://host:port；空值/含未渲染 token =
    //   未配置语义——实例配置 proxy 键缺省空串，装载期渲染后即空）；
    // - Change-Id：task_id 确定性派生（SHA-1）——重试重推同 Change-Id =
    //   Gerrit 同一 change 的新 patchset，评审号稳定；仅 Gerrit 臂消费，
    //   rt token 面恒可用（非 Gerrit 声明里引用 = 渲染为派生值，无副作用）。
    let push_ref = args
        .get("push_ref")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.contains("{{"))
        .map(str::to_string)
        .unwrap_or_else(|| format!("refs/heads/{branch}"));
    let proxy = args
        .get("proxy")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.contains("{{"))
        .map(str::to_string);
    let change_id = nemesis_board::git_repo::derive_change_id(&task_id).ok();

    let pre = match preflight(db, store, &task_id, args) {
        Ok(Preflight::Already(v)) => return Ok(v),
        Ok(p) => p,
        Err(e) => return Ok(fail_outcome(&task_id, &branch, e)),
    };
    let Preflight::Proceed {
        copy_dir,
        patch,
        expected_base,
        title,
        already_merged,
        delivery_tree,
        base_tree,
        stage,
        intent_advanced,
        recast_tree,
    } = pre
    else {
        unreachable!("preflight 只产 Already/Proceed 两形态");
    };
    // 续作形态判定（R1 去单字段依赖）：stage==pr_pending 显式；stage 缺
    // 失或中间态但意图已前进（published_commit/branch 在场——warn 时代的
    // 半行账、旧格式账本）同样走对账先行的保守路径；published 终态不进
    // 续作（preflight 已按 external_ref 短路，此处防御）。
    let pr_pending_retry = stage.as_deref() == Some("pr_pending")
        || (stage.as_deref() != Some("published") && intent_advanced);

    // ③ git 半边。重试幂等双层（真链第二缺口 + 复评）：上次
    // 发布可能 git 半边已完成（分支已推、条目创建卡审批/网络）——失败文案
    // 承诺「可重试」，重试不能被自己上次的分支挡死。上层快路：分支在场且
    // 指向树 == 交付树（payload 锚）→ 直接复用该 commit；锚缺席 → 落入
    // apply_patch_new_branch 的内容级探针（父基线一致 + patch-over-HEAD
    // tree 等值）幂等复用；树不等 = 分支被其他内容占用，诚实拒绝不猜。
    let existing = match nemesis_board::git_repo::branch_tree_if_exists(&copy_dir, &branch) {
        Ok(v) => v,
        Err(e) => {
            return Ok(fail_outcome(
                &task_id,
                &branch,
                format!("git 半边失败（确定失败，可重试）: {e}"),
            ));
        }
    };
    let commit_hex = match branch_reuse_decision(
        existing.as_ref(),
        delivery_tree.as_deref(),
        recast_tree.as_deref(),
    ) {
        Ok(Some(c)) => c,
        // R3 锚缺席窗口兜底：Gerrit 臂 + 分支在场 + 快路锚未命中。两个到达
        // 形态：①交付/重整锚均缺席（Ok(None)——旧账本）；②交付锚在场但
        // 分支树已非交付树（Err——R1 意图留痕中止后的重试现场：上一轮
        // Gerrit 臂已重整 force 过分支、abort 发生在留痕点，payload 永远
        // 没拿到 recast_tree，建单时写入的交付树锚自然对不上）。重整候选
        // 树现算（recast_candidate_tree 与 single_commit_atop 同源合并语
        // 义）：分支树 == 候选 ⇒ 上次重整产物仍在，复用；否则保留快路拒
        // 绝语义（人为改动/目标已前进不可区分，patch 套重整内容必炸——指
        // 引人工删分支后重试重铸）。
        other if existing.is_some() && push_ref.starts_with("refs/for/") => {
            let (prev_commit, branch_tree) = existing.expect("臂条件已保证在场");
            let fast_reject = match &other {
                Err(m) => m.clone(),
                Ok(_) => "交付/重整锚均缺席".to_string(),
            };
            let target = push_ref
                .strip_prefix("refs/for/")
                .expect("臂条件已保证前缀");
            let Some(delivery_hex) = delivery_tree.as_deref().filter(|s| !s.trim().is_empty())
            else {
                return Ok(fail_outcome(
                    &task_id,
                    &branch,
                    format!(
                        "Gerrit 臂分支在场（树 {branch_tree}）但交付树缺席（{fast_reject}，无法现算重整候选）——patch 套重整内容有污染风险，拒绝。人工删除本地交付分支 {branch} 后重试可重铸"
                    ),
                ));
            };
            let base_tree_hex = match base_tree.as_deref().filter(|s| !s.trim().is_empty()) {
                Some(t) => t.to_string(),
                None => {
                    match nemesis_board::git_repo::tree_of_commit_hex(&copy_dir, &expected_base) {
                        Ok(t) => t,
                        Err(e) => {
                            return Ok(fail_outcome(
                                &task_id,
                                &branch,
                                format!(
                                    "Gerrit 臂基树解析失败（基线提交 {expected_base} 不在副本仓——副本被重物化？可重试，再失败需人工核查）: {e}"
                                ),
                            ));
                        }
                    }
                }
            };
            match nemesis_board::git_repo::recast_candidate_tree(
                &copy_dir,
                target,
                &base_tree_hex,
                delivery_hex,
            ) {
                Ok(candidate) if candidate == branch_tree => prev_commit,
                Ok(candidate) => {
                    return Ok(fail_outcome(
                        &task_id,
                        &branch,
                        format!(
                            "Gerrit 重整候选树 {candidate} ≠ 分支树 {branch_tree}（{fast_reject}；分支不是上次重整产物——人为改动或目标分支已前进）。人工删除本地交付分支 {branch} 后重试可重铸，或人工核查分支内容"
                        ),
                    ));
                }
                Err(e) => {
                    return Ok(fail_outcome(
                        &task_id,
                        &branch,
                        format!("Gerrit 重整候选树计算失败（可重试）: {e}"),
                    ));
                }
            }
        }
        Ok(None) => {
            // 分支缺席（或锚缺席交 git 层探针裁决）→ 常规建分支。第三臂
            // （already_merged——交付已并入副本 main）时分支直接指向 HEAD：
            // 内容树已等值交付树，patch 再套必失败，也无需新 commit；否则
            // 套 patch 产新 commit（分支在场时 apply 内探针裁决复用/拒绝）。
            if already_merged {
                match nemesis_board::git_repo::branch_at_head(&copy_dir, &branch) {
                    Ok(c) => c,
                    Err(e) => {
                        return Ok(fail_outcome(
                            &task_id,
                            &branch,
                            format!("git 半边失败（确定失败，可重试）: {e}"),
                        ));
                    }
                }
            } else {
                match nemesis_board::git_repo::apply_patch_new_branch(
                    &copy_dir,
                    &branch,
                    patch.as_bytes(),
                    &format!("NemesisBot task {task_id}: {title}"),
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        return Ok(fail_outcome(
                            &task_id,
                            &branch,
                            format!("git 半边失败（确定失败，可重试）: {e}"),
                        ));
                    }
                }
            }
        }
        Err(msg) => {
            return Ok(fail_outcome(
                &task_id,
                &branch,
                format!("git 半边失败（确定失败，需人工核查）: {msg}"),
            ));
        }
    };
    // ③.5 Gerrit 评审形态（批 B，SC 套件）：push_ref 以 `refs/for/` 开头 =
    // Gerrit change 评审——push 本身建 change（无 MR 对象，REST 建条目走
    // query-as-create：create_op = change_list 按 Change-Id 查询，已建 =
    // 收口落账、未建 = 诚实失败留 pr_pending 由下轮对账收敛）。交付内容
    // 重整为**单提交 atop 目标分支头**（refs/remotes/origin/<target>，目标
    // 提交缺席 = 副本未 sync，诚实 Err）：
    // - 交付树 = 当前 commit 的树（reuse/apply/already_merged 三臂通吃——
    //   树是内容等价面）；基树 = payload `base_tree` 锚，缺席取基线提交的
    //   树（基线漂移已在 preflight 挡过，此处只做树解析）；
    // - 三方合并冲突 = 诚实 Err 人工 rebase；合并树 == 目标头树 = 诚实 Err
    //   无可发布差异（交付已在目标分支）；目标分支被本地检出 = Err。
    // 产出 commit 替换 commit_hex（refs/heads/{branch} 单亲重置——分支是
    // 本地交付面，推送走 refs/for/<target>，远端主线零污染）。
    let commit_hex = match push_ref.strip_prefix("refs/for/") {
        Some(target) => {
            let Some(change_id) = change_id.as_ref() else {
                return Ok(fail_outcome(
                    &task_id,
                    &branch,
                    "Change-Id 派生失败（git 哈希原语异常）——Gerrit 臂拒绝发布".to_string(),
                ));
            };
            let delivery_tree_hex =
                match nemesis_board::git_repo::tree_of_commit_hex(&copy_dir, &commit_hex) {
                    Ok(t) => t,
                    Err(e) => {
                        return Ok(fail_outcome(
                            &task_id,
                            &branch,
                            format!("Gerrit 臂交付树解析失败（确定失败，可重试）: {e}"),
                        ));
                    }
                };
            let base_tree_hex = match base_tree.as_deref().filter(|s| !s.trim().is_empty()) {
                Some(t) => t.to_string(),
                None => {
                    match nemesis_board::git_repo::tree_of_commit_hex(&copy_dir, &expected_base) {
                        Ok(t) => t,
                        Err(e) => {
                            return Ok(fail_outcome(
                                &task_id,
                                &branch,
                                format!(
                                    "Gerrit 臂基树解析失败（基线提交 {expected_base} 不在副本仓——副本被重物化？可重试，再失败需人工核查）: {e}"
                                ),
                            ));
                        }
                    }
                }
            };
            // 提交信息：commit_message 模板 rt 第二遍渲染（change_id/branch/
            // task_id 运行期 token；条目 token 已在 parse 期消化）；模板未
            // 带 Change-Id footer 时补齐——Gerrit 缺 footer 直接拒收
            //（missing Change-Id）。
            let mut message = match &protocol.commit_message {
                Some(tpl) => crate::flow_proto::render_tokens(tpl, |key: &str| match key {
                    "change_id" => Some(change_id.clone()),
                    "branch" => Some(branch.clone()),
                    "task_id" => Some(task_id.clone()),
                    _ => None,
                }),
                None => format!("NemesisBot task {task_id}: {title}"),
            };
            if !message.contains("Change-Id:") {
                while message.ends_with('\n') {
                    message.pop();
                }
                message.push_str(&format!("\n\nChange-Id: {change_id}"));
            }
            match nemesis_board::git_repo::single_commit_atop(
                &copy_dir,
                &branch,
                target,
                &base_tree_hex,
                &delivery_tree_hex,
                &message,
            ) {
                Ok(c) => c,
                Err(e) => {
                    return Ok(fail_outcome(
                        &task_id,
                        &branch,
                        format!(
                            "Gerrit 单提交重整失败（确定失败，可重试——同 Change-Id 幂等）: {e}"
                        ),
                    ));
                }
            }
        }
        None => commit_hex,
    };
    // R3 复评整改：Gerrit 臂重整锚（recast_tree = 重整 commit 的树）随
    // 两次意图留痕写入 payload——重试凭「分支树 == 重整锚」幂等复用重整
    // 产物；非 Gerrit 臂 = None（payload 不带锚字段，行为不变）。
    let recast_tree = if push_ref.starts_with("refs/for/") {
        match nemesis_board::git_repo::tree_of_commit_hex(&copy_dir, &commit_hex) {
            Ok(t) => Some(t),
            Err(e) => {
                return Ok(fail_outcome(
                    &task_id,
                    &branch,
                    format!("Gerrit 重整锚树解析失败（可重试）: {e}"),
                ));
            }
        }
    } else {
        None
    };
    // 阶段留痕（续作；best-effort——留痕写失败不阻断发布本身，只降级
    // 重试续作的精确度，warn 可观测）。**阶段单调不回退**：pr_pending 现场
    // 续作时跳过中间态——把 pr_pending 覆盖成 commit_generated/push_completed
    // 后，若本次重试又在对账/创建窗口中断，下次重试丢失 pr_pending 语义 →
    // 不对账直建条目 = 重复建条目风险（stage 只前进不后退）。
    if !pr_pending_retry {
        let mut stage_payload =
            json!({ "publish_stage": "commit_generated", "published_commit": commit_hex });
        if let Some(t) = recast_tree.as_deref() {
            stage_payload["recast_tree"] = Value::String(t.to_string());
            stage_payload["recast_commit"] = Value::String(commit_hex.clone());
        }
        if let Err(e) = db.merge_delivery_payload(&branch_key, &stage_payload) {
            // R1 复评整改：意图未落盘就不做外部写——warn+继续把「重试续
            // 作精确度」降级成「重复建条目风险」。此处中止（commit 已在
            // 本地分支，重试经 branch_reuse_decision 树等值/重整锚幂等复
            // 用，安全）。
            return Ok(fail_outcome(
                &task_id,
                &branch,
                format!("发布意图落盘失败（可重试）: {e}"),
            ));
        }
    }
    // ④ 推送（vault PAT；匿名路径 = 公开仓语义，与 clone 凭据对称）。凭据
    // 别名从 create_op 路由推导（a 单点）：Plugin → plugin/<slug>/pat
    // （自持插件 PAT），Native → connector/<id>/pat。批 B：cred_secret 显式
    // 覆盖优先（Gerrit = HTTP Basic 用户名 + HTTP Password，别名与 GitLab
    // PAT 分开，如 plugin/sc-bugfix/gerrit_http_pass；cred_user 缺省回落
    // owner——GitLab HTTPS+PAT 任意非空用户名语义不变）。
    let cred = match args
        .get("cred_secret")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(alias) => {
            let user = args
                .get("cred_user")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(owner.as_str());
            publish_credentials(workspace, alias, user)
        }
        None => match credential_alias(connector.as_deref(), &create_route) {
            Ok(alias) => publish_credentials(workspace, &alias, &owner),
            Err(e) => {
                return Ok(fail_outcome(&task_id, &branch, e));
            }
        },
    };
    if let Err(e) = nemesis_board::git_repo::push_branch(
        &copy_dir,
        &branch,
        "origin",
        &push_ref,
        cred.as_ref().map(|(u, p)| (u.as_str(), p.as_str())),
        proxy.as_deref(),
    ) {
        return Ok(fail_outcome(
            &task_id,
            &branch,
            format!("分支推送失败（确定失败，可重试——重推同内容幂等）: {e}"),
        ));
    }
    if !pr_pending_retry {
        let mut stage_payload =
            json!({ "publish_stage": "push_completed", "published_commit": commit_hex });
        if let Some(t) = recast_tree.as_deref() {
            stage_payload["recast_tree"] = Value::String(t.to_string());
            stage_payload["recast_commit"] = Value::String(commit_hex.clone());
        }
        if let Err(e) = db.merge_delivery_payload(&branch_key, &stage_payload) {
            // R1 复评整改：push 已完成但意图未落盘 = 本地分支领先账本——
            // 中止（重试树等值/重整锚复用 + 重推幂等，不做无账本锚的外部
            // 推进）。
            return Ok(fail_outcome(
                &task_id,
                &branch,
                format!("发布意图落盘失败（可重试）: {e}"),
            ));
        }
    }

    // ⑤ 发布条目创建 + unknown 对账。运行期 token 第二遍渲染（git 步之后
    // commit/base_sha 才存在；branch/task_id 幂等重渲；未知 token 保持字面
    // 量——渲染原语契约，畸形声明由解析期校验兜底）。
    let rt = |tpl: &str| {
        crate::flow_proto::render_tokens(tpl, |key: &str| match key {
            "commit" => Some(commit_hex.clone()),
            "base_sha" => Some(expected_base.clone()),
            "branch" => Some(branch.clone()),
            "task_id" => Some(task_id.clone()),
            "change_id" => change_id.clone(),
            _ => None,
        })
    };
    let mut create_args = serde_json::Map::new();
    for (k, v) in &protocol.create_args {
        create_args.insert(k.clone(), Value::String(rt(v)));
    }
    let rendered_reconcile_args: BTreeMap<String, String> = protocol
        .reconcile_args
        .iter()
        .map(|(k, v)| (k.clone(), rt(v)))
        .collect();
    let rendered_match = rt(&protocol.reconcile_match);

    // pr_pending 续作：上次若已到达创建阶段（publish_stage =
    // pr_pending：进程崩溃/落账失败留下），**先对账再决定**——已建成条目
    // 直接收敛落账（不重复建）；扫尽确认不在才安全重建；扫不尽维持保守
    // 失败（重复建条目风险不静默承担）。对账走同一条声明化翻页匹配通路
    // （reconcile_by_match），无硬编码域协议。
    if pr_pending_retry {
        match reconcile_by_match(
            agent_loop,
            connector.as_deref(),
            plugin_manager,
            &reconcile_route,
            &session_key,
            protocol,
            &rendered_reconcile_args,
            &rendered_match,
        )
        .await
        {
            ReconcileOutcome::Found(n, u) => {
                return finish_published(
                    db,
                    &branch_key,
                    &task_id,
                    &branch,
                    n,
                    u,
                    &commit_hex,
                    &owner,
                    &repo,
                    "对账发现上次已建成的条目——重试续作收敛，未重复创建",
                );
            }
            // 扫尽确认不在：重建安全，落入常规创建。
            ReconcileOutcome::Absent => {}
            ReconcileOutcome::Inconclusive => {
                return Ok(fail_outcome(
                    &task_id,
                    &branch,
                    format!(
                        "上次发布到达创建阶段后中断，对账未能扫尽（页数封顶/响应不可解析/查询失败），无法排除已建成条目——保守判失败可重试，重试仍会先对账，但重复创建风险自担（可人工到仓库按分支 {branch} 核实）"
                    ),
                ));
            }
        }
    }
    // 阶段留痕：pr_pending 在创建请求发出**之前**落账——创建请求发出后
    // 结果丢失（进程崩溃/响应不可解析）时，重试凭此先对账防重复建条目。
    // R1 复评整改：落账失败不再 warn+继续（无 pr_pending 锚的创建请求 =
    // 中断后盲目重建），改中止——创建请求未发出，重试安全。
    let mut pending_payload = json!({
        "publish_stage": "pr_pending",
        "published_commit": commit_hex,
        "published_branch": branch,
        "publish_op": protocol.create_op,
    });
    if let Some(t) = recast_tree.as_deref() {
        pending_payload["recast_tree"] = Value::String(t.to_string());
        pending_payload["recast_commit"] = Value::String(commit_hex.clone());
    }
    if let Err(e) = db.merge_delivery_payload(&branch_key, &pending_payload) {
        return Ok(fail_outcome(
            &task_id,
            &branch,
            format!("发布意图落盘失败（可重试）: {e}"),
        ));
    }
    let (external_ref, external_url) = match invoke_route(
        agent_loop,
        connector.as_deref(),
        plugin_manager,
        &create_route,
        &session_key,
        Value::Object(create_args),
    )
    .await
    {
        Ok(resp) => {
            match parse_publish_result(&resp, &protocol.result_ref, protocol.result_url.as_deref())
            {
                Some(found) => found,
                // 非预期 JSON（HTTP 错误页/截断/未知形态）→ 对账兜底。
                None => {
                    match reconcile_by_match(
                        agent_loop,
                        connector.as_deref(),
                        plugin_manager,
                        &reconcile_route,
                        &session_key,
                        protocol,
                        &rendered_reconcile_args,
                        &rendered_match,
                    )
                    .await
                    {
                        ReconcileOutcome::Found(n, u) => (n, u),
                        // 扫尽确认不在：重试建条目安全（不会重复）。
                        ReconcileOutcome::Absent => {
                            return Ok(fail_outcome(
                                &task_id,
                                &branch,
                                format!(
                                    "发布创建未确认（结果非预期 JSON），对账扫尽无匹配条目——保守判失败可重试。原始响应: {}",
                                    head_chars(&resp, 400)
                                ),
                            ));
                        }
                        // 未能扫尽（页数封顶/响应不可解析/查询失败）：可能已建成
                        // ——留 pr_pending 阶段，下次重试先对账收敛，不盲目重建。
                        ReconcileOutcome::Inconclusive => {
                            return Ok(fail_outcome(
                                &task_id,
                                &branch,
                                format!(
                                    "发布创建未确认且对账未能扫尽（页数封顶/响应不可解析/查询失败），无法排除已建成条目——保守判失败，**重试会先对账收敛**（不重复创建；也可人工核实）。原始响应: {}",
                                    head_chars(&resp, 400)
                                ),
                            ));
                        }
                    }
                }
            }
        }
        // 确定未发出（a 两族）：dispatch 层拒（estop/审批/安全层，`⛔`
        // 前缀）或 plugin 路由层失败（插件未装配/停用/词表对账不过，
        // `[not-sent]` 标记）——两者都不可能产生远端副作用，重试安全。
        Err(text)
            if crate::connector_tools::error_text_dispatch_rejected(&text)
                || text.starts_with(NOT_SENT_MARKER) =>
        {
            return Ok(fail_outcome(
                &task_id,
                &branch,
                format!("发布动作被拒绝（未发出，可重试）: {text}"),
            ));
        }
        // 其余错误 = 结果待定（超时/传输断→请求可能已送达；HTTP 4xx/5xx→
        // 服务端已应答，409 duplicate 即幂等收敛的服务端形态；连接失败→
        // 对端不可证）——一律按幂等键对账收敛，不做「未发出」的确定性断言。
        Err(text) => {
            match reconcile_by_match(
                agent_loop,
                connector.as_deref(),
                plugin_manager,
                &reconcile_route,
                &session_key,
                protocol,
                &rendered_reconcile_args,
                &rendered_match,
            )
            .await
            {
                ReconcileOutcome::Found(n, u) => (n, u),
                // 扫尽确认不在：请求未生效，重试建条目安全。
                ReconcileOutcome::Absent => {
                    return Ok(fail_outcome(
                        &task_id,
                        &branch,
                        format!(
                            "发布创建失败且对账扫尽无匹配条目（未生效）——保守判失败可重试。原始错误: {}",
                            head_chars(&text, 400)
                        ),
                    ));
                }
                // 未能扫尽（页数封顶/响应不可解析/查询失败）：无法排除已建成
                // ——留 pr_pending 阶段，下次重试先对账收敛，不盲目重建。
                ReconcileOutcome::Inconclusive => {
                    return Ok(fail_outcome(
                        &task_id,
                        &branch,
                        format!(
                            "发布创建结果未知且对账未能扫尽（页数封顶/响应不可解析/查询失败），无法排除已建成条目——保守判失败，**重试会先对账收敛**（不重复创建；也可人工核实）。原始错误: {}",
                            head_chars(&text, 400)
                        ),
                    ));
                }
            }
        }
    };

    // ⑥ 落账收敛（published 终态 + 引用留痕，单事务）。
    finish_published(
        db,
        &branch_key,
        &task_id,
        &branch,
        external_ref,
        external_url,
        &commit_hex,
        &owner,
        &repo,
        "已推送交付分支并创建发布条目",
    )
}

/// ⑥ 落账收敛：published 终态 + 外部引用留痕（**单事务**——状态与留痕要么
/// 同落要么都不落，杜绝「状态已 published 而 payload 无引用」的半行账让
/// 重试误判， 复评）。正常创建与 pr_pending 对账续作共用此
/// 出口。`external_url` 允许 None（对账只拿到引用号没拿到链接），落账为
/// null、响应同形。
#[allow(clippy::too_many_arguments)]
fn finish_published(
    db: &Arc<TaskDb>,
    branch_key: &str,
    task_id: &str,
    branch: &str,
    external_ref: i64,
    external_url: Option<String>,
    commit_hex: &str,
    owner: &str,
    repo: &str,
    detail: &str,
) -> Result<Value, String> {
    db.mark_delivery_published_with_payload(
        branch_key,
        &json!({
            "publish_stage": "published",
            "external_ref": external_ref,
            "external_url": external_url,
            "published_at": chrono::Local::now().to_rfc3339(),
            "published_branch": branch,
            "published_remote": format!("{owner}/{repo}"),
            "published_commit": commit_hex,
        }),
    )?;
    tracing::info!(
        task_id = %task_id,
        external_ref,
        branch = %branch,
        "[Publish] 交付已发布: {detail}"
    );
    Ok(json!({
        "task_id": task_id,
        "status": ST_PUBLISHED,
        "external_ref": external_ref,
        "external_url": external_url,
        "commit": commit_hex,
        "branch": branch,
        "remote": format!("{owner}/{repo}"),
        "detail": detail,
    }))
}

// ---------------------------------------------------------------------------
// 纯函数 + 薄封装（对账/解析/校验/凭据）
// ---------------------------------------------------------------------------

fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("缺少或空参数: {key}"))
}

/// task_id → git 分支名成分合法性（`nemesis/task-<task_id>` 的 task 段）：
/// ASCII 字母数字 + `-_.`，拒 `/`、空白、`..`、首 `.`——git ref 词表 + 路径
/// 注入双防。集群 task_id 是 uuid 形态，天然合规；防御的是手工传参。
fn validate_task_id_for_branch(task_id: &str) -> Result<(), String> {
    if task_id.is_empty() || task_id.len() > 64 {
        return Err(format!("task_id 长度非法（{}）：1..=64", task_id.len()));
    }
    if !task_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("task_id 含非法字符（只允许字母数字 - _ .）".to_string());
    }
    if task_id.contains("..") || task_id.starts_with('.') {
        return Err("task_id 不得以 . 开头或包含 ..".to_string());
    }
    Ok(())
}

/// 副本目录解析：显式 `copy_path` 优先；缺省走 task_id → dispatch → issue
/// → project → directory 链（任务副本）。终点必须含 `.git`
/// （clone=false 的项目目录没有远端可推，诚实报错）。
fn resolve_copy_dir(
    store: &BoardStore,
    task_id: &str,
    copy_path: Option<&Value>,
) -> Result<PathBuf, String> {
    if let Some(p) = copy_path
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let dir = PathBuf::from(p);
        if !dir.join(".git").exists() {
            return Err(format!("copy_path `{p}` 不是 git 仓库（缺 .git）"));
        }
        return Ok(dir);
    }
    let dispatch = store
        .get_dispatch(task_id)?
        .ok_or_else(|| format!("任务 {task_id} 无派发记录——缺省副本解析需要 dispatch→issue→project 链，请显式传 copy_path"))?;
    let issue = store.get_issue(dispatch.issue_id)?;
    let pid = issue
        .project_id
        .ok_or_else(|| "来源 issue 无所属项目（project_id 空）——请显式传 copy_path".to_string())?;
    let project = store.get_project(pid)?;
    let dir = project
        .directory
        .ok_or_else(|| "项目无目录（directory 空）——请显式传 copy_path".to_string())?;
    let dir = PathBuf::from(dir);
    if !dir.join(".git").exists() {
        return Err(format!(
            "项目目录 `{}` 不是 git 仓库（clone=false 副本无远端可推）——请改用 clone=true 副本或显式传 copy_path",
            dir.display()
        ));
    }
    Ok(dir)
}

/// issue 标题（发布默认标题源；链路任一环缺席 = None，落回调用方 fallback）。
fn resolve_issue_title(store: &BoardStore, task_id: &str) -> Option<String> {
    let dispatch = store.get_dispatch(task_id).ok()??;
    let issue = store.get_issue(dispatch.issue_id).ok()?;
    Some(issue.title)
}

/// vault 凭据（别名直收——`connector/<id>/pat` 或 `plugin/<slug>/pat`，批
/// 3a 别名推导单点在 [`credential_alias`]）→ (owner, pat)：与 clone 凭据
/// 同源同别名。缺席 = None（匿名推送——公开仓合法路径；私有仓 push 会诚实
/// 失败）。
fn publish_credentials(workspace: &Path, alias: &str, owner: &str) -> Option<(String, String)> {
    let vault_path = nemesis_path::resolve_vault_path_in_workspace(workspace);
    match crate::vault_runtime::resolve_alias_at(&vault_path, alias) {
        Ok(pat) if !pat.trim().is_empty() => Some((owner.to_string(), pat)),
        _ => None,
    }
}

/// connector op 调用（connector_invoke 工具走主 loop 完整 dispatch 瀑布，
/// 非旁路——estop/hidden/安全 8 层/凭据注入/回执全生效）。
async fn invoke_native(
    agent_loop: &AgentLoop,
    connector: &str,
    op: &str,
    args: Value,
) -> Result<String, String> {
    let call = nemesis_agent::types::ToolCallInfo {
        id: format!("publish-{}", uuid::Uuid::new_v4()),
        name: crate::connector_tools::INVOKE_TOOL_NAME.to_string(),
        arguments: json!({"connector": connector, "op": op, "args": args}).to_string(),
    };
    let ctx = RequestContext::new(
        "delivery-publish",
        "delivery-publish",
        "delivery-publish",
        "delivery-publish",
    );
    let text = agent_loop.handle_tool_call(&call, &ctx).await;
    if crate::connector_tools::is_tool_error(&text) {
        return Err(text);
    }
    Ok(text)
}

/// plugin 步调用（a）：`run_flow_step(slug, op, {"op","args",
/// "session_key"})` 四态映射（与 report_plugin_comment/plugin fetch 同源
/// 解析单点 [`crate::flow_proto::plugin_step_outcome`]）。
///
/// 语义分族（错误三态决定对账分支——诚实边界）：
/// - 路由层失败（子系统缺席/插件停用/词表对账不过）= **确定未发出**，
///   错误带 [`NOT_SENT_MARKER`] 前缀 → publish_task_delivery 归「未发出
///   确定失败」分支（重试安全）；
/// - guest 四态 `done` = 载荷原样返回（`{"status":"done",...}` 去 status
///   后对象，JSON path 直读字段展开）；
/// - `failed`/`running`/`skipped`/malformed = guest 已执行过或语义违规，
///   错误**不带**标记 → 走「可能已送达」对账保守分支（failed = HTTP 层
///   失败可能已发出；running = guest 异步未决；skipped = 发布步无跳过
///   语义但声明者违约时不可断言零副作用）。
#[cfg(feature = "plugins-wasm")]
async fn invoke_plugin(
    plugin_manager: &PublishPluginManager,
    slug: &str,
    op: &str,
    session_key: &str,
    args: Value,
) -> Result<String, String> {
    let input = json!({
        "op": op,
        "args": args,
        "session_key": session_key,
    })
    .to_string();
    let out_json = plugin_manager
        .run_flow_step(slug, op, input)
        .await
        .map_err(|e| format!("{NOT_SENT_MARKER} plugin:{slug}:{op} 投递失败: {e}"))?;
    match crate::flow_proto::plugin_step_outcome(&out_json) {
        crate::flow_proto::PluginOutcome::Done(payload) => Ok(payload.to_string()),
        crate::flow_proto::PluginOutcome::Skipped(_) => Err(format!(
            "{NOT_SENT_MARKER} plugin:{slug}:{op} guest 自报 skipped——发布步无跳过语义（创建/对账必须实作）"
        )),
        crate::flow_proto::PluginOutcome::Running => Err(format!(
            "plugin:{slug}:{op} guest 处理中（running）——发布是单 tick 同步语义，op 应同步完成后返回"
        )),
        crate::flow_proto::PluginOutcome::Failed(err) => {
            Err(format!("plugin:{slug}:{op} 失败: {err}"))
        }
        crate::flow_proto::PluginOutcome::Malformed(err) => {
            Err(format!("plugin:{slug}:{op} 出参畸形: {err}"))
        }
    }
}

/// 非 wasm 裁剪形态（R8 复评整改）：plugin 路由臂不可达——
/// [`PublishPluginManager`] 空枚举不可构造，`Option` 恒 `None` 在
/// [`invoke_route`] 已拦；本体是防御性兜底（恒「确定未发出」，重试安全）。
#[cfg(not(feature = "plugins-wasm"))]
async fn invoke_plugin(
    _plugin_manager: &PublishPluginManager,
    slug: &str,
    op: &str,
    _session_key: &str,
    _args: Value,
) -> Result<String, String> {
    Err(format!(
        "{NOT_SENT_MARKER} plugin:{slug}:{op} 投递失败: 插件子系统未编译（plugins-wasm feature 关）"
    ))
}

/// op 路由统一入口（a）：Native 臂 = connector_invoke 工具面原样；
/// Plugin 臂 = run_flow_step 四态。臂前置条件缺失（Native 无 connector /
/// Plugin 无子系统）= 诚实 Err。
async fn invoke_route(
    agent_loop: &Arc<AgentLoop>,
    connector: Option<&str>,
    plugin_manager: Option<&Arc<PublishPluginManager>>,
    route: &OpRoute,
    session_key: &str,
    args: Value,
) -> Result<String, String> {
    match route {
        OpRoute::Native(op) => {
            let connector = connector.ok_or_else(|| {
                format!(
                    "op '{op}' 是 connector 原语形态但 publish 步缺 connector 声明（自持插件请改用 plugin:<slug>:<op>）"
                )
            })?;
            invoke_native(agent_loop, connector, op, args).await
        }
        OpRoute::Plugin { slug, op } => {
            let manager = plugin_manager.ok_or_else(|| {
                format!(
                    "{NOT_SENT_MARKER} plugin:{slug}:{op} 投递失败: flow-step 插件子系统未装配（宿主壳缺席/总闸关）"
                )
            })?;
            invoke_plugin(manager, slug, op, session_key, args).await
        }
    }
}

/// 创建结果解析（协议声明化）：result_ref JSON path 取「外部条目号」、
/// result_url path 取人读链接（scheme 闸 [`crate::flow_proto::safe_external_url`]）。
/// 直接对象或单元素数组都认；认不出的形态交给对账（不猜）。
fn parse_publish_result(
    text: &str,
    result_ref: &str,
    result_url: Option<&str>,
) -> Option<(i64, Option<String>)> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    // 数组形态取首元素；clone 出 owned 值供 json_path_first（借用冲突规避）。
    let obj_val = match &v {
        Value::Object(_) => v.clone(),
        Value::Array(a) => a.first().cloned()?,
        _ => return None,
    };
    let n = crate::flow_proto::json_path_first(&obj_val, result_ref)?.as_i64()?;
    let url = result_url
        .and_then(|path| crate::flow_proto::json_path_first(&obj_val, path))
        .and_then(|u| u.as_str().map(str::to_string))
        .and_then(|u| crate::flow_proto::safe_external_url(&u));
    Some((n, url))
}

/// unknown 对账（协议声明化）：reconcile_op 按声明翻页回查，
/// `reconcile_match` 比较式逐条匹配同仓条目（state=all——open/closed 一并
/// 认，报告既有条目比重复建一个诚实）。
///
/// 翻页（对账固定首页根修的协议化形态）：条目总数超过单页
/// limit 时目标可能落在后续页——逐页扫描 + 硬上限：满页才翻下一页，不足
/// 一页 = 末页即停；页数硬上限防对端异常（恒满页/无限分页）把对账变成
/// 无界循环。limit/page 参数键名与页大小由 protocol 声明（宿主逐页注入/
/// 覆盖模板基参）。
///
/// 三态结论（回顾收紧）：Absent = 扫到不足一页的短页仍无命中
/// （**扫尽确认不在**，重试建条目安全）；Inconclusive = 页数硬上限封顶 /
/// 某页响应不可解析 / 查询本身失败 / **命中但引用不可读**（**未能扫尽或判
/// 不清**——保守判失败可重试，但重试存在重复建条目风险，操作者文案必须
/// 如实区分）。
enum ReconcileOutcome {
    Found(i64, Option<String>),
    Absent,
    Inconclusive,
}

/// 对账翻页推进判定（纯函数）：本页不足一页 = 末页取尽（Absent）；
/// 满页未中 = 翻下一页（Next），超过硬上限封顶（Inconclusive）。
fn reconcile_advance(page: usize, page_len: usize, protocol: &PublishProtocol) -> ReconcilePaging {
    if page_len < protocol.reconcile_limit {
        return ReconcilePaging::Absent;
    }
    let next = page + 1;
    if next <= protocol.reconcile_max_pages {
        ReconcilePaging::Next(next)
    } else {
        ReconcilePaging::Inconclusive
    }
}

#[derive(Debug, PartialEq)]
enum ReconcilePaging {
    Next(usize),
    Absent,
    Inconclusive,
}

/// 单条对账匹配结果三态：NoMatch = 比较式判定不命中；Hit = 命中；Unreadable
/// = 比较式畸形/条目形态读不出（保守 Inconclusive，不冒充「不在」）。
enum ItemMatch {
    NoMatch,
    Hit,
    Unreadable,
}

/// 逐条匹配（协议声明化）：比较式一侧 `$.` 前缀 = 条目 JSON path 取标量
/// （缺席 → "null"），另一侧 = 运行期已渲染的字面量；比较原语与 `when` 条
/// 件步同源（flow_engine::split_compare_expr/compare_str）。
fn match_item(item: &Value, expr: &str) -> ItemMatch {
    let Some((left, op, right)) = crate::flow_proto::split_compare_expr(expr) else {
        return ItemMatch::Unreadable;
    };
    let scalar = |side: &str| -> String {
        if side.starts_with("$.") {
            crate::flow_proto::json_path_first(item, side)
                .map(|v| crate::flow_proto::json_scalar(Some(v)))
                .unwrap_or_else(|| "null".to_string())
        } else {
            side.to_string()
        }
    };
    if crate::flow_proto::compare_str(op, &scalar(left), &scalar(right)) {
        ItemMatch::Hit
    } else {
        ItemMatch::NoMatch
    }
}

async fn reconcile_by_match(
    agent_loop: &Arc<AgentLoop>,
    connector: Option<&str>,
    plugin_manager: Option<&Arc<PublishPluginManager>>,
    route: &OpRoute,
    session_key: &str,
    protocol: &PublishProtocol,
    reconcile_args: &BTreeMap<String, String>,
    match_expr: &str,
) -> ReconcileOutcome {
    let mut page = 1usize;
    loop {
        // 基参（运行期已渲染）+ 宿主逐页注入 limit/page（模板基参同键被
        // 覆盖——分页参数宿主管，域参模板管）。
        let mut page_args = serde_json::Map::new();
        for (k, v) in reconcile_args {
            page_args.insert(k.clone(), Value::String(v.clone()));
        }
        page_args.insert(
            protocol.reconcile_limit_key.clone(),
            json!(protocol.reconcile_limit),
        );
        page_args.insert(protocol.reconcile_page_key.clone(), json!(page));
        let text = match invoke_route(
            agent_loop,
            connector,
            plugin_manager,
            route,
            session_key,
            Value::Object(page_args),
        )
        .await
        {
            Ok(t) => t,
            // 查询失败 = 对账没跑成，不是「不存在」。
            Err(_) => return ReconcileOutcome::Inconclusive,
        };
        // 页体数组解析 FIRST：不可解析（截断/错误页）= 未能扫尽，不冒充
        // 「扫尽不在」。三形态（a）：顶层数组（connector 原语惯例）、
        // `{"entries": [...]}` 包裹（plugin 步 done 契约同款）、其余 =
        // Inconclusive。
        let list: Vec<Value> = match serde_json::from_str(text.trim()) {
            Ok(Value::Array(a)) => a,
            Ok(Value::Object(ref o)) => match o.get("entries") {
                Some(Value::Array(a)) => a.clone(),
                _ => return ReconcileOutcome::Inconclusive,
            },
            _ => return ReconcileOutcome::Inconclusive,
        };
        for item in &list {
            match match_item(item, match_expr) {
                ItemMatch::NoMatch => {}
                ItemMatch::Unreadable => return ReconcileOutcome::Inconclusive,
                ItemMatch::Hit => {
                    let n = crate::flow_proto::json_path_first(item, &protocol.result_ref)
                        .and_then(|v| v.as_i64());
                    let url = protocol
                        .result_url
                        .as_deref()
                        .and_then(|p| crate::flow_proto::json_path_first(item, p))
                        .and_then(|v| v.as_str().map(str::to_string))
                        .and_then(|u| crate::flow_proto::safe_external_url(&u));
                    // 命中但引用不可读 = 判不清，保守 Inconclusive（绝不
                    // 丢弃已命中事实去重试建条目）。
                    return match n {
                        Some(n) => ReconcileOutcome::Found(n, url),
                        None => ReconcileOutcome::Inconclusive,
                    };
                }
            }
        }
        match reconcile_advance(page, list.len(), protocol) {
            ReconcilePaging::Next(next) => page = next,
            ReconcilePaging::Absent => return ReconcileOutcome::Absent,
            ReconcilePaging::Inconclusive => return ReconcileOutcome::Inconclusive,
        }
    }
}

fn head_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests;
