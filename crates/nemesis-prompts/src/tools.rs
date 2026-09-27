//! 工具描述分档查表。
//!
//! 描述两档：lean（1-2 句精修，60 槽位全有）+ full（一句定位 + 用法要点 +
//! 何时不用 + 兄弟工具路由，高频 24 个）。档位→文本的选取纯查表；模型能力
//! 档位（ModelTier）→ 描述档位的映射留在消费方 nemesis-agent（依赖
//! nemesis-types，本 crate 保持零依赖）。
//!
//! 纪律：schema 一律不走查表（保持注册表原文）；full 首句 == lean 首句
//! （与语义折叠的首句截断天然兼容，[`first_sentence`] 是契约的权威实现，
//! 测试钉死）。

/// 工具描述档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescLevel {
    /// 精修版：1-2 句，带选择指引。小模型与低频工具用。
    Lean,
    /// 完整版：一句定位 + 用法要点 + 何时不用 + 兄弟工具路由。首句即
    /// lean 句（与语义折叠的首句截断天然兼容）。
    Full,
}

/// 取文本首个完整句（以中文句末标点收口；无句末标点取全文）。
/// 「full 首句 == lean 首句」纪律与 tool_doc_folding 首句折叠的权威实现。
pub fn first_sentence(text: &str) -> &str {
    let t = text.trim();
    match t.find(['。', '！', '？']) {
        Some(i) => &t[..i + '。'.len_utf8()],
        None => t,
    }
}

