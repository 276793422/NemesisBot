//! P1 外循环（外部来源绑定，2026-10-25）：Gitea issue 轮询导入 + 状态回写。
//!
//! 职责（外循环装配计划 P1）：
//! 1. **导入循环**：gateway 内定时轮询外部仓库 issues（经主 loop
//!    `handle_tool_call` 完整 dispatch 瀑布调 `connector_invoke`，非旁路——
//!    estop/hidden/安全 8 层/凭据注入/回执全生效），新 issue → board 建单，
//!    `origin = TaskOrigin{origin_type: <connector>, origin_id: "owner/repo#N"}`
//!    按 origin 去重（已导入不再建单）。
//! 2. **任务专属副本**：首次导入前按来源物化 board 项目（一仓一项目），
//!    clone=true 时 `git_repo::clone_repo` 真克隆到
//!    `<workspace>/board-projects/<来源名>/`（origin 在场，P3 交付分支可
//!    直推远端）；克隆失败 = WARN + 本轮跳过导入（不建无法派发的单）。
//! 3. **状态回写**：BoardStore transition observer → 无界通道 → 消费者
//!    `issue_comment` 回帖外部仓库（→in_review/→done/→cancelled 三态）；
//!    失败只 warn，绝不阻塞主链。
//!
//! 装配点：`gateway::post_agent::init_post_agent`（board_store / agent_loop /
//! estop / cluster 引用齐备处）；来源清单读 `config.json`
//! `board.external_sources[]`（mtime 缓存，热改即时生效；空 = 整模块不装）。
//!
//! 边界（诚实声明）：
//! - 轮询翻页：`limit=20` 满页且有新导入则继续翻页（末页/满页零新导入
//!   即停，单轮硬上限 10 页；BUG-20261028-122 修复——此前固定首页 20 条）。
//! - 轮询结果体经 `HTTP_BODY_CAP`（8K 字符）截断——整段 JSON 解析失败时
//!   按括号配对抢救完整条目（截断尾部条目本轮丢失，WARN 可见）；一条都
//!   抢救不出来 = 轮询失败状态（`last_error` 可见），绝不伪成功。
//! - git2 无 TLS 后端：`https://` 克隆诚实报错，`http://`/本地路径可用
//!   （试验靶 Gitea 为 http）。
//! - 回写 v1 只评三类终态转移，编辑/关闭外动作不回写。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use nemesis_agent::r#loop::AgentLoop;
use nemesis_board::store::BoardStore;
use nemesis_config::ExternalSourceConfig;
use serde_json::json;

/// 轮询间隔运行期下限（配置更小值收拢到此——外部仓库友好，不给对端压测）。
const MIN_POLL_INTERVAL_SECS: u64 = 15;
/// issues_list 单页条数（默认按最新更新序返回）。
const POLL_PAGE_LIMIT: i64 = 20;
/// 单来源单轮翻页硬上限（防御闸：对端不翻页/去重失效时不无限烧请求；
/// 10 页 × 20 条 = 单轮最多回填 200 个 issue）。
const MAX_POLL_PAGES: i64 = 10;
/// 轮询循环空转步长。
const LOOP_TICK_SECS: u64 = 5;
/// 启动静默窗（等 gateway 组件装配完：connector 注册/cluster 启动）。
const STARTUP_DELAY_SECS: u64 = 15;
/// estop 生效期间的退避。
const ESTOP_BACKOFF_SECS: u64 = 30;

/// cluster 引用面（自动派发目标解析用）：cluster 编译形态 = 集群句柄；
/// 非 cluster 裁剪形态收敛为 `()`。两种形态**同参量同签名**——装配链
/// （spawn → spawn_poll_loop → poll_source → import_entry → dispatch_imported）
/// 调用点零 cfg 分叉，`nemesis_cluster` 路径只在 cluster 形态被命名。
#[cfg(feature = "cluster")]
type ClusterOwned = Option<Arc<nemesis_cluster::cluster::Cluster>>;
#[cfg(not(feature = "cluster"))]
type ClusterOwned = ();
#[cfg(feature = "cluster")]
type ClusterOpt<'a> = Option<&'a Arc<nemesis_cluster::cluster::Cluster>>;
#[cfg(not(feature = "cluster"))]
type ClusterOpt<'a> = ();

