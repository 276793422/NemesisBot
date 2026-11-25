# Security Configuration Guide

## Per-OS Templates

Security policy is seeded on first run from the template matching the build
platform (compile-time embedded; `nemesisbot/src/main.rs` + `commands/onboard.rs`
Step 3, runtime filename resolution in `crates/nemesis-config/src/sections/io.rs`):

| Platform | Template file |
|----------|---------------|
| Windows  | `config.security.windows.json` |
| Linux    | `config.security.linux.json` |
| macOS    | `config.security.darwin.json` |
| Other    | `config.security.other.json` |

The seeded workspace file is `config/security/config.security.json` (template
content is copied at onboard; subsequent edits read that file). Deleting it and
re-running `onboard` restores factory defaults.

## Rule Actions

| Action | Runtime behavior |
|--------|------------------|
| `allow` | Operation permitted |
| `deny`  | Operation blocked |
| `ask`   | Operation pauses and raises an **approval card** (interactive approval) |

`ask` normalization (single truth source `crates/nemesis-security/src/auditor.rs`):
`"ask"`, `"require_approval"`, `"approval"`, `"pending"` all normalize to
`ask` (rule-action alias list), and `normalize_decision` maps it to
`SecurityDecision::RequireApproval`. The pending request surfaces as an
approval card:

- Dashboard modal via SSE `approval-requested` (respond with WSAPI
  `approval.respond`; `approval.pending` lists open cards);
- IM channels get an in-chat approval card (`ChannelApprovalManager`);
- 「总是允许」approved patterns are remembered in
  `config/approval_rules.json` (manage with
  `nemesisbot security approvals list|clear`).

Unanswered approval requests auto-**deny** after
`approval_timeout_seconds` (fail-closed).

## Key Fields

- `default_action`: fallback when no rule matches (`allow` / `ask` / `deny`).
- `exec_unknown_policy`: posture for `exec`/`spawn` with no matching rule
  (`allow` / `ask` / `deny`; empty = fall through to `default_action`).
- `approval_timeout_seconds`, `max_pending_requests`: approval card limits.
- `audit_log_retention_days`, `audit_log_file_enabled`,
  `log_all_operations`, `log_denials_only`: audit-chain controls.
- `layers.dlp.*`: DLP middleware settings (enabled / action / rules /
  `low_confidence_action` / `inbound_action`).
- `file_rules` / `dir_rules` / `process_rules` / `network_rules` /
  `registry_rules` / `hardware_rules`: pattern → action rule tables per
  operation class. See the shipped platform template for the factory
  rule set (dangerous commands ship as `deny`; destructive-shell and
  delete-wildcard rules ship as `ask` = approval card).