/// 工具描述查表：(工具名, lean 文案, Option<full> 文案)。60 个命名槽位
/// 全有 lean，高频 24 个另有 full；查表未命中由调用方回落注册表原文
/// （forge/board/动态 MCP 工具自然回落，分批补表）。
static TOOL_DESCRIPTIONS: &[(&str, &str, Option<&str>)] = &[
    (
        "append_file",
        include_str!("tools/append_file.lean.md"),
        Some(include_str!("tools/append_file.md")),
    ),
    (
        "background_kill",
        include_str!("tools/background_kill.lean.md"),
        None,
    ),
    (
        "background_output",
        include_str!("tools/background_output.lean.md"),
        Some(include_str!("tools/background_output.md")),
    ),
    (
        "background_start",
        include_str!("tools/background_start.lean.md"),
        Some(include_str!("tools/background_start.md")),
    ),
    (
        "board_asset",
        include_str!("tools/board_asset.lean.md"),
        None,
    ),
    (
        "board_discuss",
        include_str!("tools/board_discuss.lean.md"),
        None,
    ),
    (
        "board_issue",
        include_str!("tools/board_issue.lean.md"),
        Some(include_str!("tools/board_issue.md")),
    ),
    (
        "claude_code",
        include_str!("tools/claude_code.lean.md"),
        Some(include_str!("tools/claude_code.md")),
    ),
    (
        "cli_reference",
        include_str!("tools/cli_reference.lean.md"),
        Some(include_str!("tools/cli_reference.md")),
    ),
    (
        "cluster_rpc",
        include_str!("tools/cluster_rpc.lean.md"),
        Some(include_str!("tools/cluster_rpc.md")),
    ),
    (
        "codex_delegate",
        include_str!("tools/codex_delegate.lean.md"),
        Some(include_str!("tools/codex_delegate.md")),
    ),
    (
        "complete_bootstrap",
        include_str!("tools/complete_bootstrap.lean.md"),
        None,
    ),
    (
        "create_dir",
        include_str!("tools/create_dir.lean.md"),
        Some(include_str!("tools/create_dir.md")),
    ),
    (
        "cron",
        include_str!("tools/cron.lean.md"),
        Some(include_str!("tools/cron.md")),
    ),
    ("delete_dir", include_str!("tools/delete_dir.lean.md"), None),
    (
        "delete_file",
        include_str!("tools/delete_file.lean.md"),
        Some(include_str!("tools/delete_file.md")),
    ),
    (
        "edit_file",
        include_str!("tools/edit_file.lean.md"),
        Some(include_str!("tools/edit_file.md")),
    ),
    (
        "exec",
        include_str!("tools/exec.lean.md"),
        Some(include_str!("tools/exec.md")),
    ),
    (
        "exec_async",
        include_str!("tools/exec_async.lean.md"),
        Some(include_str!("tools/exec_async.md")),
    ),
    (
        "find_skills",
        include_str!("tools/find_skills.lean.md"),
        Some(include_str!("tools/find_skills.md")),
    ),
    (
        "forge_build_mcp",
        include_str!("tools/forge_build_mcp.lean.md"),
        None,
    ),
    (
        "forge_create",
        include_str!("tools/forge_create.lean.md"),
        None,
    ),
    (
        "forge_evaluate",
        include_str!("tools/forge_evaluate.lean.md"),
        None,
    ),
    (
        "forge_learning_status",
        include_str!("tools/forge_learning_status.lean.md"),
        None,
    ),
    ("forge_list", include_str!("tools/forge_list.lean.md"), None),
    (
        "forge_reflect",
        include_str!("tools/forge_reflect.lean.md"),
        None,
    ),
    (
        "forge_share",
        include_str!("tools/forge_share.lean.md"),
        None,
    ),
    (
        "forge_update",
        include_str!("tools/forge_update.lean.md"),
        None,
    ),
    (
        "git",
        include_str!("tools/git.lean.md"),
        Some(include_str!("tools/git.md")),
    ),
    (
        "grep",
        include_str!("tools/grep.lean.md"),
        Some(include_str!("tools/grep.md")),
    ),
    (
        "history_search",
        include_str!("tools/history_search.lean.md"),
        Some(include_str!("tools/history_search.md")),
    ),
    ("i2c", include_str!("tools/i2c.lean.md"), None),
    (
        "install_skill",
        include_str!("tools/install_skill.lean.md"),
        Some(include_str!("tools/install_skill.md")),
    ),
    (
        "list_dir",
        include_str!("tools/list_dir.lean.md"),
        Some(include_str!("tools/list_dir.md")),
    ),
    (
        "lsp",
        include_str!("tools/lsp.lean.md"),
        Some(include_str!("tools/lsp.md")),
    ),
    (
        "mcp_discover",
        include_str!("tools/mcp_discover.lean.md"),
        Some(include_str!("tools/mcp_discover.md")),
    ),
    ("mcp_list", include_str!("tools/mcp_list.lean.md"), None),
    (
        "memory_forget",
        include_str!("tools/memory_forget.lean.md"),
        Some(include_str!("tools/memory_forget.md")),
    ),
    (
        "memory_list",
        include_str!("tools/memory_list.lean.md"),
        Some(include_str!("tools/memory_list.md")),
    ),
    (
        "memory_search",
        include_str!("tools/memory_search.lean.md"),
        Some(include_str!("tools/memory_search.md")),
    ),
    (
        "memory_store",
        include_str!("tools/memory_store.lean.md"),
        Some(include_str!("tools/memory_store.md")),
    ),
    (
        "message",
        include_str!("tools/message.lean.md"),
        Some(include_str!("tools/message.md")),
    ),
    (
        "multiedit",
        include_str!("tools/multiedit.lean.md"),
        Some(include_str!("tools/multiedit.md")),
    ),
    (
        "question",
        include_str!("tools/question.lean.md"),
        Some(include_str!("tools/question.md")),
    ),
    (
        "read_file",
        include_str!("tools/read_file.lean.md"),
        Some(include_str!("tools/read_file.md")),
    ),
    (
        "run_checks",
        include_str!("tools/run_checks.lean.md"),
        Some(include_str!("tools/run_checks.md")),
    ),
    (
        "run_script",
        include_str!("tools/run_script.lean.md"),
        Some(include_str!("tools/run_script.md")),
    ),
    (
        "skill_manage",
        include_str!("tools/skill_manage.lean.md"),
        None,
    ),
    (
        "skills_info",
        include_str!("tools/skills_info.lean.md"),
        None,
    ),
    (
        "skills_list",
        include_str!("tools/skills_list.lean.md"),
        Some(include_str!("tools/skills_list.md")),
    ),
    ("sleep", include_str!("tools/sleep.lean.md"), None),
    (
        "spawn",
        include_str!("tools/spawn.lean.md"),
        Some(include_str!("tools/spawn.md")),
    ),
    ("spi", include_str!("tools/spi.lean.md"), None),
    (
        "todowrite",
        include_str!("tools/todowrite.lean.md"),
        Some(include_str!("tools/todowrite.md")),
    ),
    (
        "web_fetch",
        include_str!("tools/web_fetch.lean.md"),
        Some(include_str!("tools/web_fetch.md")),
    ),
    (
        "web_search",
        include_str!("tools/web_search.lean.md"),
        Some(include_str!("tools/web_search.md")),
    ),
    (
        "workflow_capabilities",
        include_str!("tools/workflow_capabilities.lean.md"),
        None,
    ),
    (
        "workflow_create",
        include_str!("tools/workflow_create.lean.md"),
        None,
    ),
    (
        "workflow_run",
        include_str!("tools/workflow_run.lean.md"),
        Some(include_str!("tools/workflow_run.md")),
    ),
    (
        "write_file",
        include_str!("tools/write_file.lean.md"),
        Some(include_str!("tools/write_file.md")),
    ),
];

/// 按档位取描述文案；未命中返回 `None`（调用方回落注册表原文）。
pub fn description_for(name: &str, level: DescLevel) -> Option<&'static str> {
    TOOL_DESCRIPTIONS
        .iter()
        .find(|(n, _, _)| *n == name)
        .and_then(|(_, lean, full)| match level {
            DescLevel::Lean => Some(*lean),
            DescLevel::Full => full.or(Some(*lean)),
        })
}

/// 查表条目数（测试锚点用）。
pub fn table_len() -> usize {
    TOOL_DESCRIPTIONS.len()
}

/// 遍历查表条目（测试断言用：结构不变量校验）。
pub fn table_entries() -> &'static [(&'static str, &'static str, Option<&'static str>)] {
    TOOL_DESCRIPTIONS
}