/// 单来源轮询状态快照（WSAPI `board.external_sources` 数据源）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceStatus {
    /// 来源键 `connector/owner/repo`。
    pub source: String,
    pub enabled: bool,
    /// 上一轮成功轮询时刻（RFC3339 本地时区）。
    pub last_poll: Option<String>,
    /// 最近一次错误（诚实面：轮询/解析/物化/建单失败都落这里）。
    pub last_error: Option<String>,
    /// 最近一次警告（部分截断抢救：尾部条目本轮丢失且下轮同样截断=不自
    /// 愈；干净轮询一轮后清除。与 last_error 分槽——部分成功轮不是错误轮，
    /// BUG-20261028-122）。
    pub last_warning: Option<String>,
    /// 累计导入单数（本进程生命周期内）。
    pub imported_total: u64,
    /// 最近一轮导入的 origin_id（空 = 无新单）。
    pub last_imported: Vec<String>,
    /// 任务副本钉住的基线 HEAD（clone 成功后记录）。
    pub clone_base_sha: Option<String>,
    /// 回写成功 / 失败累计。
    pub writeback_ok: u64,
    pub writeback_failed: u64,
    /// 水位：本进程已导入的最大外部 issue 号（观测面——翻页停页判定走
    /// 「满页零新导入即停」，对端排序无关；水位只是导入深度的可读投影，
    /// BUG-20261028-122）。
    pub watermark: Option<i64>,
}

/// 回写请求（observer 侧产、消费者侧消；跨线程过无界通道）。
struct WriteBack {
    connector: String,
    origin_id: String,
    body: String,
    /// 状态快照更新定位键。
    source_key: String,
}

/// 运行时共享件（spawn 的轮询循环 / 回写消费者 / WSAPI 面三方持有）。
pub struct ExternalSourceRuntime {
    agent_loop: Arc<AgentLoop>,
    store: Arc<BoardStore>,
    workspace: PathBuf,
    home: PathBuf,
    estop: Arc<nemesis_agent::estop::EstopState>,
    /// 自动派发的集群句柄（周期轮询与 WSAPI 手动 sync 共用——手动路径
    /// 缺它会让 auto_dispatch 导入静默不派发）。非 cluster 形态 = `()`。
    #[cfg_attr(not(feature = "cluster"), allow(dead_code))] // 裁剪形态占位，读取点随 cfg 消失
    cluster: ClusterOwned,
    statuses: parking_lot::Mutex<Vec<SourceStatus>>,
    writeback_tx: tokio::sync::mpsc::UnboundedSender<WriteBack>,
}

/// config.json mtime 缓存的来源清单（每次循环现读 mtime，变了才重解析——
/// 与 sweep/CD6 同款纪律：dashboard/CLI 改配置即时生效，不改零 IO）。
#[derive(Default)]
struct SourcesCache {
    stamp: Option<(u64, u64)>,
    sources: Vec<ExternalSourceConfig>,
}

impl SourcesCache {
    fn load(&mut self, home: &Path) -> Vec<ExternalSourceConfig> {
        let path = home.join("config.json");
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => {
                self.stamp = None;
                self.sources.clear();
                return Vec::new();
            }
        };
        let mtime = meta
            .modified()
            .ok()
            .map(|t| {
                t.duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
            })
            .transpose()
            .ok()
            .flatten();
        let len = meta.len();
        let fresh = mtime.is_some() && self.stamp == mtime.map(|m| (m, len));
        if !fresh {
            let sources = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str::<nemesis_config::Config>(&raw).ok())
                .and_then(|c| c.board)
                .map(|b| {
                    b.external_sources
                        .into_iter()
                        .filter(|s| s.enabled)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            self.stamp = mtime.map(|m| (m, len));
            self.sources = sources;
        }
        self.sources.clone()
    }
}

