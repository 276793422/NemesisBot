// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 board_review.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

/// 锚点双根核验的 async 安全包装（2026-10-17 根修）：`run_anchors_dual_ext`
/// 内部 std::process 同步等待（nemesis-board 无 tokio 依赖），直接在 async
/// fn 里调用时，exec 锚点命令挂住（cargo test 卡死等）会占死调用方 tokio
/// worker 最长 timeout_secs（可配到 24h）——同 worker 上的 SSE/WS 心跳全部
/// 停摆。统一丢 spawn_blocking（参数全部 owned）；Join 失败（线程 panic）
/// = 空结果 + 跳过注记（不进失败集，与 exec EnvSkip 同语义——评审照常，
/// 未核验项由注记诚实声明）。
pub(super) async fn run_anchors_dual_ext_blocking(
    anchors: &[nemesis_board::AnchorCheck],
    workspace_root: &std::path::Path,
    project_root: Option<&std::path::Path>,
    delivery_text: &str,
    exec_policy: &nemesis_board::ExecAnchorPolicy,
) -> (Vec<nemesis_board::AnchorResult>, Vec<String>) {
    let anchors = anchors.to_vec();
    let root = workspace_root.to_path_buf();
    let fallback = project_root.map(|p| p.to_path_buf());
    let text = delivery_text.to_string();
    let policy = exec_policy.clone();
    tokio::task::spawn_blocking(move || {
        nemesis_board::run_anchors_dual_ext(&anchors, &root, fallback.as_deref(), &text, &policy)
    })
    .await
    .unwrap_or_else(|e| {
        (
            Vec::new(),
            vec![format!("⏭ 锚点执行线程异常退出（{e}），本轮全部锚点未核验")],
        )
    })
}

/// B2a 执行型验收的只读工具白名单（评审 agent 取证轮可用工具；全部是
/// 现网注册工具名）。写类/网络副作用类不进名单——评审 agent 是只读角色，
/// 安全 8 层照常在 dispatch 闸拦截（纵深防御，白名单是第一道）。
/// `board_evidence_read`（evidence-on-demand）：档案围栏只读，本机/远端
/// worker 评审都可用（档案已合并到 master 本机，无张冠李戴面）。
pub(super) const READONLY_REVIEW_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "grep",
    "git",
    "web_fetch",
    "lsp",
    "run_checks",
    crate::board_evidence_tool::TOOL_NAME,
];

/// EvidenceOnly 模式的工具面：只给档案取证读（远端 worker 评审——本机
/// 工作区工具无意义还可能误读，档案围栏工具是唯一有牙齿的通路）。
pub(super) const EVIDENCE_ONLY_TOOLS: &[&str] = &[crate::board_evidence_tool::TOOL_NAME];

/// B3（profession-capability-evolution 2026-10-14）：架构工件禁令节的节头
/// 前缀（与 architecture 契约的强制工件节对齐；匹配按前缀找，容许标题
/// 尾缀差异）。
const ARCH_BAN_SECTION_HEADER: &str = "## 禁令与约束清单";

/// B3 服务端预取：抽取架构工件的禁令节文本（节 = 从节头到下一个 `## `
/// 或文末）。节缺席 → None（不造空节，lens 清单自带「无法核对」诚实臂）。
pub(super) fn extract_ban_section(text: &str) -> Option<String> {
    let start = text.find(ARCH_BAN_SECTION_HEADER)?;
    let rest = &text[start..];
    let end = rest[ARCH_BAN_SECTION_HEADER.len()..]
        .find("\n## ")
        .map(|off| ARCH_BAN_SECTION_HEADER.len() + off + 1)
        .unwrap_or(rest.len());
    Some(rest[..end].trim_end().to_string())
}

/// B3 服务端预取：读架构工件（`docs/architecture.md`）禁令节，作为引用
/// 文本段拼进评审 prompt——**不依赖评审 agent 自己去读**（三机部署下评审
/// 通常是 NoTools 纯文本形态，「让评审员自己读」在这条路上没有牙齿）。
/// 双根解析与 [`nemesis_board::run_anchors_dual`] 同源：主根 workspace 优先，
/// 项目目录 fallback（NB-22 同款：档案管线交付集合并在项目目录）。
/// 双根都读不到/节缺席 → None（评审照常，无禁令可核由 lens 清单诚实声明）。
pub(super) fn prefetch_ban_section(
    workspace: &std::path::Path,
    project_fallback: Option<&std::path::Path>,
) -> Option<String> {
    for root in [Some(workspace), project_fallback] {
        let Some(root) = root else { continue };
        let path = root.join("docs").join("architecture.md");
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Some(section) = extract_ban_section(&text)
        {
            return Some(section);
        }
    }
    None
}

/// B2a 评审工具模式：`NoTools` 与历史行为字节等价（max_turns=1 纯文本）；
/// `ReadOnly` 允许只读白名单多轮取证（`u32::MAX` = 配置 0=不限的哨兵映射）；
/// `EvidenceOnly`（evidence-on-demand，2026-10-15）：远端 worker 评审的取证
/// 面——只有档案围栏工具 [`crate::board_evidence_tool::TOOL_NAME`]。历史
/// 约束「远端 worker 一律纯文本」的理由是张冠李戴（就地读文件只会读到
/// master 工作区）；交付档案已由系统合并到 master 本机
///（`board-projects/<项目>/`），档案围栏工具按单号解析档案根，读到的就是
/// 本次交付——顾虑不成立，注入节选不再需要独自承担正确性。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewToolMode {
    NoTools,
    ReadOnly { max_turns: u32 },
    EvidenceOnly { max_turns: u32 },
}
