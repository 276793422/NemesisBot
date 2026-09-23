// 消费者（BoardWritebackOutcome / write_back_board_dispatch）均为
// all(board,cluster) 门——随门收放免裁剪构建 unused import。
#[cfg(all(feature = "board", feature = "cluster"))]
use tracing::{info, warn};

/// Swarm M4 批作业（§6）：写回把 issue 推进到 in_review 时携带评审触发
/// 目标——回调闭包据此 spawn 验收 agent（`board.auto_review` 闸在评审
/// 任务内读配置判定）。
#[cfg(all(feature = "board", feature = "cluster"))]
pub(crate) struct BoardWritebackOutcome {
    pub(crate) is_board_task: bool,
    pub(crate) issue_for_review: Option<i64>,
    /// 本次回调真实终结了一条派发（finish_dispatch Ok(true)）。R-9 互斥
    /// 释放波据此触发（见调用点「派发落定重估波」）——幂等早退/并发
    /// 竞态/查表未中不触发。
    pub(crate) settled: bool,
}

/// W2 P2 派发写回：board 派发（`issue.dispatch`）的 task_id 命中
/// issue_dispatch 表 → 终结派发（幂等）+ worker 结果评论 + 状态推进
/// （成功 → in_review 等 coordinator 验收；失败留在 in_progress）。
/// 返回是否为 board 派发任务——是则 peer_chat_callback 跳过 agent 续行
/// 路由（board 派发无续行快照，进 bus 只会产生加载失败噪音）。
/// board store 未注入（打开失败）→ 恒 false，写回静默关闭。
/// 唯一调用点在 peer_chat_callback（cluster 编译时）——board-only 构建下
/// 该函数不可达，cfg 需同时含 cluster 以免 dead_code 告警。
///
/// Swarm M3 交付线程（§5.6/G5）：成功且 worker 按四段汇报格式回流 →
/// ctype='delivery' 首评（原样保留，M4 验收 agent 同源解析）；没按格式 →
/// 全文当普通评论（诚实降级）。汇报超 64KB → 全文落资产 + 截断内联 +
/// 引用注记（全文走层 2 HTTP 资产拉取）。
#[cfg(all(feature = "board", feature = "cluster"))]
pub(crate) fn write_back_board_dispatch(
    board_store: &Option<std::sync::Arc<nemesis_board::BoardStore>>,
    workspace: &std::path::Path,
    task_id: &str,
    status: &str,
    response: &str,
    fail_class: &str,
) -> BoardWritebackOutcome {
    use nemesis_board::models::dispatch_state;

    let Some(bstore) = board_store.as_ref() else {
        return BoardWritebackOutcome {
            is_board_task: false,
            issue_for_review: None,
            settled: false,
        };
    };
    if task_id.is_empty() {
        return BoardWritebackOutcome {
            is_board_task: false,
            issue_for_review: None,
            settled: false,
        };
    }
    let disp = match bstore.get_dispatch(task_id) {
        Ok(Some(d)) => d,
        Ok(None) => {
            return BoardWritebackOutcome {
                is_board_task: false,
                issue_for_review: None,
                settled: false,
            };
        }
        Err(e) => {
            // 读失败≠非 board 任务，但也不能误吞 agent 续行——按既有路由
            // 处理（false），告警留痕。
            warn!("[Gateway] board dispatch lookup failed (task_id={task_id}): {e}");
            return BoardWritebackOutcome {
                is_board_task: false,
                issue_for_review: None,
                settled: false,
            };
        }
    };
    // D0b 修正：写回守卫只拦真正终态（done/failed/cancelled）——
    // RUNNING（已开跑）的派发回调必须照常写回，否则 issue 卡死 in_progress。
    let already_terminal = matches!(
        disp.state.as_str(),
        dispatch_state::DONE | dispatch_state::FAILED | dispatch_state::CANCELLED
    );
    if already_terminal {
        info!(
            "[Gateway] board dispatch {task_id} already terminal ({}), skip",
            disp.state
        );
        return BoardWritebackOutcome {
            is_board_task: true,
            issue_for_review: None,
            settled: false,
        };
    }

    let terminal = if status == "error" {
        dispatch_state::FAILED
    } else {
        dispatch_state::DONE
    };
    match bstore.finish_dispatch(task_id, terminal) {
        Ok(true) => {
            let worker_actor = nemesis_board::Actor::agent(&disp.worker_id);
            let (ctype, content) = if status == "error" {
                // 失败汇报不做格式判定——错误输出原样留痕。P2A（2026-09-12
                // NB-15）：worker 结构化失败分类（如有）落结构化标记行，
                // review_issue 读线程后据此避免同 worker 同模型盲重派
                //（能力类失败重派必复现）；人看也是一眼可读的分类。
                let mut c = format!("⛔ worker 汇报失败：\n\n{response}");
                if !fail_class.is_empty() {
                    c.push_str(&format!("\n\nfail_class: {fail_class}"));
                }
                (nemesis_board::CommentType::Comment, c)
            } else if nemesis_board::parse_delivery_report(response).is_some() {
                // 结构化汇报 → 交付线程首评（G5）。超限先做截断+资产注记。
                (
                    nemesis_board::CommentType::Delivery,
                    delivery_inline_or_asset(bstore, workspace, response),
                )
            } else {
                // 无格式 → 诚实降级为普通评论。
                (
                    nemesis_board::CommentType::Comment,
                    format!("✅ worker 汇报完成：\n\n{response}"),
                )
            };
            if let Err(e) = bstore.add_comment(nemesis_board::NewComment {
                issue_id: disp.issue_id,
                author: worker_actor.clone(),
                content,
                parent_id: None,
                ctype,
            }) {
                warn!("[Gateway] board writeback comment failed (task_id={task_id}): {e}");
            }
            // 成功 → in_review（coordinator/验收 agent 处置）；worker 上报
            // 失败（P1 error 回调）**同样**转 in_review 进验收决策链——
            // 失败评论（⛔ Comment）正是 review_issue 无 Delivery 时的诚实
            // 降级输入：锚点必然 FAIL 短路 → 走同一重派/预算/转人工漏斗。
            // 旧实现把失败单留在 in_progress 且不触发 review，max_redispatch
            // 预算耗不出去，单据卡死无人接手（2026-09-11 双端真机 S2 实证：
            // NB-15 重派轮 error 回调后 90s 无任何决策动作）。推进成功与
            // 失败均携带 M4 评审触发目标。
            //
            // P4/E4（看板项目档案 goal 合并批）分流：档案管线派发（基线行
            // 在场）且交付成功 = **合并先行**（E4 时序：交付→合并→in_review
            // →评审）——这里不转 in_review：变更集已落地 = 立即合并（落地腿
            // 先到），未落地 = 等 ingest 腿触发并补「📦 变更集在途」评论；
            // 评审由合并路径 spawn（issue_for_review=None）。失败派发与非
            // 档案管线走既有立即 in_review。
            let archive_pipeline = terminal == dispatch_state::DONE
                && matches!(bstore.get_dispatch_baseline(task_id), Ok(Some(_)));
            let mut issue_for_review = None;
            if archive_pipeline {
                match crate::board_archive_ingest::merge_and_maybe_review(task_id) {
                    crate::board_archive_ingest::MergeAttempt::WaitingChangeset
                    | crate::board_archive_ingest::MergeAttempt::WaitingDispatch => {
                        if let Err(e) = bstore.add_comment(nemesis_board::NewComment {
                            issue_id: disp.issue_id,
                            author: nemesis_board::Actor::system("board"),
                            content:
                                "📦 交付已收，变更集在途——执行档案落地后自动合并并进入验收评审。"
                                    .to_string(),
                            parent_id: None,
                            ctype: nemesis_board::CommentType::System,
                        }) {
                            warn!(
                                "[Gateway] board writeback in-flight comment failed (task_id={task_id}): {e}"
                            );
                        }
                    }
                    _ => {} // 合并/丢弃/停车/急停路径各自留痕，不重复评论
                }
            } else if let Ok(issue) = bstore.get_issue(disp.issue_id)
                && issue.status == nemesis_board::IssueStatus::InProgress
            {
                match bstore.transition_issue(
                    disp.issue_id,
                    nemesis_board::IssueStatus::InReview,
                    &worker_actor,
                ) {
                    Ok(_) => issue_for_review = Some(disp.issue_id),
                    Err(e) => {
                        warn!(
                            "[Gateway] board writeback transition failed (task_id={task_id}): {e}"
                        );
                    }
                }
            }
            info!(
                "[Gateway] board dispatch writeback done (task_id={task_id}, issue_id={}, state={terminal})",
                disp.issue_id
            );
            // C 里程碑 3（看板项目档案 goal P2）：交付落定 → records/NB-xx/
            // delivery.md + timeline。成功/失败交付都入档（零信息丢失）；
            // 写失败不阻塞写回（writer 内部 WARN+审计）；存量项目静默跳过。
            if let Ok(issue) = bstore.get_issue(disp.issue_id) {
                nemesis_board::archive_writer::write_delivery_milestone(
                    bstore,
                    &issue,
                    &disp.worker_id,
                    status != "error",
                    response,
                );
            }
            BoardWritebackOutcome {
                is_board_task: true,
                issue_for_review,
                settled: true,
            }
        }
        Ok(false) => {
            info!("[Gateway] board dispatch {task_id} finished concurrently, skip writeback");
            BoardWritebackOutcome {
                is_board_task: true,
                issue_for_review: None,
                settled: false,
            }
        }
        Err(e) => {
            warn!("[Gateway] board dispatch finish failed (task_id={task_id}): {e}");
            BoardWritebackOutcome {
                is_board_task: true,
                issue_for_review: None,
                settled: false,
            }
        }
    }
}