/// 装配入口：状态表初始化 + 轮询循环 spawn + 回写 observer 安装 + 回写
/// 消费者 spawn + WSAPI 面注册。config 无来源 = 整模块不装（零开销）。
pub fn spawn(
    agent_loop: Arc<AgentLoop>,
    store: Arc<BoardStore>,
    workspace: PathBuf,
    home: PathBuf,
    estop: Arc<nemesis_agent::estop::EstopState>,
    cluster: ClusterOwned,
) {
    // 无来源配置 = 不装（worker 节点/未用外循环的 master 零波及）。
    let configured = std::fs::read_to_string(home.join("config.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<nemesis_config::Config>(&raw).ok())
        .and_then(|c| c.board)
        .map(|b| b.external_sources.into_iter().filter(|s| s.enabled).count())
        .unwrap_or(0);
    if configured == 0 {
        return;
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<WriteBack>();
    let mut statuses = Vec::new();
    {
        // 首帧状态表按配置展开（enabled=true 已过滤）。
        let cache = read_sources_once(&home);
        for s in cache {
            statuses.push(SourceStatus {
                source: format!("{}/{}/{}", s.connector, s.owner, s.repo),
                enabled: true,
                last_poll: None,
                last_error: None,
                imported_total: 0,
                last_imported: Vec::new(),
                clone_base_sha: None,
                writeback_ok: 0,
                writeback_failed: 0,
                watermark: None,
                last_warning: None,
            });
        }
    }
    let rt = Arc::new(ExternalSourceRuntime {
        agent_loop,
        store,
        workspace,
        home,
        estop,
        cluster,
        statuses: parking_lot::Mutex::new(statuses),
        writeback_tx: tx,
    });
    install_write_back(&rt);
    spawn_writeback_consumer(rt.clone(), rx);
    spawn_poll_loop(rt.clone());
    install_wsapi_surface(&rt);
    tracing::info!("[ExternalSource] 外循环装配完成：{configured} 个来源进入轮询");
}

fn read_sources_once(home: &Path) -> Vec<ExternalSourceConfig> {
    let mut cache = SourcesCache::default();
    cache.load(home)
}

// ---------------------------------------------------------------------------
// 轮询循环
// ---------------------------------------------------------------------------

fn spawn_poll_loop(rt: Arc<ExternalSourceRuntime>) {
    tokio::spawn(async move {
        let mut cache = SourcesCache::default();
        let mut last_round: HashMap<String, Instant> = HashMap::new();
        // cluster 引用降维：非 cluster 形态占位 `()`（统一签名）。
        #[cfg(feature = "cluster")]
        let cluster_ref: ClusterOpt<'_> = rt.cluster.as_ref();
        #[cfg(not(feature = "cluster"))]
        let cluster_ref: ClusterOpt<'_> = ();
        tokio::time::sleep(Duration::from_secs(STARTUP_DELAY_SECS)).await;
        loop {
            if rt.estop.is_engaged() {
                // 急停 = 冻结外循环活动（导入/派发都停），恢复后继续。
                tokio::time::sleep(Duration::from_secs(ESTOP_BACKOFF_SECS)).await;
                continue;
            }
            let sources = cache.load(&rt.home);
            for src in &sources {
                let key = format!("{}/{}/{}", src.connector, src.owner, src.repo);
                let interval =
                    Duration::from_secs(src.poll_interval_secs.max(MIN_POLL_INTERVAL_SECS));
                if last_round.get(&key).is_some_and(|t| t.elapsed() < interval) {
                    continue;
                }
                last_round.insert(key, Instant::now());
                poll_source(&rt, src, cluster_ref).await;
            }
            tokio::time::sleep(Duration::from_secs(LOOP_TICK_SECS)).await;
        }
    });
}

/// dispatch 返回文本的错误形态（handle_tool_call 恒 Ok(String)，错误以
/// "Tool error: " / "⛔" 前缀携带——runtime.rs invoke 同款识别法）。
/// pub(crate)：P3 publish 复用同一裁决（错误形态单一真相源，防漂移）。
pub(crate) fn is_tool_error(text: &str) -> bool {
    text.starts_with("Tool error: ") || text.starts_with("⛔")
}

async fn invoke_connector(
    rt: &ExternalSourceRuntime,
    connector: &str,
    op: &str,
    args: serde_json::Value,
) -> Result<String, String> {
    let call = nemesis_agent::types::ToolCallInfo {
        id: format!("external-{}", uuid::Uuid::new_v4()),
        name: crate::connector_tools::INVOKE_TOOL_NAME.to_string(),
        arguments: json!({"connector": connector, "op": op, "args": args}).to_string(),
    };
    let ctx = nemesis_agent::context::RequestContext::new(
        "external-source",
        "external-source",
        "external-source",
        "external-source",
    );
    let text = rt.agent_loop.handle_tool_call(&call, &ctx).await;
    if is_tool_error(&text) {
        return Err(text);
    }
    Ok(text)
}

async fn poll_source(
    rt: &ExternalSourceRuntime,
    src: &ExternalSourceConfig,
    cluster: ClusterOpt<'_>,
) {
    let key = format!("{}/{}/{}", src.connector, src.owner, src.repo);
    let run = async {
        // ① 物化来源项目（导入前；失败 = 本轮不建单）。翻页导入沿页推进，
        //    物化必须在首页之前完成。
        let project = match ensure_source_project(rt, src) {
            Ok(p) => p,
            Err(e) => {
                return Err(format!("任务副本物化失败（本轮跳过导入）: {e}"));
            }
        };
        // ② 翻页拉取 + 去重导入（BUG-20261028-122：此前固定首页 limit=20，
        //    第 21 条起的 issue 永远导入不了；issues_list 默认按最新更新序
        //    返回，新 issue 恒在首页——满页零新导入即停，稳态 1 请求/轮）。
        let mut imported = Vec::new();
        // 本轮已导入的最大外部 issue 号（水位投影，跨轮取 max）。
        let mut watermark: Option<i64> = None;
        let mut page: i64 = 1;
        loop {
            let text = invoke_connector(rt, &src.connector, "issues_list", page_args(src, page))
                .await?;
            // ③ 解析（8K 截断兜底：括号配对抢救完整条目）。
            let (entries, truncated) = parse_issue_list(&text);
            if entries.is_empty() && !text.trim_start().starts_with('[') {
                return Err(format!(
                    "issues_list 第 {page} 页返回非 JSON 数组形态: {}",
                    head(&text, 200)
                ));
            }
            // 截断诚实化（BUG-20261028-122：旧实现「下轮重取」是空头支票
            // ——下轮还是同一个被截断的首页）：一条都抢救不出来 = 轮询失败
            // 状态（外部可见 last_error），绝不伪造成功。
            if truncation_blocked(truncated, entries.len()) {
                return Err(format!(
                    "issues_list 第 {page} 页响应超 8K 截断且抢救出 0 条完整条目——本轮放弃（响应 {} 字节）",
                    text.len()
                ));
            }
            if truncated {
                let msg = format!(
                    "第 {page} 页响应超 8K 截断，抢救出 {} 条完整条目（页内尾部丢失；同参数下轮同样截断=不自愈，需调大 HTTP_BODY_CAP 或等结构化通道）",
                    entries.len()
                );
                tracing::warn!("[ExternalSource] {key}: {msg}");
                // 部分截断 ≠ 错误轮：导入照常推进，警告落独立槽（外部可见）。
                return Ok((imported, watermark, Some(msg)));
            }
            // ④ 去重导入 + 自动派发（沿页推进）。
            let mut newly_imported = false;
            for entry in &entries {
                if entry.get("pull_request").is_some() {
                    continue; // Gitea issues_list 混入 PR——不导入
                }
                let Some(number) = entry.get("number").and_then(|v| v.as_i64()) else {
                    continue;
                };
                let origin_id = format!("{}/{}/{}#{}", src.connector, src.owner, src.repo, number);
                if !rt
                    .store
                    .list_issues_by_origin(&src.connector, &origin_id, 1)
                    .unwrap_or_default()
                    .is_empty()
                {
                    continue; // 已导入
                }
                if !labels_match(src, entry) {
                    continue;
                }
                match import_entry(rt, src, &project, entry, &origin_id, cluster) {
                    Ok(board_no) => {
                        imported.push(board_no);
                        newly_imported = true;
                        watermark = Some(watermark.map_or(number, |w| w.max(number)));
                    }
                    Err(e) => {
                        tracing::warn!("[ExternalSource] {key}: 导入 {origin_id} 失败: {e}");
                    }
                }
            }
            // 停页判定（纯函数 [`should_stop_paging`]，回归锁钉死）：末页
            // （返回不足一页）或满页但零新导入（前沿已过）。
            if should_stop_paging(entries.len() as i64, newly_imported) {
                break;
            }
            page += 1;
            if page > MAX_POLL_PAGES {
                tracing::warn!(
                    "[ExternalSource] {key}: 翻页达硬上限 {MAX_POLL_PAGES} 页，本轮停止（对端未取尽——若常态出现请检查去重/水位）"
                );
                break;
            }
        }
        Ok((imported, watermark, None))
    }
    .await;
    let mut statuses = rt.statuses.lock();
    if let Some(st) = statuses.iter_mut().find(|s| s.source == key) {
        match run {
            Ok((imported, watermark, warning)) => {
                st.last_poll = Some(chrono::Local::now().to_rfc3339());
                st.last_error = None;
                // 干净轮询一轮即清警告（与 last_error=None 同语义）。
                st.last_warning = warning;
                st.imported_total += imported.len() as u64;
                st.last_imported = imported;
                if let Some(w) = watermark {
                    st.watermark = Some(st.watermark.map_or(w, |old| old.max(w)));
                }
            }
            Err(e) => {
                st.last_poll = Some(chrono::Local::now().to_rfc3339());
                let msg = head(&e, 300);
                if st.last_error.as_deref() != Some(msg.as_str()) {
                    tracing::warn!("[ExternalSource] {key} 轮询失败: {msg}");
                }
                st.last_error = Some(msg);
            }
        }
    }
}

/// 翻页请求体（wire 契约单一真相源，回归锁钉死）：`page` 是
/// BUG-20261028-122 修复核心——此前固定首页（无 page 参数），第 21 条起
/// 的 issue 永远导入不了。
fn page_args(src: &ExternalSourceConfig, page: i64) -> serde_json::Value {
    json!({
        "owner": src.owner,
        "repo": src.repo,
        "state": "open",
        "limit": POLL_PAGE_LIMIT,
        "page": page,
    })
}

/// 翻页停页判定（BUG-20261028-122 回归锁锚点）：末页（返回不足一页，
/// 对端已取尽）或满页但零新导入（前沿已过——全熟/全被标签滤掉）即停。
/// 前提：issues_list 默认最新更新序，新 issue 恒出现在首页——满页有新
/// 导入才值得继续翻。
fn should_stop_paging(fetched: i64, newly_imported: bool) -> bool {
    fetched < POLL_PAGE_LIMIT || !newly_imported
}

/// 截断诚实化判定（BUG-20261028-122 回归锁锚点）：响应被 8K 截断且一条
/// 完整条目都抢救不出来 = 本轮失败（真数据可能全在那个被斩断的尾部，
/// 旧代码 `[` 前缀守卫会把它当空列表伪成功 + 「下轮重取」空头支票）。
fn truncation_blocked(truncated: bool, salvaged: usize) -> bool {
    truncated && salvaged == 0
}

/// 标签过滤：源配置 `labels` 空 = 全收；非空 = issue 至少命中一个。
fn labels_match(src: &ExternalSourceConfig, entry: &serde_json::Value) -> bool {
    if src.labels.is_empty() {
        return true;
    }
    let issue_labels: Vec<String> = entry
        .get("labels")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| {
                    l.get("name")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    issue_labels.iter().any(|l| src.labels.contains(l))
}

/// 建 board 单（标题带 origin 前缀便于人读；AC 留空由
/// `store.create_issue` 的 ac_gen 单一真相点自动生成——NB-125 同源）。
fn import_entry(
    rt: &ExternalSourceRuntime,
    src: &ExternalSourceConfig,
    project: &nemesis_board::Project,
    entry: &serde_json::Value,
    origin_id: &str,
    cluster: ClusterOpt<'_>,
) -> Result<String, String> {
    let title = entry
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("(无标题)")
        .trim()
        .to_string();
    let body = entry
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let html_url = entry
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let mut description = body;
    if !html_url.is_empty() {
        description.push_str(&format!("\n\n---\n外部来源: {html_url}"));
    }
    let issue = rt.store.create_issue(nemesis_board::models::NewIssue {
        title: format!("[{origin_id}] {title}"),
        description,
        priority: nemesis_board::models::priority::MEDIUM,
        assignee: None,
        assignee_id: None,
        creator: nemesis_board::Actor::system("external-source"),
        parent_issue_id: None,
        project_id: Some(project.id),
        due_date: None,
        acceptance_criteria: None, // ac_gen 自动生成（NB-125 单一真相点）
        origin: Some(nemesis_board::models::TaskOrigin {
            origin_type: src.connector.clone(),
            origin_id: origin_id.to_string(),
        }),
        required_role: None,
        required_tags: Vec::new(),
        required_profession: None,
        required_capabilities: Vec::new(),
        required_backend: None,
    })?;
    tracing::info!(
        "[ExternalSource] 导入 {}: board {}（项目 {}）",
        origin_id,
        issue.number,
        project.name
    );
    if src.auto_dispatch {
        dispatch_imported(rt, cluster, &issue, src.target.as_deref());
    }
    Ok(issue.number)
}

/// 自动派发桥（双臂同参量）：cluster 形态带集群句柄选人；非 cluster 裁剪
/// 形态对 nemesis-web 的 4 参签名（web 侧双臂实现签名不同）。
#[cfg(feature = "cluster")]
fn dispatch_imported(
    rt: &ExternalSourceRuntime,
    cluster: ClusterOpt<'_>,
    issue: &nemesis_board::Issue,
    explicit_target: Option<&str>,
) -> bool {
    let actor = nemesis_board::Actor::system("external-source");
    nemesis_web::handlers::board::dispatch_issue_for_external_source(
        &rt.store,
        cluster,
        issue,
        explicit_target,
        &actor,
    )
}

#[cfg(not(feature = "cluster"))]
fn dispatch_imported(
    rt: &ExternalSourceRuntime,
    _cluster: ClusterOpt<'_>,
    issue: &nemesis_board::Issue,
    explicit_target: Option<&str>,
) -> bool {
    let actor = nemesis_board::Actor::system("external-source");
    nemesis_web::handlers::board::dispatch_issue_for_external_source(
        &rt.store,
        issue,
        explicit_target,
        &actor,
    )
}

// ---------------------------------------------------------------------------
// 任务专属副本（一仓一项目的 board 项目；克隆 = P3 交付分支可直推远端）
// ---------------------------------------------------------------------------

/// 物化来源项目（幂等）：同名项目在场即复用；否则解析目录 → 克隆（钉
/// base SHA）→ 脚手架投影文件 → `.git/info/exclude` 屏蔽投影 → 注册项目。
fn ensure_source_project(
    rt: &ExternalSourceRuntime,
    src: &ExternalSourceConfig,
) -> Result<nemesis_board::Project, String> {
    let name = format!("{}/{}/{}", src.connector, src.owner, src.repo);
    if let Some(p) = rt
        .store
        .list_projects()?
        .into_iter()
        .find(|p| p.name == name)
    {
        return Ok(p);
    }
    let projects = rt.store.list_projects()?;
    let existing_dirs: Vec<PathBuf> = projects
        .iter()
        .filter_map(|p| p.directory.clone())
        .map(PathBuf::from)
        .collect();
    let dir = nemesis_board::resolve_project_directory(None, &rt.workspace, &name, &existing_dirs)?;
    let mut base_sha = None;
    if src.clone {
        let url = match src
            .clone_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(u) => u.to_string(),
            None => derive_clone_url(rt, src)?,
        };
        let cred = clone_credentials(rt, src);
        let head = nemesis_board::git_repo::clone_repo(
            &url,
            &dir,
            cred.as_ref().map(|(u, p)| (u.as_str(), p.as_str())),
        )?;
        tracing::info!("[ExternalSource] {name} 副本克隆完成（基线 {head}）");
        base_sha = Some(head);
    }
    // 投影四件套（幂等；克隆仓内经 info/exclude 屏蔽，不进交付 PR）。
    nemesis_board::ensure_scaffold(&dir, 0, &name, "active")?;
    exclude_projection_files(&dir);
    let project = rt.store.create_project(
        &name,
        &format!("外部来源任务副本（{} {}）", src.connector, src.owner),
        None,
        "repo",
        "",
        Some(&dir.to_string_lossy()),
    )?;
    nemesis_board::sync_project_manifest(&rt.store, project.id);
    let mut statuses = rt.statuses.lock();
    if let Some(st) = statuses.iter_mut().find(|s| s.source == name) {
        st.clone_base_sha = base_sha;
    }
    Ok(project)
}

/// 克隆 URL 推导：显式 `clone_url` 优先；否则 connector base_url 剥
/// `/api/v1` 尾（Gitea 惯例）+ `/{owner}/{repo}.git`。剥不出 = 诚实报错
/// 让用户显式配（不做更多平台猜测）。
fn derive_clone_url(
    rt: &ExternalSourceRuntime,
    src: &ExternalSourceConfig,
) -> Result<String, String> {
    let (c, _) = nemesis_connectors::registry::find_connector(&rt.workspace, &src.connector)
        .map_err(|e| {
            format!(
                "connector `{}` 不可用: {e:?}（可在配置显式 clone_url）",
                src.connector
            )
        })?;
    let base = c.base_url.clone().ok_or_else(|| {
        format!(
            "connector `{}` 无 base_url（显式配置 clone_url）",
            src.connector
        )
    })?;
    let stripped = base
        .trim_end_matches('/')
        .strip_suffix("/api/v1")
        .map(|s| s.to_string())
        .ok_or_else(|| {
            format!(
                "无法从 base_url `{base}` 推导克隆地址（仅支持 /api/v1 尾剥除）——请显式配置 clone_url"
            )
        })?;
    Ok(format!("{stripped}/{}/{}.git", src.owner, src.repo))
}

/// 克隆凭据：vault `connector/<id>/pat` 在场 → `(clone_username || owner, pat)`；
/// 缺席 → None（匿名/公开仓路径）。vault 读取失败不炸——克隆阶段诚实报错。
fn clone_credentials(
    rt: &ExternalSourceRuntime,
    src: &ExternalSourceConfig,
) -> Option<(String, String)> {
    let vault_path = nemesis_path::resolve_vault_path_in_workspace(&rt.workspace);
    match crate::vault_runtime::resolve_alias_at(
        &vault_path,
        &format!("connector/{}/pat", src.connector),
    ) {
        Ok(pat) if !pat.trim().is_empty() => Some((
            src.clone_username
                .clone()
                .unwrap_or_else(|| src.owner.clone()),
            pat,
        )),
        _ => None,
    }
}

/// 克隆副本的投影文件屏蔽：看板档案（project.json/timeline.jsonl/docs//
/// records//.gitignore）写进 `.git/info/exclude`（仓内不生效、不随仓库
/// 传播、不进 PR）。写 .gitignore 会成为未跟踪文件，worker `git add -A`
/// 会把看板档案提交进交付 PR——info/exclude 是唯一正确位置。
fn exclude_projection_files(root: &Path) {
    let git_dir = root.join(".git");
    if !git_dir.is_dir() {
        return;
    }
    let info = git_dir.join("info");
    if std::fs::create_dir_all(&info).is_err() {
        return;
    }
    let exclude = info.join("exclude");
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    let want = [
        "/project.json",
        "/timeline.jsonl",
        "/docs/",
        "/records/",
        "/.gitignore",
    ];
    let mut out = existing.clone();
    for w in want {
        if !existing.lines().any(|l| l.trim() == w) {
            out.push_str(w);
            out.push('\n');
        }
    }
    if out != existing {
        let _ = std::fs::write(&exclude, out);
    }
}

// ---------------------------------------------------------------------------
// 解析（8K 截断兜底）
// ---------------------------------------------------------------------------

/// 解析 issues_list 返回体。首选整段 JSON；失败（HTTP_BODY_CAP 截断常态）
/// 按括号配对抢救 depth-1 完整对象。返回（条目，是否发生截断抢救）。
fn parse_issue_list(text: &str) -> (Vec<serde_json::Value>, bool) {
    let trimmed = text.trim_start();
    if !trimmed.starts_with('[') {
        return (Vec::new(), false);
    }
    if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(trimmed) {
        return (arr, false);
    }
    let entries = salvage_json_objects(trimmed);
    (entries, true)
}

/// 括号配对扫描：收集顶层数组内完整 `{...}` 对象（string 状态机感知）。
/// 深度语义：`{` 先增后判（depth 1 = 顶层对象起点）；`}` 先判后减
/// （depth 1 = 顶层对象收口 → 切片解析）。
fn salvage_json_objects(text: &str) -> Vec<serde_json::Value> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut start: Option<usize> = None;
    let mut out = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => {
                depth += 1;
                if depth == 1 {
                    start = Some(i);
                }
            }
            b'}' => {
                if depth == 1
                    && let Some(s) = start.take()
                    && let Ok(v) = serde_json::from_str(&text[s..=i])
                {
                    out.push(v);
                }
                depth = (depth - 1).max(0);
            }
            _ => {}
        }
    }
    out
}

fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ---------------------------------------------------------------------------
// 状态回写（transition observer → 通道 → 消费者 issue_comment）
// ---------------------------------------------------------------------------

fn install_write_back(rt: &Arc<ExternalSourceRuntime>) {
    let rt = rt.clone();
    let store = rt.store.clone();
    store.set_transition_observer(Arc::new(move |issue, from, to| {
        // v1 只回写三类对外可见的终态转移。
        if !matches!(to, "in_review" | "done" | "cancelled") {
            return;
        }
        let Some(origin) = &issue.origin else {
            return;
        };
        // 只回写外部来源（origin_type 非内置族——autopilot/cron/channel/cli/
        // board 不回写；外部 connector 名与配置匹配由消费者凭据面自然裁决）。
        if matches!(
            origin.origin_type.as_str(),
            "autopilot" | "cron" | "channel" | "cli" | "board" | "manual"
        ) {
            return;
        }
        let body = render_writeback(issue, from, to);
        // origin_id="owner/repo#N" → 状态表键 "connector/owner/repo"。
        let repo_path = origin
            .origin_id
            .rsplit_once('#')
            .map(|(a, _)| a)
            .unwrap_or(&origin.origin_id);
        let _ = rt.writeback_tx.send(WriteBack {
            connector: origin.origin_type.clone(),
            origin_id: origin.origin_id.clone(),
            body,
            source_key: format!("{}/{}", origin.origin_type, repo_path),
        });
    }));
}

