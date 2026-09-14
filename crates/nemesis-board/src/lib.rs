//! Managed agent board — issue/comment/project data model + SQLite store.
//!
//! Manager 侧单写者权威看板（本地 `board.db`，无复制、无共识）。两级
//! manager+worker 模型里的"看板状态"层：worker 对看板无状态，只管执行 +
//! report；看板读写都发生在 manager 进程内（gateway 注入 [`BoardStore`]）。
//!
//! 设计文档：`docs/PLAN/2026-07-26_managed-agent-board-integration.md`
//! （架构定稿）+ `docs/PLAN/2026-07-26_managed-agent-board-integration_开发计划.md`
//! （P1-P4 任务分解）。crate 自包含 SQLite（不与 nemesis-data 的 usage.db 混），
//! 遵循 `nemesis-data/src/db.rs` 的 WAL + `user_version` 迁移模式。

pub mod anchor;
pub mod arbitrator;
pub mod archive;
pub mod archive_writer;
pub mod asset_token;
pub mod assignment;
pub mod backup;
pub mod db;
pub mod git_repo;
pub mod matcher;
pub mod models;
pub mod planner;
pub mod project_state;
pub mod quota;
pub mod report;
pub mod review;
pub mod service;
pub mod state_machine;
pub mod store;
pub mod team_memory;
pub mod watcher;

pub use anchor::{
    ANCHOR_PREFIX, AnchorCheck, AnchorKind, AnchorResult, RejectedAnchor, all_passed,
    parse_anchors, render_anchor_failures, render_anchor_summary, run_anchors,
    validate_anchor_path_shape,
};
pub use arbitrator::{
    NodeCandidate, SkipRecord, WakeInput, WakePlan, has_mentions, mentions_node,
    resolve_wake_targets,
};
pub use archive::{
    ProjectManifest, append_timeline, default_directory, ensure_scaffold, read_manifest,
    resolve_project_directory, sanitize_project_name, write_manifest,
};
pub use archive_writer::{
    ACTIVITY_ARCHIVE_WRITE_FAILED, sync_project_manifest, write_delivery_milestone,
    write_dispatch_milestone, write_plan_milestone, write_review_milestone,
};
pub use asset_token::{
    AdvertisedUrl, AssetSignContext, AssetTokenBundle, AssetTokenError, DEFAULT_TOKEN_TTL_SECS,
    issue_asset_bundle, load_or_create_secret, render_assets_section, sanitize_asset_ref,
    sha256_bytes, sha256_file, sign_asset_token, verify_asset_token,
};
pub use assignment::{Actor, AssignmentType};
pub use git_repo::{
    ChangesetFile, ConflictFile, MergeInput, MergeOutcome, commit_resolution, commit_worktree,
    ensure_repo, export_head_tree, head_commit_hex, looks_binary, merge_changeset,
};
pub use matcher::{MatchInput, PeerCandidate, rank_peers};
pub use models::{
    ActivityLog, Attachment, Autopilot, AutopilotPatch, BoardAsset, Channel, ChannelMember,
    ChannelMessage, Comment, CommentType, Issue, IssueFilter, IssuePatch, IssueStatus, NewAsset,
    NewAutopilot, NewChannel, NewChannelMessage, NewComment, NewIssue, NewNotification,
    Notification, Project, ProjectPatch, TaskOrigin, channel_message_type, notification_kind,
};
pub use planner::{
    MAX_SUBISSUES, PLANNER_SYSTEM_PROMPT, PlanParseError, PlannedSubIssue,
    build_planner_user_prompt, build_retry_prompt, parse_plan, parse_touch_paths,
};
pub use report::{DeliveryReport, REPORT_FORMAT_SECTION, TASK_CARD_HEADER, parse_delivery_report};
pub use review::{
    ExperienceNote, REVIEW_SYSTEM_PROMPT, ReviewOutput, ReviewParseError, ReviewVerdict,
    build_review_user_prompt, parse_review, selfcheck_request_text,
};
pub use service::BoardService;
pub use state_machine::{can_transition, validate_transition};
pub use store::BoardStore;
pub use team_memory::{
    MAX_MATCHED_EXPERIENCES, match_experiences, render_dispatch_experience_section,
    render_planner_experience_strings,
};