/// 汇报内联策略（交付线程首评）：≤64KB 原样内联；超限 → 全文落资产
/// （`delivery-<sha8>`）+ 截断内联 + 引用束注记（全文走层 2 HTTP）。
/// 资产写盘/登记/签发失败时诚实给无束注记，不丢截断内容也不炸写回。
#[cfg(all(feature = "board", feature = "cluster"))]
fn delivery_inline_or_asset(
    bstore: &nemesis_board::BoardStore,
    workspace: &std::path::Path,
    response: &str,
) -> String {
    use nemesis_board::report::MAX_INLINE_BYTES;

    if response.len() <= MAX_INLINE_BYTES {
        return response.to_string();
    }

    let head = floor_char_boundary(response, MAX_INLINE_BYTES);
    let sha = nemesis_board::sha256_bytes(response.as_bytes());
    let ref_name = format!("delivery-{}", &sha[..8]);

    // 签发引用束（与 board_asset 工具 publish 同源：同一 secret 文件 + 同一
    // node url 文件，token 在资产端点验证一致）。任一步失败 → 诚实注记，
    // 不丢截断内容也不炸写回。
    let bundle_json = (|| -> Option<String> {
        let assets_dir = nemesis_path::resolve_board_assets_dir_in_workspace(workspace);
        std::fs::create_dir_all(&assets_dir).ok()?;
        std::fs::write(assets_dir.join(&ref_name), response).ok()?;
        bstore
            .register_asset(nemesis_board::NewAsset {
                ref_name: ref_name.clone(),
                origin_issue: None,
                sha256: sha.clone(),
                size: response.len() as i64,
            })
            .ok()?;
        let secret = nemesis_board::load_or_create_secret(
            &nemesis_path::resolve_asset_secret_path_in_workspace(workspace),
        )
        .ok()?;
        let node_url = std::fs::read_to_string(
            nemesis_path::resolve_asset_node_url_path_in_workspace(workspace),
        )
        .ok()?;
        let node_url = node_url.trim().to_string();
        if node_url.is_empty() {
            return None;
        }
        let bundle = nemesis_board::issue_asset_bundle(
            &secret,
            &ref_name,
            &sha,
            response.len() as i64,
            &node_url,
            &crate::board_asset_tool::read_asset_node_id(workspace),
            nemesis_board::DEFAULT_TOKEN_TTL_SECS,
        );
        serde_json::to_string(&bundle).ok()
    })();

    match bundle_json {
        Some(json) => format!(
            "{head}\n\n[汇报全文 {len} 字节，超过 64KB 内联上限已截断——全文下载引用：\n```json\n{json}\n```]",
            len = response.len()
        ),
        None => format!(
            "{head}\n\n[汇报全文 {len} 字节，超过 64KB 内联上限已截断；资产存档/签发失败，全文未能存档]",
            len = response.len()
        ),
    }
}

/// 字节上限的安全切片（多字节字符向下取整到 char boundary）。
#[cfg(all(feature = "board", feature = "cluster"))]
fn floor_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}