/// 回写文案（中文简报；含 board 单号可回查）。
fn render_writeback(issue: &nemesis_board::Issue, from: &str, to: &str) -> String {
    match to {
        "in_review" => format!(
            "🔄 **已提交评审**：该 issue 已导入看板（{}）并完成执行，进入验收环节（{} → in_review）。验收结论将回帖本 issue。",
            issue.number, from
        ),
        "done" => format!(
            "✅ **已完成**：该 issue 已通过验收收口（board {}，{} → done）。交付详情见看板任务档案。",
            issue.number, from
        ),
        "cancelled" => format!(
            "⛔ **已取消**：该任务在看板侧被取消（board {}，{} → cancelled）。",
            issue.number, from
        ),
        _ => String::new(),
    }
}

fn spawn_writeback_consumer(
    rt: Arc<ExternalSourceRuntime>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<WriteBack>,
) {
    tokio::spawn(async move {
        while let Some(wb) = rx.recv().await {
            // origin_id = "owner/repo#N" → owner/repo + index
            let Some((repo_path, index)) = wb.origin_id.rsplit_once('#') else {
                continue;
            };
            let Some((owner, repo)) = repo_path.rsplit_once('/') else {
                continue;
            };
            let result = invoke_connector(
                &rt,
                &wb.connector,
                "issue_comment",
                json!({
                    "owner": owner,
                    "repo": repo,
                    "index": index.parse::<i64>().unwrap_or(0),
                    "body": wb.body,
                }),
            )
            .await;
            let mut statuses = rt.statuses.lock();
            if let Some(st) = statuses.iter_mut().find(|s| s.source == wb.source_key) {
                match result {
                    Ok(_) => st.writeback_ok += 1,
                    Err(e) => {
                        st.writeback_failed += 1;
                        tracing::warn!(
                            "[ExternalSource] {} 回写失败（不阻塞主链）: {}",
                            wb.source_key,
                            head(&e, 300)
                        );
                    }
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// WSAPI 面（board.external_sources / board.external_sync）
// ---------------------------------------------------------------------------

fn install_wsapi_surface(rt: &Arc<ExternalSourceRuntime>) {
    let rt_list = rt.clone();
    let list = Box::new(move || {
        let statuses = rt_list.statuses.lock();
        json!({ "installed": true, "sources": *statuses })
    });
    let rt_sync = rt.clone();
    let sync_now = Box::new(move || {
        let rt = rt_sync.clone();
        Box::pin(async move { sync_all_once(&rt).await })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
            >
    });
    if nemesis_web::handlers::board::install_external_source_surface(Arc::new(
        nemesis_web::handlers::board::ExternalSourceSurface { list, sync_now },
    )) {
        tracing::info!("[ExternalSource] WSAPI 状态面已装配");
    }
}

/// 手动触发：立即对全部来源各跑一轮 poll（不按间隔闸），汇总结果。
async fn sync_all_once(rt: &Arc<ExternalSourceRuntime>) -> Result<serde_json::Value, String> {
    let sources = read_sources_once(&rt.home);
    if sources.is_empty() {
        return Err("无已启用的外部来源（board.external_sources 为空）".into());
    }
    let mut results = Vec::new();
    // 与周期轮询同源：手动同步的 auto_dispatch 也带集群句柄（缺它 = 导入
    // 后静默不派发）。
    #[cfg(feature = "cluster")]
    let cluster_ref: ClusterOpt<'_> = rt.cluster.as_ref();
    #[cfg(not(feature = "cluster"))]
    let cluster_ref: ClusterOpt<'_> = ();
    for src in &sources {
        poll_source(rt, src, cluster_ref).await;
        let key = format!("{}/{}/{}", src.connector, src.owner, src.repo);
        let statuses = rt.statuses.lock();
        if let Some(st) = statuses.iter().find(|s| s.source == key) {
            results.push(json!({
                "source": st.source,
                "error": st.last_error,
                "last_imported": st.last_imported,
            }));
        }
    }
    Ok(json!({ "synced": true, "results": results }))
}

// ---------------------------------------------------------------------------
// 测试（纯函数面：解析抢救 / 标签过滤 / 回写文案 / exclude 幂等）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
