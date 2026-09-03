//! Unified path management for NemesisBot.

pub mod paths;

pub use paths::{
    ENV_CONFIG, ENV_HOME, ENV_MCP_CONFIG, ENV_SCANNER_CONFIG, ENV_SECURITY_CONFIG,
    ENV_SKILLS_CONFIG, PathManager, cluster_dir_in_workspace, default_path_manager, detect_local,
    expand_home, home_logs_dir, is_local_mode, legacy_models_catalog_cache_path,
    logs_dir_in_workspace, migrate_legacy_models_catalog_cache, models_catalog_cache_path,
    resolve_audit_log_dir_in_workspace, resolve_boundary_events_dir_in_workspace,
    resolve_chat_config_path_in_workspace, resolve_checkpoints_dir_in_workspace,
    resolve_cluster_config_path_in_workspace, resolve_cluster_logs_dir_in_workspace,
    resolve_cluster_peers_path_in_workspace, resolve_cluster_results_dir_in_workspace,
    resolve_cluster_rpc_cache_dir_in_workspace, resolve_cluster_state_path_in_workspace,
    resolve_commands_config_path_in_workspace, resolve_config_path,
    resolve_config_path_in_workspace, resolve_cors_config_path_in_workspace,
    resolve_enhanced_memory_config_path_in_workspace, resolve_forge_config_path_in_workspace,
    resolve_gateway_logs_dir_in_workspace, resolve_gateway_state_path_in_workspace,
    resolve_home_dir, resolve_hooks_config_path_in_workspace, resolve_mcp_config_path,
    resolve_mcp_config_path_in_workspace, resolve_request_logs_dir_in_workspace,
    resolve_scanner_config_path, resolve_scanner_config_path_in_workspace,
    resolve_security_config_path, resolve_security_config_path_in_workspace,
    resolve_session_logs_dir_in_workspace, resolve_sessions_dir_in_workspace,
    resolve_skills_cache_dir_in_workspace, resolve_skills_config_path,
    resolve_skills_config_path_in_workspace, resolve_spill_dir_for_home,
    resolve_spill_dir_in_workspace, resolve_state_dir_in_workspace,
    resolve_uploads_dir_in_workspace, set_local_mode, skills_dir_in_workspace,
    workspace_config_dir, workspace_data_dir, workspace_dir,
};
